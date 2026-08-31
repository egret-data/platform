//! DB-gated dataset/metric certification ROUTE tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! WHY THESE EXIST. Certification rode on `dataset:manage` until 0109, which meant TWO defects,
//! and each needs its own assertion because closing one does not close the other:
//!
//!   1. SELF-CERTIFICATION — an editor could vouch for their own dataset.
//!   2. FORGED ATTRIBUTION — `certified_by` arrived in the REQUEST BODY, so an editor could
//!      record the certification as someone else's. This is the worse one, and a permission
//!      check alone does not fix it: a steward could still name a colleague. The fix is that the
//!      field no longer exists on the wire, and the test for it asserts the STORED value equals
//!      the session's email, not merely that the request succeeded.
//!
//! Metrics get the same treatment because a metric is what the semantic layer shows a reader; a
//! forgeable metric badge would make the dataset gate cosmetic.
//!
//! Written against the ROUTER, like `dashboard_certification_routes.rs`, because a repo test
//! cannot observe a request being refused. The fixture is written out rather than `include!`d for
//! the reason stated there: including it would compile that file's `#[tokio::test]` functions
//! into this binary and run its suite a second time.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dataset_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        saved: SavedQueryRepo::new(db.clone()),
        schema: SchemaService::new(),
        databases: egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

struct Fx {
    app: axum::Router,
    slug: String,
    tok: String,
    ds: Uuid,
    metric: Uuid,
    user: Uuid,
    email: String,
    db: Db,
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let email = format!("dscert_{}@egret.local", Uuid::new_v4().simple());
    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(&email)
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let slug = format!("dscert-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DS Cert Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    // ws_editor: holds dataset:manage on everything here, and NOT dataset:certify.
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_editor'",
    )
    .bind(ws)
    .bind(owner)
    .execute(&db.pool)
    .await
    .expect("add member")
    .rows_affected();
    assert_eq!(n, 1, "ws_editor must exist and have been granted");

    // A dataset + a metric, written directly: this file is about WHO MAY CERTIFY, and creating
    // them through the API would drag in a live engine for expression validation.
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, options) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, '{}'::jsonb) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("insert database");
    let ds: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, description, schema_name, table_name, created_by) \
         VALUES ($1,$2,'Cert DS','', 'public','t',$3) RETURNING id",
    )
    .bind(ws)
    .bind(db_id)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert dataset");
    let metric: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dataset_metrics \
           (dataset_id, metric_name, expression, data_type, type_generic, verbose_name, description, format, ordinal) \
         VALUES ($1,'m','count(*)','bigint','number','','','',0) RETURNING id",
    )
    .bind(ds)
    .fetch_one(&db.pool)
    .await
    .expect("insert metric");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let tok = st
        .sessions
        .create(egret_core::ids::UserId(owner), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    Some(Fx {
        app: dataset_router().with_state(st),
        slug,
        tok,
        ds,
        metric,
        user: owner,
        email,
        db,
    })
}

