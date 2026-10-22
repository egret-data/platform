//! DB-gated integration tests. Skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a
//! run with no `DATABASE_URL` FAILS. CI / `just up` provides Postgres.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@test.local", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn insert_and_find_user() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = UserRepo::new(db);
    let email = unique_email("user");
    let hash = hash_password("pw").unwrap();

    let id = repo.insert_local(&email, "Tester", &hash).await.unwrap();
    let found = repo
        .find_by_email(&email)
        .await
        .unwrap()
        .expect("user exists");
    assert_eq!(found.id, id);
    assert_eq!(found.provider, "local");
    assert!(found.password_hash.is_some());

    let by_id = repo.find_by_id(id).await.unwrap().expect("by id");
    assert_eq!(by_id.email.to_lowercase(), email.to_lowercase());
}

use chrono::Duration;
use egret_auth::session::SessionService;
use egret_core::UserId;

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&unique_email("sess"), "S", &hash)
        .await
        .unwrap()
}

#[tokio::test]
async fn create_then_validate_slides_idle() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));

    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().expect("valid");
    assert_eq!(principal.user_id, uid);
}

#[tokio::test]
async fn expired_session_does_not_validate() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    // idle_ttl in the past → immediately expired.
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(-1));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

#[tokio::test]
async fn revoke_invalidates() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "127.0.0.1").await.unwrap();
    svc.revoke(&issued.token).await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

/// Two-tier expiry is independent: a session past its *absolute* cap must be
/// rejected even when its idle window is still wide open (the reverse of
/// `expired_session_does_not_validate`, which pins the idle side instead).
#[tokio::test]
async fn absolute_expiry_invalidates_session_even_when_idle_fresh() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    // absolute_ttl in the past, idle_ttl generous → only the max-lifetime cap
    // should be the reason this fails.
    let svc = SessionService::new(db.clone(), Duration::minutes(-1), Duration::hours(24));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

use chrono::{DateTime, Utc};

/// A disabled user's session must stop resolving immediately, even though the
/// session row itself is still within both expiry windows — this is the
/// "central revocation deferred, egret-side revocation kept" posture: egret's
/// own `users.status` is authoritative for session validity.
#[tokio::test]
async fn disabled_user_session_is_invalid() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    // Sanity: valid before disabling.
    assert!(svc.validate(&issued.token).await.unwrap().is_some());

    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(uid.0)
        .execute(&db.pool)
        .await
        .unwrap();

    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

/// `validate` slides both `last_seen_at` and `idle_expires_at` forward — the
/// sliding half of the two-tier expiry, asserted directly against the row
/// rather than just "still resolves" (which `create_then_validate_slides_idle`
/// already covers).
#[tokio::test]
async fn validate_slides_last_seen_and_idle_expiry() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    let token_hash = egret_auth::token::hash_token(&issued.token);

    let (last_seen_before, idle_before): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("SELECT last_seen_at, idle_expires_at FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    // Postgres `now()` is the TRANSACTION timestamp, and `create` and
    // `validate` are two of them. On a loaded machine — a full `cargo test
    // --workspace`, which is when this was seen — they can land on the same
    // microsecond, and a strictly-forward assertion then fails for a reason
    // that has nothing to do with sliding. Not a weaker assertion (`>=` would
    // pass whether or not the update happens); a guaranteed gap instead.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(svc.validate(&issued.token).await.unwrap().is_some());

    let (last_seen_after, idle_after): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("SELECT last_seen_at, idle_expires_at FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    assert!(
        last_seen_after > last_seen_before,
        "last_seen_at should slide forward on validate"
    );
    assert!(
        idle_after > idle_before,
        "idle_expires_at should slide forward on validate"
    );
}

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::routes::{auth_router, AuthState};
use egret_config::Config;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn login_sets_cookies_and_me_returns_user() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let repo = UserRepo::new(db.clone());
    let email = unique_email("login");
    repo.insert_local(&email, "L", &hash_password("pw").unwrap())
        .await
        .unwrap();

    let st = AuthState {
        users: repo,
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: egret_core::ratelimit::RateLimiter::new(5, 300, 300),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    let app = axum::Router::new()
        .nest("/api/auth", auth_router())
        .with_state(st);

    // Login.
    let body = serde_json::to_vec(&serde_json::json!({"email": email, "password": "pw"})).unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie_count = resp.headers().get_all("set-cookie").iter().count();
    assert!(set_cookie_count >= 2, "expects session + csrf cookies");
    // Find the egret_session= cookie specifically (order may vary).
    let session_kv = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|s| s.starts_with("egret_session="))
        .expect("egret_session cookie present")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // /me with the session cookie.
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri("/api/auth/me")
                .header("cookie", session_kv)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["email"].as_str().unwrap().to_lowercase(),
        email.to_lowercase()
    );
}

