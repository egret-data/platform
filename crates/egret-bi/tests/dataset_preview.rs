//! Object-scoped `POST /ws/{ws}/datasets/{id}/data` (Effort B / B2-2 Task 4).
//! DB-gated; self-skips without DATABASE_URL. Builds a REAL `CedarPdp` AFTER
//! `migrate()` so it snapshots the `ws_viewer` BI perms.
//!
//! Proves the dataset data-preview endpoint is object-scoped and runs ONLY a
//! bounded `SELECT * FROM <the dataset's own source> LIMIT n` (no client SQL —
//! the body carries at most a `limit`):
//!  * an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant
//!    on dataset D REACHES execution (not 403) — 200 with columns if a live
//!    engine answers, else 502 (engine reached but unavailable in CI);
//!  * the same guest is denied a sibling dataset D2 they have no grant on (403,
//!    BEFORE any execution);
//!  * a request BODY carrying a bogus `sql` is ignored (only `limit` is read) —
//!    posting it to the granted dataset is never a 400/415/422 and still runs.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dataset_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService, SourceSpec,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// Test stub: object-scope tests use non-delegated DBs, so mint is never called.
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
    }
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Seed a direct object grant of workspace `role` to `uid` on dataset `ds`.
async fn grant_object(db: &Db, uid: Uuid, ds: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'dataset', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(ds)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

/// POST the dataset data-preview endpoint with an optional request body, returning
/// (status, body-bytes). The endpoint reads at most a `limit`; any other body
/// field is ignored.
async fn post_dataset_data(
    app: &axum::Router,
    slug: &str,
    ds: Uuid,
    tok: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/datasets/{ds}/data"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn dataset_preview_endpoint_is_object_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("dsdata-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DS Data Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // A dataset needs a (granted) database connection to resolve its engine.
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // One OUTSIDE user (not a workspace member) who will get a Viewer object grant.
    let outside_viewer = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let outside_viewer_tok = session_for(&sessions, outside_viewer).await;

    // Two backing datasets. Virtual sql aliases a column so the SELECT * preview
    // executes cleanly if a live engine is present. D is shared with the outside
    // viewer; D2 is a sibling that is NOT shared.
    let source = SourceSpec::Virtual {
        sql: "SELECT 1 AS one".into(),
    };
    let cols = [NewColumn {
        column_name: "one".into(),
        data_type: "integer".into(),
        type_generic: "numeric".into(),
        is_temporal: false,
    }];
    let ds = st
        .datasets
        .create(
            ws,
            db_id,
            "D",
            "",
            &source,
            &cols,
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;
    let ds2 = st
        .datasets
        .create(
            ws,
            db_id,
            "D2",
            "",
            &source,
            &cols,
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    grant_object(&db, outside_viewer, ds, "ws_viewer", owner).await;

    let app = dataset_router().with_state(st);

    // 1. Outside VIEWER reaches execution on its granted dataset D (NOT 403/404/400).
    //    A live engine → 200 + columns; no engine in CI → 502 (engine reached).
    let (status, body) =
        post_dataset_data(&app, &slug, ds, &outside_viewer_tok, Body::empty()).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_GATEWAY,
        "outside ws_viewer object-grant must REACH the preview run on D (200 with a live \
         engine, else 502); got {status}, body={}",
        String::from_utf8_lossy(&body)
    );
    if status == StatusCode::OK {
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert!(
            v.get("columns")
                .and_then(|c| c.as_array())
                .is_some_and(|c| !c.is_empty()),
            "ChartRunResponse must carry columns; got {v}"
        );
        assert!(
            v.get("rows").and_then(|r| r.as_array()).is_some(),
            "must carry rows"
        );
    } else {
        eprintln!("note: no live query engine — authz reached run_query (502), rows gated");
    }

    // 2. The same guest is denied a sibling dataset it has NO grant on — 403 BEFORE
    //    any execution (the object gate rejects it).
    let (sibling_status, _) =
        post_dataset_data(&app, &slug, ds2, &outside_viewer_tok, Body::empty()).await;
    assert_eq!(
        sibling_status,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT preview an ungranted sibling D2 (403)"
    );

    // 3. The endpoint accepts NO client SQL: the body carries at most a `limit`, and
    //    any other field (a spoofed `sql`) is ignored. Posting it to the granted
    //    dataset is never a 400/415/422 — the SELECT * preview still runs as in (1).
    let malicious = Body::from(r#"{"sql":"DROP TABLE x","limit":150}"#);
    let (body_status, _) = post_dataset_data(&app, &slug, ds, &outside_viewer_tok, malicious).await;
    assert!(
        body_status != StatusCode::BAD_REQUEST
            && body_status != StatusCode::UNSUPPORTED_MEDIA_TYPE
            && body_status != StatusCode::UNPROCESSABLE_ENTITY,
        "endpoint must ignore any non-`limit` body field (no client SQL); got {body_status}"
    );
    assert!(
        body_status == StatusCode::OK || body_status == StatusCode::BAD_GATEWAY,
        "with the spoofed sql ignored, the SELECT * preview still runs (200/502); got {body_status}"
    );
}
