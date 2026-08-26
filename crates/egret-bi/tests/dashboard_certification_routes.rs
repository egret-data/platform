//! DB-gated certification ROUTE tests. Self-skip without DATABASE_URL.
//!
//! WHY THESE EXIST. Certification's whole value rests on one property: a workspace editor must
//! not be able to certify their own dashboard. Everything else about the feature is cosmetic if
//! that fails. Before this file, that property was pinned in two places that both stop short of
//! the boundary — an rbac unit test asserting `dashboard:certify` is `Scope::Platform`, and a
//! frontend test asserting the button is withheld. Neither observes the API refusing, and a UI
//! that merely hides a control is not an authorization mechanism.
//!
//! The subject is a `ws_editor`: they hold `dashboard:manage` on this dashboard and can save it
//! freely. That is exactly the principal the platform-scoped perm exists to exclude, so "editor
//! can save but cannot certify" is the assertion, not "a stranger gets 403".
//!
//! Written against the ROUTER, like `dashboard_revision_routes.rs`, because a repo-level test
//! cannot observe a request being rejected. The fixture mirrors that file and is written out
//! rather than `include!`d for the reason stated there: including it would compile its
//! `#[tokio::test]` functions into this binary and run that suite a second time.

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
    user: Uuid,
    db: Db,
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("cert_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let slug = format!("dbcert-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DB Cert Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    // ws_editor: holds dashboard:manage on everything in this workspace, and NOT
    // dashboard:certify, which is platform-scoped.
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
        .create(ws, "Cert", "", Some(owner))
        .await
        .unwrap()
        .id;
    let app = dashboard_router().with_state(st.clone());
    Some(Fx {
        app,
        slug,
        tok,
        dash,
        user: owner,
        db,
    })
}

async fn req(
    fx: &Fx,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Cookie", format!("egret_session={}", fx.tok));
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

fn certify_path(fx: &Fx) -> String {
    format!("/ws/{}/dashboards/{}/certification", fx.slug, fx.dash)
}

/// THE assertion this file exists for. The same principal, in the same request sequence, may
/// save the dashboard and may not certify it.
#[tokio::test]
async fn a_workspace_editor_may_save_but_may_not_certify() {
    let Some(fx) = setup().await else { return };

    // Control: this principal really can write to this dashboard. Without it, a 403 below would
    // be indistinguishable from "the user has no access at all", which proves nothing.
    let (save, _) = req(
        &fx,
        "PUT",
        &format!("/ws/{}/dashboards/{}", fx.slug, fx.dash),
        Some(r#"{"name":"Cert","description":"","parameters":[],"tabs":[],"width":"full","cards":[]}"#),
    )
    .await;
    assert_eq!(save, StatusCode::OK, "a ws_editor must be able to SAVE");

    let (status, _) = req(
        &fx,
        "PUT",
        &certify_path(&fx),
        Some(r#"{"certified":true}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a ws_editor holds dashboard:manage and must NOT thereby be able to certify"
    );

    // And the refusal is real, not merely a status: nothing was written.
    let certified: bool =
        sqlx::query_scalar("SELECT certified FROM analytics.dashboards WHERE id = $1")
            .bind(fx.dash)
            .fetch_one(&fx.db.pool)
            .await
            .expect("read back");
    assert!(
        !certified,
        "a rejected certify must leave the column untouched"
    );
}

/// The same request, once the principal actually holds the platform perm. Without this, the test
/// above would pass against an endpoint that rejects EVERYONE — including a steward.
#[tokio::test]
async fn granting_the_platform_perm_lets_the_same_request_through() {
    let Some(fx) = setup().await else { return };

    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test steward') \
         RETURNING id",
    )
    .bind(format!("test_steward_{}", Uuid::new_v4().simple()))
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'dashboard:certify')",
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

    // The session caches its permission set, so a fresh one is needed for the new grant to apply.
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
    let fx2 = Fx {
        app: dashboard_router().with_state(st),
        tok,
        ..fx
    };

    let (status, body) = req(
        &fx2,
        "PUT",
        &certify_path(&fx2),
        Some(r#"{"certified":true,"details":"reviewed"}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a holder of dashboard:certify must be allowed"
    );
    assert_eq!(body["certified"], serde_json::json!(true));
    assert_eq!(body["certification_details"], serde_json::json!("reviewed"));
}

/// The structural claim, at the boundary: the ordinary PUT has no path to these columns. A
/// route-level test is the only place this is observable — the repo method simply does not take
/// them, which is the point, but a future edit could add them to `update` and nothing else here
/// would notice.
#[tokio::test]
async fn an_ordinary_save_cannot_change_certification() {
    let Some(fx) = setup().await else { return };

    fx_certify_directly(&fx).await;

    let (save, _) = req(
        &fx,
        "PUT",
        &format!("/ws/{}/dashboards/{}", fx.slug, fx.dash),
        Some(r#"{"name":"Renamed","description":"","parameters":[],"tabs":[],"width":"full","cards":[]}"#),
    )
    .await;
    assert_eq!(save, StatusCode::OK);

    let (certified, by): (bool, String) =
        sqlx::query_as("SELECT certified, certified_by FROM analytics.dashboards WHERE id = $1")
            .bind(fx.dash)
            .fetch_one(&fx.db.pool)
            .await
            .expect("read back");
    assert!(certified, "a save must not clear certification");
    assert_eq!(
        by, "steward@egret.local",
        "a save must not rewrite the certifier"
    );

    // Control: the save really happened.
    let name: String = sqlx::query_scalar("SELECT name FROM analytics.dashboards WHERE id = $1")
        .bind(fx.dash)
        .fetch_one(&fx.db.pool)
        .await
        .expect("read name");
    assert_eq!(name, "Renamed", "the PUT itself must have taken effect");
}

/// Certify through the repo, bypassing the route — this test is about what `update` does, not
/// about who may certify, and going through the endpoint would need the platform grant again.
async fn fx_certify_directly(fx: &Fx) {
    DashboardRepo::new(fx.db.clone())
        .set_certification(fx.dash, true, "steward@egret.local", "reviewed")
        .await
        .expect("certify");
}