#[tokio::test]
async fn me_without_cookie_is_401() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: egret_core::ratelimit::RateLimiter::new(5, 300, 300),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    let app = axum::Router::new()
        .nest("/api/auth", auth_router())
        .with_state(st);
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri("/api/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn platform_role_via_group_appears_in_principal() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    // user's group is assigned platform_admin
    let gid: uuid::Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(uid.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'group', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin' \
         ON CONFLICT DO NOTHING",
    )
    .bind(gid)
    .execute(&db.pool)
    .await
    .unwrap();

    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal
        .platform_roles
        .iter()
        .any(|r| r == "platform_admin"));
}

#[tokio::test]
async fn validate_populates_principal_group_ids() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let uid = seed_user(&db).await;
    let gid: uuid::Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(uid.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();

    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal.group_ids.iter().any(|g| g.0 == gid));
}

#[tokio::test]
async fn permissions_resolve_direct_group_and_empty() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));

    // Temp platform role + a single permission, unique per run so reruns don't collide.
    let role_name = format!("t_role-{}", uuid::Uuid::new_v4().simple());
    let role_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'engine:read')")
        .bind(role_id)
        .execute(&db.pool)
        .await
        .unwrap();

    // U1: role assigned directly.
    let u1 = seed_user(&db).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(u1.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let issued = svc.create(u1, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal.permissions.iter().any(|p| p == "engine:read"));

    // U2: role assigned via group membership.
    let u2 = seed_user(&db).await;
    let gid: uuid::Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(u2.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('group', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(gid)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let issued = svc.create(u2, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal.permissions.iter().any(|p| p == "engine:read"));

    // U3: no roles at all -> empty permissions.
    let u3 = seed_user(&db).await;
    let issued = svc.create(u3, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal.permissions.is_empty());
}

#[tokio::test]
async fn permissions_include_inherited_parent_role_perms() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));

    // A parent platform role holding storage:read, and a child role that inherits it
    // (its own perm is engine:read). A user granted ONLY the child role must see BOTH
    // permissions — the FE-gating closure now matches the PDP's.
    let parent: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(format!("t_parent-{}", uuid::Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'storage:read')")
        .bind(parent)
        .execute(&db.pool)
        .await
        .unwrap();
    let child: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, parent_role_id) VALUES ('platform', $1, $2) RETURNING id",
    )
    .bind(format!("t_child-{}", uuid::Uuid::new_v4().simple()))
    .bind(parent)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'engine:read')")
        .bind(child)
        .execute(&db.pool)
        .await
        .unwrap();

    let u = seed_user(&db).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(u.0)
    .bind(child)
    .execute(&db.pool)
    .await
    .unwrap();
    let issued = svc.create(u, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(
        principal.permissions.iter().any(|p| p == "engine:read"),
        "own perm"
    );
    assert!(
        principal.permissions.iter().any(|p| p == "storage:read"),
        "inherited parent perm surfaced via the hierarchy closure"
    );
}

