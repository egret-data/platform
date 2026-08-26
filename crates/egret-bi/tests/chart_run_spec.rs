//! `POST /ws/{workspace}/charts/run` (Task 6, E-notebooks feature-entitlement
//! program): execute a CLIENT-SUPPLIED `QuerySpec` against a dataset, server
//! side, and return `{columns, rows}` — without going through the free-form
//! SQL endpoint (`POST /queries`).
//!
//! This is a NEW file rather than an addition to `chart_data_object_scope.rs`:
//! that file is scoped to the OBJECT-scoped, saved-spec-only `/charts/{id}/data`
//! endpoint (`ch_run`); this one exercises a workspace-scoped, client-spec
//! endpoint with a different authorization shape (a real membership + role,
//! not an object grant) and a different reason for existing (the SQL Lab
//! entitlement split). Keeping them apart keeps each file's fixture focused on
//! what it actually proves.
//!
//! DB-gated: self-skips without `DATABASE_URL`, exactly like its neighbours.
//!
//! **What is and is not provable here:** the test environment does not run a
//! live Trino/StarRocks engine, so a request that reaches `run_query` cannot
//! be asserted as `200 OK` — only that authorization, the entitlement gate,
//! and dataset resolution all let it through, and it fails no earlier than
//! the connector. Where that matters, each test says so explicitly instead of
//! asserting a status it cannot honestly prove.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use egret_bi::{
    chart_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService, SourceSpec,
};
use egret_core::ids::WorkspaceId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// Test stub: these tests use non-delegated databases, so mint is never called.
struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: Uuid,
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

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

/// Seed a workspace (raw SQL, matching the neighbouring fixtures) plus a
/// database row for it. Returns `(workspace_id, database_id)`.
async fn seed_workspace(db: &Db, owner: Uuid) -> (Uuid, Uuid) {
    let slug = format!("chrun-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'CH Run Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("insert database");

    (ws, db_id)
}

struct Fixture {
    app: axum::Router,
    db: Db,
    slug: String,
    ws: WorkspaceId,
    dataset_id: Uuid,
    token: String,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let url = egret_test_db::url().await?;
        let db = Db::connect(&url, 5).await.expect("connect");
        db.migrate().await.expect("migrate");

        let owner = seed_user(&db).await;
        let (ws, db_id) = seed_workspace(&db, owner).await;
        let raw_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .expect("read back slug");

        let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
            egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
                .await
                .expect("build CedarPdp"),
        );
        let st = build_state(db.clone(), pdp);

        // The caller is a real workspace member (ws_admin), not an object
        // guest — `charts/run` is workspace-scoped `chart:read`, with no
        // object id to resolve.
        let members = MemberRepo::new(db.clone());
        let caller = seed_user(&db).await;
        members
            .add(
                WorkspaceId(ws),
                "user",
                caller,
                "ws_admin",
                egret_core::UserId(owner),
            )
            .await
            .expect("seed membership");
        let token = st
            .sessions
            .create(egret_core::ids::UserId(caller), "test", "127.0.0.1")
            .await
            .expect("create session")
            .token;

        let ds = st
            .datasets
            .create(
                ws,
                db_id,
                "DS",
                "",
                &SourceSpec::Virtual {
                    sql: "SELECT 1 AS id".into(),
                },
                &[NewColumn {
                    column_name: "id".into(),
                    data_type: "integer".into(),
                    type_generic: "numeric".into(),
                    is_temporal: false,
                }],
                Some(egret_core::UserId(owner)),
            )
            .await
            .unwrap()
            .id;

        let app = chart_router().with_state(st);

        Some(Fixture {
            app,
            db,
            slug: raw_slug,
            ws: WorkspaceId(ws),
            dataset_id: ds,
            token,
        })
    }

    /// Seed an unrelated workspace + database + dataset (a different owner, no
    /// membership for this fixture's caller) and return its dataset id — for
    /// proving cross-tenant reads are rejected.
    async fn seed_dataset_in_another_workspace(&self) -> Uuid {
        let other_owner = seed_user(&self.db).await;
        let (other_ws, other_db) = seed_workspace(&self.db, other_owner).await;
        let datasets = DatasetRepo::new(self.db.clone());
        datasets
            .create(
                other_ws,
                other_db,
                "OTHER-DS",
                "",
                &SourceSpec::Virtual {
                    sql: "SELECT 1 AS id".into(),
                },
                &[NewColumn {
                    column_name: "id".into(),
                    data_type: "integer".into(),
                    type_generic: "numeric".into(),
                    is_temporal: false,
                }],
                Some(egret_core::UserId(other_owner)),
            )
            .await
            .unwrap()
            .id
    }

    async fn post(&self, uri: &str, body: &str) -> Response {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Cookie", format!("egret_session={}", self.token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn json(&self, resp: Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
        )
    }
}

