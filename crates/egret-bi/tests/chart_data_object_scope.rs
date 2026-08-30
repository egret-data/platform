//! Object-scoped `POST /ws/{ws}/charts/{id}/data` (Effort B / B2-1b Task 2).
//! DB-gated; self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Builds a REAL `CedarPdp` AFTER
//! `migrate()` so it snapshots the `ws_viewer` BI perms.
//!
//! Proves the executed-chart-data endpoint is object-scoped and runs ONLY the
//! chart's SAVED query (no client-supplied spec — the handler takes only the
//! path id):
//!  * an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant
//!    on chart CH REACHES execution (not 403) — 200 with columns if a live
//!    engine answers, else 502 (engine reached but unavailable in CI);
//!  * the same guest is denied a sibling chart CH2 they have no grant on (403,
//!    BEFORE any execution);
//!  * a request BODY is ignored (no body extractor) — posting a bogus spec to
//!    the granted chart is never a 400/415/422.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    chart_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
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

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Seed a direct object grant of workspace `role` to `uid` on chart `ch`.
async fn grant_object(db: &Db, uid: Uuid, ch: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'chart', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(ch)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

/// POST the executed-chart-data endpoint with an optional request body, returning
/// (status, body-bytes). The handler takes ONLY the path id; any body is ignored.
async fn post_chart_data(
    app: &axum::Router,
    slug: &str,
    ch: Uuid,
    tok: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/charts/{ch}/data"))
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
async fn chart_data_endpoint_is_object_scoped() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("chdata-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'CH Data Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // charts.dataset_id → a dataset, which needs a (granted) database connection.
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

    // A backing dataset. Virtual sql aliases the column so a raw-records spec on
    // `one` resolves (and executes cleanly if a live engine is present).
    let ds = st
        .datasets
        .create(
            ws,
            db_id,
            "DS",
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 1 AS one".into(),
            },
            &[NewColumn {
                column_name: "one".into(),
                data_type: "integer".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    // A valid saved query spec lives under params.spec (as the frontend persists it).
    let spec = serde_json::json!({
        "metrics": [],
        "dimensions": [],
        "columns": ["one"],
        "filters": [],
        "orderby": [],
        "row_limit": 10,
        "rollup_groups": []
    });
    let params = serde_json::json!({ "spec": spec });

    // CH is shared with the outside viewer; CH2 is a sibling that is NOT shared.
    let ch = st
        .charts
        .create(ws, ds, "CH", "", "table", &params, Some(owner))
        .await
        .unwrap()
        .id;
    let ch2 = st
        .charts
        .create(ws, ds, "CH2", "", "table", &params, Some(owner))
        .await
        .unwrap()
        .id;

    grant_object(&db, outside_viewer, ch, "ws_viewer", owner).await;

    let app = chart_router().with_state(st);

    // 1. Outside VIEWER reaches execution on its granted chart CH (NOT 403/404/400).
    //    A live engine → 200 + columns; no engine in CI → 502 (engine reached).
    let (status, body) = post_chart_data(&app, &slug, ch, &outside_viewer_tok, Body::empty()).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_GATEWAY,
        "outside ws_viewer object-grant must REACH execution on CH (200 with a live \
         engine, else 502); got {status}, body={}",
        String::from_utf8_lossy(&body)
    );
    if status == StatusCode::OK {
        // Live engine answered: assert the response is a ChartRunResponse with columns.
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

    // 2. The same guest is denied a sibling chart it has NO grant on — 403 BEFORE
    //    any execution (the object gate rejects it).
    let (sibling_status, _) =
        post_chart_data(&app, &slug, ch2, &outside_viewer_tok, Body::empty()).await;
    assert_eq!(
        sibling_status,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT run data for ungranted sibling CH2 (403)"
    );

    // 3. The endpoint takes NO request body: a client-supplied spec is ignored
    //    (the handler has only a Path extractor). Posting a bogus body to the
    //    granted chart is never a 400/415/422 — it runs the SAVED query as in (1).
    let malicious = Body::from(r#"{"query_spec":{"row_limit":999999},"sql":"DROP TABLE x"}"#);
    let (body_status, _) = post_chart_data(&app, &slug, ch, &outside_viewer_tok, malicious).await;
    assert!(
        body_status != StatusCode::BAD_REQUEST
            && body_status != StatusCode::UNSUPPORTED_MEDIA_TYPE
            && body_status != StatusCode::UNPROCESSABLE_ENTITY,
        "endpoint must ignore any request body (no client spec); got {body_status}"
    );
    assert!(
        body_status == StatusCode::OK || body_status == StatusCode::BAD_GATEWAY,
        "with a body ignored, the SAVED query still runs (200/502); got {body_status}"
    );
}