#[tokio::test]
async fn login_is_rate_limited() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let email = unique_email("rl");
    UserRepo::new(db.clone())
        .insert_local(&email, "R", &hash_password("pw").unwrap())
        .await
        .unwrap();
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: egret_core::ratelimit::RateLimiter::new(2, 300, 300),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    let app = axum::Router::new()
        .nest("/api/auth", auth_router())
        .with_state(st);
    let bad =
        serde_json::to_vec(&serde_json::json!({"email": email, "password": "wrong"})).unwrap();
    let mk = || {
        HttpRequest::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(bad.clone()))
            .unwrap()
    };

    let s1 = app.clone().oneshot(mk()).await.unwrap().status(); // 401
    let s2 = app.clone().oneshot(mk()).await.unwrap().status(); // 401
    let s3 = app.oneshot(mk()).await.unwrap().status(); // 429 (limiter max=2)
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(s3, StatusCode::TOO_MANY_REQUESTS);
}

/// One `POST /api/auth/login` against a fresh router over `st`.
async fn post_login(
    app: &axum::Router,
    email: &str,
    password: &str,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let body =
        serde_json::to_vec(&serde_json::json!({"email": email, "password": password})).unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, headers, json)
}

fn login_app(db: &Db, limiter: egret_core::ratelimit::RateLimiter) -> axum::Router {
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: limiter,
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    axum::Router::new()
        .nest("/api/auth", auth_router())
        .with_state(st)
}

/// KI-145: the login response carries the same roles and permissions as
/// `GET /api/auth/me`, not two empty lists.
#[tokio::test]
async fn login_response_carries_the_platform_roles() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let email = unique_email("roles");
    let id = UserRepo::new(db.clone())
        .insert_local(&email, "R", &hash_password("pw").unwrap())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, r.id, 'platform' FROM roles r \
          WHERE r.scope = 'platform' AND r.name = 'data_steward'",
    )
    .bind(id.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let app = login_app(&db, egret_core::ratelimit::RateLimiter::new(5, 300, 300));
    let (status, _, json) = post_login(&app, &email, "pw").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["platform_roles"], serde_json::json!(["data_steward"]));
    assert!(
        json["permissions"]
            .as_array()
            .is_some_and(|p| p.iter().any(|v| v == "role:read")),
        "the steward's permissions, as /me lists them: {json}"
    );
}

/// KI-144: successes do not use up the sign-in budget, and a success forgives
/// the failures before it.
#[tokio::test]
async fn login_limit_counts_only_failures() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let email = unique_email("rlok");
    UserRepo::new(db.clone())
        .insert_local(&email, "R", &hash_password("pw").unwrap())
        .await
        .unwrap();
    let app = login_app(&db, egret_core::ratelimit::RateLimiter::new(5, 300, 300));

    assert_eq!(post_login(&app, &email, "pw").await.0, StatusCode::OK);
    for _ in 0..4 {
        assert_eq!(
            post_login(&app, &email, "wrong").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        post_login(&app, &email, "pw").await.0,
        StatusCode::OK,
        "1 success + 4 failures is not 5 failures"
    );
    for _ in 0..4 {
        assert_eq!(
            post_login(&app, &email, "wrong").await.0,
            StatusCode::UNAUTHORIZED,
            "the success forgave the earlier failures"
        );
    }
    assert_eq!(post_login(&app, &email, "pw").await.0, StatusCode::OK);
}

/// KI-002: the refusal says when to try again, in `Retry-After` and in the body.
#[tokio::test]
async fn a_locked_out_login_says_when_to_retry() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let email = unique_email("rlwait");
    UserRepo::new(db.clone())
        .insert_local(&email, "R", &hash_password("pw").unwrap())
        .await
        .unwrap();
    let app = login_app(&db, egret_core::ratelimit::RateLimiter::new(5, 300, 300));

    for _ in 0..5 {
        assert_eq!(
            post_login(&app, &email, "wrong").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, headers, json) = post_login(&app, &email, "wrong").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "five failures lock the sixth attempt"
    );
    assert_eq!(headers["retry-after"], "300");
    assert_eq!(json["retry_after_secs"], 300);
    assert_eq!(json["code"], "rate_limited");

    let (status, headers, _) = post_login(&app, &email, "pw").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the right password inside the lockout is still refused"
    );
    let wait: u64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((299..=300).contains(&wait), "remaining wait, got {wait}");
}
