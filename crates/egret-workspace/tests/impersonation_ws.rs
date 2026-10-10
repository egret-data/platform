//! Task 9: ws_admin scoped impersonation + `WsContext` containment.
//!
//! DB-gated HTTP integration tests; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Exercises the `POST
//! /api/ws/{workspace}/impersonate` endpoint (local step-up) and the workspace-containment
//! that confines a ws-scoped overlay to the single workspace it was started in — the
//! ws_admin isolation boundary.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::model::Workspace;
use egret_workspace::routes::{me_router, ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// Seed a LOCAL user whose password is `"pw"` (so the inline step-up succeeds).
async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
    .await
    .unwrap()
}

async fn build_pdp(db: &Db) -> std::sync::Arc<dyn egret_core::authz::Pdp> {
    std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: egret_workspace::members::MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
            db.clone(),
            egret_workspace::members::MemberRepo::new(db.clone()),
        ),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: build_pdp(db).await,
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
        session_secure: false,
    }
}

fn app(st: WorkspaceState) -> axum::Router {
    axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .nest("/api/me", me_router())
        .with_state(st)
}

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

async fn add_member(st: &WorkspaceState, ws: &Workspace, user: UserId, role: &str, by: UserId) {
    st.members
        .add(ws.id, "user", user.0, role, by)
        .await
        .unwrap();
}

async fn send(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let b = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    app.clone()
        .oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(b)
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// ws_admin of W1 impersonates Bob (a member) via `POST /api/ws/W1/impersonate`
/// with the step-up password → 200, and the overlay is scoped to W1.
#[tokio::test]
async fn ws_admin_impersonates_member_scoped_to_w1() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let bob = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;
    add_member(&st, &w1, bob, "ws_viewer", owner).await;

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": bob.0.to_string(),
            "reason": "support ticket",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ws_admin may impersonate a member"
    );

    // The overlay exists and is scoped to W1.
    let scope: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT workspace_scope_id FROM impersonations \
         WHERE impersonator_id = $1 AND target_user_id = $2 AND expires_at > now()",
    )
    .bind(ws_admin.0)
    .bind(bob.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(scope, Some(w1.id.0), "overlay must be scoped to W1");
}

/// While impersonating Bob-in-W1: `GET /api/me/workspaces` lists ONLY W1, and a
/// W2-scoped route returns 403 (containment) even though Bob is a W2 member (in
/// fact a W2 ws_admin, so absent containment it would be 200).
#[tokio::test]
async fn scoped_impersonation_sees_only_w1() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let bob = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    let w2 = st
        .workspaces
        .create(&uniq("w2"), "W2", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;
    add_member(&st, &w1, bob, "ws_viewer", owner).await;
    // Bob is a ws_admin of W2 → absent containment, listing W2 members would 200;
    // a 403 therefore isolates the containment guard as the sole cause.
    add_member(&st, &w2, bob, "ws_admin", owner).await;

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;

    // Start the scoped impersonation. The SAME session cookie now resolves to Bob.
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": bob.0.to_string(),
            "reason": "support",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // /api/me/workspaces lists ONLY W1, though Bob belongs to both.
    let me = body_json(send(&app, &cookie, "GET", "/api/me/workspaces", None).await).await;
    let ids: Vec<String> = me
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&w1.id.0.to_string()), "W1 is visible");
    assert!(
        !ids.contains(&w2.id.0.to_string()),
        "W2 must NOT be visible under a W1-scoped impersonation"
    );

    // A W2-scoped route is denied by containment in `WsContext`, before membership
    // resolution — even though Bob is a ws_admin of W2.
    let resp = send(
        &app,
        &cookie,
        "GET",
        &format!("/api/ws/{}/members", w2.slug),
        None,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "containment must deny any workspace other than the scoped one"
    );
}

