//! KI-284: `PUT /api/admin/users/{id}/platform-roles` cannot remove the last
//! active platform admin. DB-gated; self-skips without `DATABASE_URL` ONLY
//! under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Its own test binary, so its own database (`egret_test_db`: one per test
//! process) and a single test: the guard counts the GLOBAL set of active
//! platform admins, which the other workspace tests, running concurrently in
//! one process, would never leave at exactly one.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::routes::{admin_user_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
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

async fn platform_roles_of(db: &Db, user: UserId) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
          WHERE ag.principal_type = 'user' AND ag.principal_id = $1 \
            AND ag.scope_type = 'platform' ORDER BY r.name",
    )
    .bind(user.0)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// The sole active platform admin replacing their own platform roles with a
/// set that leaves out `platform_admin` is refused with the last-admin 409,
/// and their roles are unchanged. Once a second admin exists the same request
/// goes through.
#[tokio::test]
async fn replacing_platform_roles_cannot_remove_the_last_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    // Arrange: this database's only platform admin.
    let platform_admin_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
          WHERE r.name = 'platform_admin' AND ag.scope_type = 'platform'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(platform_admin_grants, 0, "a fresh database holds no admin");
    let admin = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = format!(
        "egret_session={}",
        st.sessions.create(admin, "a", "").await.unwrap().token
    );
    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st.clone());
    let put = |roles: serde_json::Value| {
        HttpRequest::builder()
            .method("PUT")
            .uri(format!("/api/admin/users/{}/platform-roles", admin.0))
            .header("cookie", cookie.clone())
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "roles": roles })).unwrap(),
            ))
            .unwrap()
    };

    // Act: drop platform_admin for an unrelated role.
    let resp = app
        .clone()
        .oneshot(put(serde_json::json!(["data_steward"])))
        .await
        .unwrap();

    // Assert: refused, nothing changed.
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert!(body.contains("last active platform admin"), "body={body}");
    assert_eq!(platform_roles_of(&db, admin).await, vec!["platform_admin"]);

    // A second admin breaks the tie: the same replace now goes through.
    let second = seed_user(&db).await;
    st.authz
        .replace_platform_roles(second, &["platform_admin".into()])
        .await
        .unwrap();
    let resp = app
        .oneshot(put(serde_json::json!(["data_steward"])))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(platform_roles_of(&db, admin).await, vec!["data_steward"]);
}
