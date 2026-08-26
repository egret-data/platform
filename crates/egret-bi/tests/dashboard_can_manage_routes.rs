//! DB-gated `can_manage` ROUTE tests. Self-skip without DATABASE_URL.
//!
//! WHY THESE EXIST. `DashboardDetailDto.can_manage` is what decides whether the browser offers
//! an Edit button. Before this file the flag was pinned only in jsdom, with the value MOCKED —
//! those tests prove the button reacts to the flag and say nothing about the flag being right.
//! A server that returned `true` for every reader would pass all of them, and a `ws_viewer`
//! would be handed the editor again, which is the exact defect this was written to fix.
//!
//! The property is a CONJUNCTION, so both halves are asserted against the same dashboard:
//! a `ws_viewer` reads it and gets `false`, a `ws_editor` reads it and gets `true`. Either
//! assertion alone is satisfied by a constant.
//!
//! And the flag is only worth anything if it predicts the write. So the editor's `true` is
//! followed by a PUT that succeeds and the viewer's `false` by a PUT that is refused — that is
//! the claim the UI actually rests on, not the boolean itself.
//!
//! Written against the ROUTER, like `dashboard_certification_routes.rs`, because `can_manage` is
//! computed in `load_dashboard` from the request's `WorkspaceContext`; there is no repo-level
//! seam where it exists. The fixture mirrors that file and is written out rather than `include!`d
//! for the reason stated there: including it would compile its `#[tokio::test]` functions into
//! this binary and run that suite a second time.

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
    /// Holds `dashboard:manage` in this workspace.
    editor_tok: String,
    /// Holds `dashboard:read` and nothing more.
    viewer_tok: String,
    dash: Uuid,
    /// For the certification test, which sets the column through the repo rather than the
    /// endpoint — this file is about what the LIST reports, not about who may certify.
    db: Db,
}

async fn mk_user(db: &Db, tag: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("{tag}_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn grant_ws_role(db: &Db, user: Uuid, ws: Uuid, role: &str) {
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3",
    )
    .bind(ws)
    .bind(user)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add member")
    .rows_affected();
    // A typo'd role name would insert nothing and leave the user with NO access — which reads
    // as `can_manage: false` and would make the viewer assertion pass for the wrong reason.
    assert_eq!(n, 1, "role {role} must exist and have been granted");
}

async fn setup() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = mk_user(&db, "cm_editor").await;
    let viewer = mk_user(&db, "cm_viewer").await;
    let slug = format!("dbcm-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DB CanManage Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    grant_ws_role(&db, owner, ws, "ws_editor").await;
    grant_ws_role(&db, viewer, ws, "ws_viewer").await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let mut tok = Vec::new();
    for u in [owner, viewer] {
        tok.push(
            st.sessions
                .create(egret_core::ids::UserId(u), "test", "127.0.0.1")
                .await
                .expect("create session")
                .token,
        );
    }
    let dash = st
        .dashboards
        .create(ws, "CanManage", "", Some(owner))
        .await
        .unwrap()
        .id;
    Some(Fx {
        app: dashboard_router().with_state(st),
        slug,
        viewer_tok: tok.pop().unwrap(),
        editor_tok: tok.pop().unwrap(),
        dash,
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

fn dash_path(fx: &Fx) -> String {
    format!("/ws/{}/dashboards/{}", fx.slug, fx.dash)
}

const SAVE_BODY: &str =
    r#"{"name":"CanManage","description":"","parameters":[],"tabs":[],"width":"full","cards":[]}"#;

/// THE assertion. Same dashboard, same request, two principals — the flag must distinguish them.
/// Split into one test so a constant-`true` or constant-`false` implementation cannot pass by
/// having only one half run.
#[tokio::test]
async fn can_manage_distinguishes_a_viewer_from_an_editor() {
    let Some(fx) = setup().await else { return };

    let (s, editor) = req(&fx, &fx.editor_tok, "GET", &dash_path(&fx), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, viewer) = req(&fx, &fx.viewer_tok, "GET", &dash_path(&fx), None).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a ws_viewer must still be able to READ — otherwise the flag below is moot"
    );

    assert_eq!(
        editor["can_manage"],
        serde_json::json!(true),
        "a ws_editor holds dashboard:manage here"
    );
    assert_eq!(
        viewer["can_manage"],
        serde_json::json!(false),
        "a ws_viewer does not, and the Edit button is withheld on exactly this"
    );
}

/// The flag is only worth anything if it predicts the write. Without this, `can_manage` could be
/// computed from something adjacent to authorization and still satisfy the test above.
#[tokio::test]
async fn can_manage_agrees_with_what_the_save_actually_does() {
    let Some(fx) = setup().await else { return };

    let (s, _) = req(&fx, &fx.editor_tok, "PUT", &dash_path(&fx), Some(SAVE_BODY)).await;
    assert_eq!(s, StatusCode::OK, "can_manage:true must mean the PUT lands");

    let (s, _) = req(&fx, &fx.viewer_tok, "PUT", &dash_path(&fx), Some(SAVE_BODY)).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "can_manage:false must mean the PUT is refused — the flag exists to stop a reader \
         discovering this only after making their changes"
    );
}

/// The same claim for the LIST, which carries its own `can_manage` — workspace-scoped and
/// repeated on every row. The jsdom tests that gate the list's Duplicate/Trash/Delete menu mock
/// this value, so without this the destructive affordances rest on an unverified server field.
#[tokio::test]
async fn the_list_tells_a_viewer_it_cannot_manage() {
    let Some(fx) = setup().await else { return };
    let path = format!("/ws/{}/dashboards", fx.slug);

    let (s, editor) = req(&fx, &fx.editor_tok, "GET", &path, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, viewer) = req(&fx, &fx.viewer_tok, "GET", &path, None).await;
    assert_eq!(s, StatusCode::OK, "a ws_viewer must still see the list");

    // Control: both principals really are looking at the same non-empty list. An empty array
    // would make the two assertions below vacuous — `.iter().all()` is true of nothing.
    assert!(!editor.as_array().unwrap().is_empty());
    assert_eq!(
        viewer.as_array().unwrap().len(),
        editor.as_array().unwrap().len()
    );

    assert!(
        editor
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["can_manage"] == serde_json::json!(true)),
        "a ws_editor may duplicate/move/trash in this workspace"
    );
    assert!(
        viewer
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["can_manage"] == serde_json::json!(false)),
        "a ws_viewer may not, and the row menu is withheld on exactly this"
    );
}

/// `certified` reaches the list, so the badge is not decoration. Both states, because a field
/// hard-wired to either constant would satisfy a single-state assertion.
#[tokio::test]
async fn the_list_reports_certification_both_ways() {
    let Some(fx) = setup().await else { return };
    let path = format!("/ws/{}/dashboards", fx.slug);

    let (_, before) = req(&fx, &fx.editor_tok, "GET", &path, None).await;
    assert_eq!(
        before.as_array().unwrap()[0]["certified"],
        serde_json::json!(false),
        "a fresh dashboard is not certified"
    );

    DashboardRepo::new(fx.db.clone())
        .set_certification(fx.dash, true, "steward@egret.local", "reviewed")
        .await
        .expect("certify");

    let (_, after) = req(&fx, &fx.editor_tok, "GET", &path, None).await;
    assert_eq!(
        after.as_array().unwrap()[0]["certified"],
        serde_json::json!(true),
        "certifying must show up in the list, not only on the detail page"
    );
}