/// ws_admin cannot impersonate ANOTHER ws_admin of W1 → 403.
#[tokio::test]
async fn ws_admin_cannot_impersonate_another_ws_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let other_admin = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;
    add_member(&st, &w1, other_admin, "ws_admin", owner).await;

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": other_admin.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// ws_admin cannot impersonate a platform_admin → 403 (shared target guard).
#[tokio::test]
async fn ws_admin_cannot_impersonate_a_platform_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let padmin = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;
    // padmin is a platform_admin AND a member of W1 — the shared guard must still
    // reject them regardless of membership.
    add_member(&st, &w1, padmin, "ws_viewer", owner).await;
    st.authz
        .replace_platform_roles(padmin, &["platform_admin".into()])
        .await
        .unwrap();

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": padmin.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// **SP4d debt Task 13, over the real route: a `ws_admin` cannot impersonate a
/// member who holds the platform `data_steward` role.**
///
/// This is the first half of the chain Task 4 measured and left open, and this
/// test is the only one of the three that drives the actual HTTP endpoint — a
/// route handler's guard is not covered by `--lib`, and the guard it exercises
/// lives in `egret-auth`, two crates away from the route.
///
/// Why `data_steward` specifically and not an invented role: migration
/// `0059_rbac_permission_seed.sql` seeds it with the whole `policy:*` family, so a
/// scoped overlay over such a member satisfied the `policy:update` gate on
/// `POST /api/control-plane/perm/grants` — a WRITE on any workspace's catalog,
/// confined by nothing, because no `/api/control-plane` route resolves a
/// `{workspace}` and so none builds a `WsContext`.
///
/// Two targets, not one. `bob` holds no platform role and must still be
/// impersonable through the same request shape, or this test would also pass
/// against an endpoint that refuses everyone.
#[tokio::test]
async fn ws_admin_cannot_impersonate_a_platform_data_steward() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let steward = seed_user(&db).await;
    let bob = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;
    // Both targets are ordinary MEMBERS of W1 and neither is a ws_admin there, so
    // every other guard on this endpoint passes for both and the platform role is
    // the only difference between them.
    add_member(&st, &w1, steward, "ws_viewer", owner).await;
    add_member(&st, &w1, bob, "ws_viewer", owner).await;
    st.authz
        .replace_platform_roles(steward, &["data_steward".into()])
        .await
        .unwrap();

    // The fixture holds the state it guards: `data_steward` really carries
    // `policy:update` in THIS database, and the steward is not a platform_admin
    // (which the pre-Task-13 guard already refused, and which would make the
    // measurement vacuous).
    let resolved = {
        let issued = st.sessions.create(steward, "a", "").await.unwrap();
        st.sessions.validate(&issued.token).await.unwrap().unwrap()
    };
    assert!(
        resolved.permissions.iter().any(|p| p == "policy:update"),
        "migration 0059 must seed data_steward with policy:update, or this test \
         pins nothing; got {:?}",
        resolved.permissions
    );
    assert!(
        !resolved
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin"),
        "the steward must not be a platform_admin; roles: {:?}",
        resolved.platform_roles
    );

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;

    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": steward.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a ws_admin must not mint a workspace-scoped overlay over a holder of \
         platform authority: no /api/control-plane route confines one"
    );

    // The control leg: same request, a member with no platform role → started.
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": bob.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ws-scoped impersonation of an ordinary member must still work"
    );
}

/// A ws_editor (not a ws_admin) calling the endpoint → 403.
#[tokio::test]
async fn non_ws_admin_cannot_start() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let editor = seed_user(&db).await;
    let bob = seed_user(&db).await;
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, editor, "ws_editor", owner).await;
    add_member(&st, &w1, bob, "ws_viewer", owner).await;

    let app = app(st.clone());
    let cookie = login_cookie(editor, &st.sessions).await;
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": bob.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// ws_admin cannot impersonate someone who is NOT a member of W1 → 403.
#[tokio::test]
async fn cannot_impersonate_non_member() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws_admin = seed_user(&db).await;
    let stranger = seed_user(&db).await; // exists, but not a member of W1
    let w1 = st
        .workspaces
        .create(&uniq("w1"), "W1", owner)
        .await
        .unwrap();
    add_member(&st, &w1, ws_admin, "ws_admin", owner).await;

    let app = app(st.clone());
    let cookie = login_cookie(ws_admin, &st.sessions).await;
    let resp = send(
        &app,
        &cookie,
        "POST",
        &format!("/api/ws/{}/impersonate", w1.slug),
        Some(serde_json::json!({
            "target_user_id": stranger.0.to_string(),
            "reason": "x",
            "password": "pw",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
