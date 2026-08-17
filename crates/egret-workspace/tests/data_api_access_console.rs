//! Task 9 (P5a): the access-console arm and the member-picker over-grant.
//! DB-gated; self-skip without DATABASE_URL — same `connect`/`uniq`/`seed_user`/
//! `login_cookie`/`ws_state` setup as the sibling harnesses in this crate.
//!
//! Two regressions this pins:
//!
//! 1. Migration 0124 put `data_api_endpoint:invoke` on `ws_admin`, and Cedar's
//!    workspace→object cascade applies a workspace-scoped grant to every object
//!    in the workspace. `UserRepo::list` (which backs `GET /api/admin/users`,
//!    the Access console's principal picker) already excludes
//!    `provider = 'service'` rows outright — a service account is machine
//!    identity, not a colleague to add. This test pins that exclusion AND that
//!    the surviving rows still carry `provider`, so a regression that dropped
//!    either would be caught here instead of only in production.
//! 2. `data_api_invoker` (migration 0124) exists to be granted OBJECT-scoped to
//!    one service account for one endpoint. Offered as a workspace MEMBERSHIP
//!    role, it means "invoke every endpoint in this workspace" through the same
//!    cascade — so it must never appear in the role catalog the member picker
//!    is served (`GET /api/ws/{workspace}/roles`).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::routes::{admin_user_router, ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(
            &format!("{}@test.local", uuid::Uuid::new_v4()),
            "U",
            &hash_password("pw").unwrap(),
        )
        .await
        .unwrap()
}

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
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

/// Send a JSON request through a built app router and return the raw response.
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

/// `GET /api/admin/users` (the Access console's principal picker) must never
/// hand back a service account, and the human rows it does return must still
/// carry `provider` — the field the picker needs to label a principal. Both
/// halves are `UserRepo::list`'s job (`WHERE provider <> 'service'` +
/// `provider` in the row), so this pins that exact behavior rather than
/// re-deriving it: a regression that dropped the filter would hand out
/// `data_api_endpoint:invoke` on every endpoint in the workspace the moment the
/// service account was clicked and set to `ws_admin` (migration 0124).
#[tokio::test]
async fn admin_users_search_excludes_service_accounts_and_surfaces_provider() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let admin = seed_user(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(admin, &st.sessions).await;

    let human_email = format!("human-{}@t.local", uuid::Uuid::new_v4().simple());
    let human = UserRepo::new(db.clone())
        .insert_local(&human_email, "Human", &hash_password("pw").unwrap())
        .await
        .unwrap();
    let sa_email = format!("svc-{}@t.local", uuid::Uuid::new_v4().simple());
    let sa = UserRepo::new(db.clone())
        .insert_service(&sa_email, "Reporting Bot")
        .await
        .unwrap();

    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st);
    let resp = send(&app, &cookie, "GET", "/api/admin/users?per_page=500", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().expect("array body");

    let human_row = rows
        .iter()
        .find(|r| r["id"] == human.0.to_string())
        .expect("the human user must be findable in the picker's search");
    assert_eq!(
        human_row["provider"], "local",
        "a returned row must still carry provider"
    );

    assert!(
        rows.iter().all(|r| r["id"] != sa.0.to_string()),
        "a service account must never appear in the member-picker's user search: \
         inviting one as ws_admin hands it data_api_endpoint:invoke on every \
         endpoint in the workspace through the Cedar workspace cascade"
    );
}

/// `data_api_invoker` carries exactly `data_api_endpoint:invoke` (migration
/// 0124) and exists to be granted OBJECT-scoped to one service account for one
/// endpoint. Offered through the workspace membership role picker
/// (`GET /api/ws/{workspace}/roles`), it would mean "invoke every endpoint in
/// this workspace" via the same cascade a `ws_admin` membership now carries —
/// so it must never appear in that catalog.
#[tokio::test]
async fn data_api_invoker_is_not_offered_as_a_membership_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let admin = seed_user(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", admin).await.unwrap();
    st.members
        .add(ws.id, "user", admin.0, "ws_admin", admin)
        .await
        .unwrap();
    let cookie = login_cookie(admin, &st.sessions).await;

    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st);
    let resp = send(
        &app,
        &cookie,
        "GET",
        &format!("/api/ws/{}/roles", ws.slug),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(
        !body.to_string().contains("data_api_invoker"),
        "data_api_invoker carries invoke on every endpoint through the workspace \
         cascade (migration 0124) and must never be offerable as a membership \
         role; it is only meant to be granted object-scoped to a service account"
    );
}