fn spec_body(dataset_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "dataset_id": dataset_id,
        "query_spec": { "metrics": [], "dimensions": [], "columns": ["id"],
                        "filters": [], "orderby": [], "row_limit": 10, "rollup_groups": [] }
    })
}

#[tokio::test]
async fn charts_run_executes_a_client_supplied_spec() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let body = spec_body(f.dataset_id);
    let resp = f
        .post(&format!("/ws/{}/charts/run", f.slug), &body.to_string())
        .await;

    let status = resp.status();
    // No live engine in this environment: authorization, the entitlement
    // gate, and dataset resolution must all have let the request through, so
    // it can only fail (if at all) inside `run_query` — 502/500, never
    // 400/403/404.
    assert!(
        status == StatusCode::OK
            || status == StatusCode::BAD_GATEWAY
            || status == StatusCode::INTERNAL_SERVER_ERROR,
        "expected the request to reach execution (200 with a live engine, else \
         502/500); got {status}"
    );
    if status == StatusCode::OK {
        let json = f.json(resp).await;
        assert!(json["columns"].is_array(), "response carries columns");
        assert!(json["rows"].is_array(), "response carries rows");
    } else {
        eprintln!(
            "note: no live query engine in this environment — proved the request reaches \
             run_query ({status}), not that it returns rows"
        );
    }
}

#[tokio::test]
async fn charts_run_rejects_a_dataset_from_another_workspace() {
    let Some(f) = Fixture::new().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let other = f.seed_dataset_in_another_workspace().await;

    let body = spec_body(other);
    let resp = f
        .post(&format!("/ws/{}/charts/run", f.slug), &body.to_string())
        .await;

    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-tenant reads are 404, not 403"
    );
}

#[tokio::test]
async fn charts_run_survives_sql_lab_being_switched_off() {
    // The whole reason this endpoint exists: the chart builder must keep
    // working in a workspace whose SQL Lab entitlement is revoked. `charts/*`
    // is gated by the `charts` feature (untouched here), not `sql_lab`.
    let Some(f) = Fixture::new().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, 'sql_lab', false)",
    )
    .bind(f.ws.0)
    .execute(&f.db.pool)
    .await
    .unwrap();

    let body = spec_body(f.dataset_id);
    let resp = f
        .post(&format!("/ws/{}/charts/run", f.slug), &body.to_string())
        .await;

    let status = resp.status();
    assert!(
        status != StatusCode::FORBIDDEN,
        "sql_lab being off must not gate charts/run; got {status}"
    );
    assert!(
        status == StatusCode::OK
            || status == StatusCode::BAD_GATEWAY
            || status == StatusCode::INTERNAL_SERVER_ERROR,
        "expected the request to reach execution untouched by the sql_lab gate; got {status}"
    );
}

#[tokio::test]
async fn charts_run_is_gated_by_its_own_charts_feature() {
    // Control for the test above: `charts/run` IS still subject to the
    // `charts` entitlement itself (it is a charts surface, just not a
    // sql_lab one).
    let Some(f) = Fixture::new().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    sqlx::query(
        "INSERT INTO workspace_features (workspace_id, feature, enabled) VALUES ($1, 'charts', false)",
    )
    .bind(f.ws.0)
    .execute(&f.db.pool)
    .await
    .unwrap();

    let body = spec_body(f.dataset_id);
    let resp = f
        .post(&format!("/ws/{}/charts/run", f.slug), &body.to_string())
        .await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = f.json(resp).await;
    assert_eq!(json["code"], "feature_disabled", "body={json}");
}
