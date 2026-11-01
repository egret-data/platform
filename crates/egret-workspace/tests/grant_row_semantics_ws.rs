//! P5b: workspace-side reads of `access_grants` that decide who is a member,
//! who is a `ws_admin` and who is a platform admin count only a live `allow`
//! grant (`egret_auth::live_allow_grant!`), the rule #810 set for the session
//! and the engine-admin set. A `deny` grant takes a role's actions away in
//! the PDP; it never makes its holder a member or an admin.
//!
//! No API writes deny or expiring workspace/platform grants today, so the rows
//! are inserted directly. DB-gated; self-skips without `DATABASE_URL` ONLY
//! under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::routing::get;
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use tower::ServiceExt;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(
            &format!("{}@t.local", uniq("p5b")),
            "U",
            &hash_password("pw").unwrap(),
        )
        .await
        .unwrap()
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
            db.clone(),
            MemberRepo::new(db.clone()),
        ),
        authz: Authorizer::new(db.clone()),
        pdp: std::sync::Arc::new(
            egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
                .await
                .expect("build CedarPdp"),
        ),
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

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

/// A grant of `role` (of `scope`) to `(ptype, pid)` on `scope_id`, with the
/// given effect, expiring `expires_in_secs` from now (`None`: never).
#[allow(clippy::too_many_arguments)]
async fn grant(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    scope: &str,
    scope_id: Option<Uuid>,
    role: &str,
    effect: &str,
    expires_in_secs: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, effect, expires_at) \
         SELECT $1, $2, id, $3, $4, $6, \
                CASE WHEN $7::bigint IS NULL THEN NULL \
                     ELSE now() + make_interval(secs => $7::bigint) END \
           FROM roles WHERE scope = $3 AND name = $5",
    )
    .bind(ptype)
    .bind(pid)
    .bind(scope)
    .bind(scope_id)
    .bind(role)
    .bind(effect)
    .bind(expires_in_secs)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// A user whose only grant on a workspace is a `deny` is not a member of it:
/// `WsContext` refuses them (403) and the workspace list leaves the workspace
/// out. A live allow grant is a member (the control).
#[tokio::test]
async fn a_deny_only_workspace_grant_makes_no_member() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    let denied = seed_user(&db).await;
    grant(&db, "user", denied.0, "workspace", Some(ws.id.0), "ws_viewer", "deny", None).await;
    let member = seed_user(&db).await;
    grant(&db, "user", member.0, "workspace", Some(ws.id.0), "ws_viewer", "allow", None).await;
    let app = axum::Router::new()
        .route(
            "/api/ws/{workspace}/probe",
            get(|WsContext(ctx): WsContext| async move { ctx.workspace_role }),
        )
        .with_state(st.clone());
    let probe = |user: UserId| {
        let app = app.clone();
        let sessions = st.sessions.clone();
        let slug = ws.slug.clone();
        async move {
            app.oneshot(
                HttpRequest::builder()
                    .uri(format!("/api/ws/{slug}/probe"))
                    .header("cookie", login_cookie(user, &sessions).await)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };

    // Act
    let denied_status = probe(denied).await;
    let member_status = probe(member).await;
    let denied_roles = st.authz.workspace_roles_for(denied, ws.id).await.unwrap();
    let denied_list = st.workspaces.list_for_principal(denied, false).await.unwrap();
    let member_list = st.workspaces.list_for_principal(member, false).await.unwrap();

    // Assert
    assert_eq!(
        denied_status,
        StatusCode::FORBIDDEN,
        "a deny-only grant must not admit its holder to the workspace"
    );
    assert_eq!(member_status, StatusCode::OK, "the control: a live allow grant admits");
    assert!(denied_roles.is_empty(), "a deny grant is no workspace role: {denied_roles:?}");
    assert!(
        !denied_list.iter().any(|w| w.id == ws.id),
        "a deny-only grant must not list the workspace"
    );
    assert!(member_list.iter().any(|w| w.id == ws.id));
}

/// The last-`ws_admin` guard counts only live `allow` grants: with one live
/// admin left beside a lapsed and a denied `ws_admin` row, removing or
/// demoting the live admin is refused (409). Removing the lapsed row is not
/// refused: it makes no one an admin.
#[tokio::test]
async fn the_last_ws_admin_guard_counts_only_live_allow_grants() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let st = ws_state(&db).await;
    let admin = seed_user(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", admin).await.unwrap();
    st.members
        .add(ws.id, "user", admin.0, "ws_admin", admin)
        .await
        .unwrap();
    let lapsed = seed_user(&db).await;
    grant(&db, "user", lapsed.0, "workspace", Some(ws.id.0), "ws_admin", "allow", Some(-60)).await;
    let denied = seed_user(&db).await;
    grant(&db, "user", denied.0, "workspace", Some(ws.id.0), "ws_admin", "deny", None).await;
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());
    let cookie = login_cookie(admin, &st.sessions).await;
    let send = |method: &str, uri: String, body: Option<serde_json::Value>| {
        let mut req = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("cookie", cookie.clone());
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        app.clone().oneshot(req)
    };

    // Act
    let demote = send(
        "POST",
        format!("/api/ws/{}/members", ws.slug),
        Some(serde_json::json!({
            "principal_type": "user", "principal_id": admin.0, "role": "ws_viewer"
        })),
    )
    .await
    .unwrap()
    .status();
    let remove = send(
        "DELETE",
        format!("/api/ws/{}/members/user/{}", ws.slug, admin.0),
        None,
    )
    .await
    .unwrap()
    .status();
    let remove_lapsed = send(
        "DELETE",
        format!("/api/ws/{}/members/user/{}", ws.slug, lapsed.0),
        None,
    )
    .await
    .unwrap()
    .status();

    // Assert
    assert_eq!(
        demote,
        StatusCode::CONFLICT,
        "a lapsed and a denied ws_admin row must not count as a second admin"
    );
    assert_eq!(remove, StatusCode::CONFLICT, "nor for a removal");
    assert_eq!(
        remove_lapsed,
        StatusCode::OK,
        "a lapsed ws_admin row is no admin, so removing it leaves the admin set as it was"
    );
    assert_eq!(st.members.count_ws_admins(ws.id).await.unwrap(), 1);
}

/// The member list's platform-admin badge is set only by a live `allow`
/// `platform_admin` grant.
#[tokio::test]
async fn the_member_list_badge_needs_a_live_allow_platform_admin_grant() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let st = ws_state(&db).await;
    let owner = seed_user(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    let denied = seed_user(&db).await;
    let lapsed = seed_user(&db).await;
    let admin = seed_user(&db).await;
    for u in [denied, lapsed, admin] {
        st.members
            .add(ws.id, "user", u.0, "ws_viewer", owner)
            .await
            .unwrap();
    }
    grant(&db, "user", denied.0, "platform", None, "platform_admin", "deny", None).await;
    grant(&db, "user", lapsed.0, "platform", None, "platform_admin", "allow", Some(-60)).await;
    grant(&db, "user", admin.0, "platform", None, "platform_admin", "allow", None).await;

    // Act
    let members = st.members.list(ws.id).await.unwrap();
    let badge = |u: UserId| {
        members
            .iter()
            .find(|m| m.principal_id == u.0)
            .expect("listed")
            .is_platform_admin
    };

    // Assert
    assert!(!badge(denied), "a deny grant is no platform admin");
    assert!(!badge(lapsed), "a lapsed grant is no platform admin");
    assert!(badge(admin), "the control: a live allow grant is");
}