async fn req(
    fx: &Fx,
    tok: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={tok}"));
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let res = fx
        .app
        .clone()
        .oneshot(
            b.body(
                body.map(|s| Body::from(s.to_string()))
                    .unwrap_or(Body::empty()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn ds_path(fx: &Fx) -> String {
    format!("/ws/{}/datasets/{}", fx.slug, fx.ds)
}

/// Grant the platform perm to the fixture user and hand back a state whose session and PDP both
/// see it. A fresh session is required: the existing one cached its permission set.
async fn as_steward(fx: &Fx) -> (axum::Router, String) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test steward') \
         RETURNING id",
    )
    .bind(format!("test_ds_steward_{}", Uuid::new_v4().simple()))
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'dataset:certify')",
    )
    .bind(role)
    .execute(&fx.db.pool)
    .await
    .expect("grant perm");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(fx.user)
    .bind(role)
    .execute(&fx.db.pool)
    .await
    .expect("grant role");

    let st = build_state(
        fx.db.clone(),
        Arc::new(
            egret_auth::pdp::CedarPdp::build(fx.db.clone(), &egret_events::EventBus::new())
                .await
                .expect("rebuild CedarPdp"),
        ),
    );
    let tok = st
        .sessions
        .create(egret_core::ids::UserId(fx.user), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    (dataset_router().with_state(st), tok)
}

/// Defect 1. The editor may save the dataset and may not certify it.
#[tokio::test]
async fn a_workspace_editor_may_save_but_may_not_certify_a_dataset() {
    let Some(fx) = setup().await else { return };

    // Control: this principal really can write to this dataset. Without it a 403 below would be
    // indistinguishable from "no access at all", which proves nothing.
    let (save, _) = req(
        &fx,
        &fx.tok,
        "PUT",
        &ds_path(&fx),
        Some(r#"{"name":"Cert DS","description":"renamed"}"#),
    )
    .await;
    assert_eq!(save, StatusCode::OK, "a ws_editor must be able to SAVE");

    let (status, _) = req(
        &fx,
        &fx.tok,
        "PUT",
        &format!("{}/certification", ds_path(&fx)),
        Some(r#"{"certified":true}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "dataset:manage must not confer dataset:certify"
    );

    let certified: bool =
        sqlx::query_scalar("SELECT certified FROM analytics.datasets WHERE id = $1")
            .bind(fx.ds)
            .fetch_one(&fx.db.pool)
            .await
            .expect("read back");
    assert!(!certified, "a refused certify must write nothing");
}

/// The same, for a metric. Separate test because the metric route has its own handler and its own
/// `require_metric_in_dataset` guard; a fix applied to one is not a fix applied to the other.
#[tokio::test]
async fn a_workspace_editor_may_not_certify_a_metric() {
    let Some(fx) = setup().await else { return };

    let (status, _) = req(
        &fx,
        &fx.tok,
        "PUT",
        &format!("{}/metrics/{}/certification", ds_path(&fx), fx.metric),
        Some(r#"{"certified":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let certified: bool =
        sqlx::query_scalar("SELECT certified FROM analytics.dataset_metrics WHERE id = $1")
            .bind(fx.metric)
            .fetch_one(&fx.db.pool)
            .await
            .expect("read back");
    assert!(!certified);
}

/// Defect 2, and the reason a permission check alone is not enough: the certifier is the SESSION.
/// Asserts the STORED string, because a route that accepted a body-supplied name would still
/// return 200 here.
#[tokio::test]
async fn the_certifier_is_the_session_and_cannot_be_named_by_the_caller() {
    let Some(fx) = setup().await else { return };
    let (app, tok) = as_steward(&fx).await;
    let fx2 = Fx { app, tok, ..fx };

    // The body names someone else. The old update path would have honoured it.
    let (status, body) = req(
        &fx2,
        &fx2.tok,
        "PUT",
        &format!("{}/certification", ds_path(&fx2)),
        Some(
            r#"{"certified":true,"details":"reviewed","certified_by":"someone.else@egret.local"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a holder of dataset:certify passes");
    assert_eq!(body["certified"], serde_json::json!(true));
    assert_eq!(
        body["certified_by"],
        serde_json::json!(fx2.email),
        "the certifier must be the caller, NOT the name in the body"
    );
    assert_eq!(body["certification_details"], serde_json::json!("reviewed"));

    // And withdrawal clears the attribution rather than leaving a stale name attached.
    let (status, body) = req(
        &fx2,
        &fx2.tok,
        "PUT",
        &format!("{}/certification", ds_path(&fx2)),
        Some(r#"{"certified":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["certified"], serde_json::json!(false));
    assert_eq!(body["certified_by"], serde_json::json!(""));
}

/// The structural claim: the ordinary save has no path to these columns. A route test is the only
/// place this is observable — `update` simply does not take them, which is the point, but a future
/// edit could re-add them and nothing else here would notice.
#[tokio::test]
async fn an_ordinary_save_cannot_change_dataset_certification() {
    let Some(fx) = setup().await else { return };
    DatasetRepo::new(fx.db.clone())
        .set_certification(fx.ds, true, "steward@egret.local", "reviewed")
        .await
        .expect("certify");

    let (save, _) = req(
        &fx,
        &fx.tok,
        "PUT",
        &ds_path(&fx),
        // The old field names, sent deliberately: serde must ignore them now.
        Some(r#"{"name":"Renamed","description":"","certified":false,"certified_by":"me@egret.local"}"#),
    )
    .await;
    assert_eq!(save, StatusCode::OK);

    let (certified, by): (bool, String) =
        sqlx::query_as("SELECT certified, certified_by FROM analytics.datasets WHERE id = $1")
            .bind(fx.ds)
            .fetch_one(&fx.db.pool)
            .await
            .expect("read back");
    assert!(certified, "a save must not clear certification");
    assert_eq!(
        by, "steward@egret.local",
        "a save must not rewrite the certifier"
    );

    // Control: the save really happened, so the two assertions above are not vacuous.
    let name: String = sqlx::query_scalar("SELECT name FROM analytics.datasets WHERE id = $1")
        .bind(fx.ds)
        .fetch_one(&fx.db.pool)
        .await
        .expect("read name");
    assert_eq!(name, "Renamed", "the PUT itself must have taken effect");
}
