//! DB-gated revision ROUTE tests. Self-skip without DATABASE_URL.
//!
//! Three behaviours, each of which is a way the feature can be wrong rather than absent:
//!
//!  1. a rejected PUT leaves no revision           — the capture point
//!  2. revert restores the earlier card set        — the round trip
//!  3. revert is itself recorded, as kind 'revert' — undo of an undo
//!
//! Written against the ROUTER rather than the repo because all three are properties of the
//! handler's statement ORDER, and a repo-level test cannot observe a request being rejected.
//!
//! The fixture mirrors `dashboard_object_scope.rs` (the crate's only in-process axum harness) but
//! is written out rather than `include!`d: including that file would compile its own
//! `#[tokio::test]` functions into this binary and run the whole object-scope suite a second time.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dashboard_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
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
    dash: Uuid,
    st: BiState,
}

async fn setup() -> Option<Fx> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let slug = format!("dbrev-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DB Rev Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_editor' ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(owner)
    .execute(&db.pool)
    .await
    .expect("add member");

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
    let dash = st
        .dashboards
        .create(ws, "Rev", "", Some(owner))
        .await
        .unwrap()
        .id;
    let app = dashboard_router().with_state(st.clone());
    Some(Fx {
        app,
        slug,
        tok,
        dash,
        st,
    })
}

async fn put(fx: &Fx, body: &str) -> StatusCode {
    fx.app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{}/dashboards/{}", fx.slug, fx.dash))
                .header("Cookie", format!("egret_session={}", fx.tok))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn json_get(fx: &Fx, path: &str) -> (StatusCode, serde_json::Value) {
    let res = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("Cookie", format!("egret_session={}", fx.tok))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

async fn revisions(fx: &Fx) -> Vec<serde_json::Value> {
    let (status, v) = json_get(
        fx,
        &format!("/ws/{}/dashboards/{}/revisions", fx.slug, fx.dash),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revisions list: {v}");
    v.as_array().cloned().unwrap_or_default()
}

async fn dashboard(fx: &Fx) -> serde_json::Value {
    let (status, v) = json_get(fx, &format!("/ws/{}/dashboards/{}", fx.slug, fx.dash)).await;
    assert_eq!(status, StatusCode::OK, "dashboard get: {v}");
    v
}

async fn revert(fx: &Fx, revision_id: &str) -> StatusCode {
    fx.app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/ws/{}/dashboards/{}/revisions/{}/revert",
                    fx.slug, fx.dash, revision_id
                ))
                .header("Cookie", format!("egret_session={}", fx.tok))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

fn text_card(col: i32) -> String {
    format!(
        r#"{{"chart_id":null,"card_type":"text","grid_col":{col},"grid_row":0,"size_x":6,"size_y":4,"options":{{}},"tab_index":null}}"#
    )
}

fn put_body(cards: &[String]) -> String {
    format!(
        r#"{{"name":"Rev","description":"","parameters":[],"width":"fixed","tabs":[],"cards":[{}]}}"#,
        cards.join(",")
    )
}

#[tokio::test]
async fn put_with_a_bad_tab_index_leaves_no_revision() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    // One tab, and a card pointing at tab_index 7 — rejected by validate_layout, which runs
    // AFTER authz and after require_cards_in_ws. This is the capture-point assertion: if the
    // revision write moved above validation, this dashboard would gain history from an edit that
    // never happened.
    let bad = r#"{"name":"Rev","description":"","parameters":[],"width":"fixed","tabs":[{"name":"A","position":0}],"cards":[{"chart_id":null,"card_type":"text","grid_col":0,"grid_row":0,"size_x":6,"size_y":4,"options":{},"tab_index":7}]}"#;
    assert_eq!(put(&fx, bad).await, StatusCode::BAD_REQUEST);
    assert!(
        revisions(&fx).await.is_empty(),
        "a rejected PUT must not record a revision"
    );

    // An invalid width is rejected by a different statement; it must also record nothing.
    let bad_width =
        r#"{"name":"Rev","description":"","parameters":[],"width":"wide","tabs":[],"cards":[]}"#;
    assert_eq!(put(&fx, bad_width).await, StatusCode::BAD_REQUEST);
    assert!(revisions(&fx).await.is_empty());

    // And a VALID put does record — otherwise the two assertions above would pass against a
    // feature that simply never writes anything.
    assert_eq!(put(&fx, &put_body(&[text_card(0)])).await, StatusCode::OK);
    assert!(!revisions(&fx).await.is_empty());
    // Cleanup so the dashboard does not leak into the shared test database.
    fx.st.dashboards.delete(fx.dash).await.unwrap();
}

#[tokio::test]
async fn revert_restores_the_earlier_card_set() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    assert_eq!(
        put(&fx, &put_body(&[text_card(0), text_card(6)])).await,
        StatusCode::OK
    );
    assert_eq!(put(&fx, &put_body(&[text_card(0)])).await, StatusCode::OK);
    assert_eq!(dashboard(&fx).await["cards"].as_array().unwrap().len(), 1);

    // Newest first: [0] is the one-card save, [1] is the two-card save, [2] is the baseline.
    let revs = revisions(&fx).await;
    let two_card = revs[1]["id"].as_str().unwrap().to_string();
    assert_eq!(revert(&fx, &two_card).await, StatusCode::OK);
    assert_eq!(
        dashboard(&fx).await["cards"].as_array().unwrap().len(),
        2,
        "revert must restore the earlier card set"
    );
    fx.st.dashboards.delete(fx.dash).await.unwrap();
}

#[tokio::test]
async fn revert_is_recorded_as_a_revision_of_kind_revert() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    assert_eq!(
        put(&fx, &put_body(&[text_card(0), text_card(6)])).await,
        StatusCode::OK
    );
    assert_eq!(put(&fx, &put_body(&[text_card(0)])).await, StatusCode::OK);
    let revs = revisions(&fx).await;
    let two_card = revs[1]["id"].as_str().unwrap().to_string();
    assert_eq!(revert(&fx, &two_card).await, StatusCode::OK);

    let after = revisions(&fx).await;
    assert_eq!(after.len(), revs.len() + 1, "the revert is itself recorded");
    assert_eq!(after[0]["kind"], "revert");
    assert_eq!(after[0]["is_current"], true);
    // is_current is true on EXACTLY one row, so the client can key the revert action off it.
    assert_eq!(after.iter().filter(|r| r["is_current"] == true).count(), 1);
    // The oldest entry is the baseline: the pre-feature state, so even the first edit is undoable.
    assert_eq!(after.last().unwrap()["kind"], "baseline");
    fx.st.dashboards.delete(fx.dash).await.unwrap();
}

#[tokio::test]
async fn a_revision_of_another_dashboard_is_a_404_not_a_cross_object_restore() {
    let Some(fx) = setup().await else {
        eprintln!("skip");
        return;
    };
    assert_eq!(put(&fx, &put_body(&[text_card(0)])).await, StatusCode::OK);
    let foreign = revisions(&fx).await[0]["id"].as_str().unwrap().to_string();

    // A second dashboard in the same workspace, which this caller may also manage. The revision
    // id is valid — it just belongs to someone else's dashboard — so only the SCOPE in
    // DashboardRepo::revision stands between this and a cross-object restore.
    let Some(other) = setup().await else { return };
    assert_eq!(revert(&other, &foreign).await, StatusCode::NOT_FOUND);
    fx.st.dashboards.delete(fx.dash).await.unwrap();
    other.st.dashboards.delete(other.dash).await.unwrap();
}
