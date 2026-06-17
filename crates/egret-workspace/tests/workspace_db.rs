//! DB-gated integration tests; self-skip without DATABASE_URL.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_config::Config;
use egret_core::{Pagination, UserId};
use egret_metastore::Db;
use egret_workspace::routes::{
    admin_user_router, admin_workspace_router, ws_member_router, WorkspaceState,
};
use egret_workspace::workspaces::WorkspaceRepo;
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
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn create_get_and_unique_slug() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db.clone());
    let slug = uniq("ws");

    let ws = repo.create(&slug, "Sales", owner).await.unwrap();
    assert_eq!(ws.slug, slug);
    assert_eq!(ws.status, "active");

    let got = repo.get_by_slug(&slug).await.unwrap().expect("exists");
    assert_eq!(got.id, ws.id);

    // duplicate slug → Conflict
    let dup = repo.create(&slug, "Other", owner).await;
    assert!(matches!(dup, Err(egret_core::Error::Conflict(_))));
}

#[tokio::test]
async fn invalid_slug_rejected() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let err = repo.create("Bad Slug", "x", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}

#[tokio::test]
async fn archive_hides_from_active_lookups() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let ws = repo.create(&uniq("ws"), "Temp", owner).await.unwrap();
    repo.archive(ws.id).await.unwrap();
    let got = repo.get_by_id(ws.id).await.unwrap().unwrap();
    assert_eq!(got.status, "archived");
    let _ = Pagination::default();
}

use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;

#[tokio::test]
async fn add_list_remove_member() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let member = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    mrepo
        .add(ws.id, "user", member.0, "ws_editor", owner)
        .await
        .unwrap();
    let members = mrepo.list(ws.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, "ws_editor");

    // upsert role
    mrepo
        .add(ws.id, "user", member.0, "ws_admin", owner)
        .await
        .unwrap();
    assert_eq!(mrepo.list(ws.id).await.unwrap()[0].role, "ws_admin");

    mrepo.remove(ws.id, "user", member.0).await.unwrap();
    assert!(mrepo.list(ws.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_unknown_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db);
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let err = mrepo.add(ws.id, "user", owner.0, "superuser", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}

async fn make_group(db: &Db, name: &str) -> uuid::Uuid {
    sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn workspace_permission_via_group_is_unioned() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let user = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let authz = Authorizer::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    // user is NOT a direct member; their GROUP is ws_admin.
    let gid = make_group(&db, &uniq("grp")).await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    mrepo
        .add(ws.id, "group", gid, "ws_admin", owner)
        .await
        .unwrap();

    let perms = authz.workspace_permissions(user, ws.id).await.unwrap();
    assert!(
        perms.contains("workspace:manage_members"),
        "group role must grant the perm"
    );
}

#[tokio::test]
async fn replace_platform_roles_and_check() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let user = seed_user(&db).await;
    let authz = Authorizer::new(db.clone());
    authz
        .replace_platform_roles(user, &["data_steward".into()])
        .await
        .unwrap();
    let perms = authz.platform_permissions(user).await.unwrap();
    assert!(perms.contains("user:read"));
    assert!(!perms.contains("workspace:create")); // data_steward lacks it
                                                  // bad role rejected
    assert!(authz
        .replace_platform_roles(user, &["root".into()])
        .await
        .is_err());
}

async fn login_cookie(db: &Db, user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    let _ = db;
    format!("egret_session={}", issued.token)
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: egret_workspace::members::MemberRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
    }
}

#[tokio::test]
async fn create_workspace_requires_platform_permission() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let plain = seed_user(&db).await; // no platform role
    let st = ws_state(&db).await;
    let cookie = login_cookie(&db, plain, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_workspace_router())
        .with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn platform_admin_can_create_workspace() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_workspace_router())
        .with_state(st.clone());

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(1),
            chrono::Duration::minutes(1),
        ),
        config: Config::default(),
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(5, 300, 300),
    };
}

// ── Task 8: Admin user routes ─────────────────────────────────────────────────

#[tokio::test]
async fn assign_platform_role_then_user_gains_access() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"roles": ["data_steward"]})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("PUT")
                .uri(format!("/api/admin/users/{}/platform-roles", target.0))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Task 9: Workspace member routes ──────────────────────────────────────────

#[tokio::test]
async fn member_routes_enforce_membership() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let admin_user = seed_user(&db).await;
    let outsider = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members
        .add(ws.id, "user", admin_user.0, "ws_admin", owner)
        .await
        .unwrap();
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());

    // ws_admin can list members
    let cookie = login_cookie(&db, admin_user, &st.sessions).await;
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // outsider gets 403
    let cookie2 = login_cookie(&db, outsider, &st.sessions).await;
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn archived_workspace_is_404() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members
        .add(ws.id, "user", owner.0, "ws_admin", owner)
        .await
        .unwrap();
    st.workspaces.archive(ws.id).await.unwrap();
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());
    let cookie = login_cookie(&db, owner, &st.sessions).await;
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
