//! DB-gated integration tests. Skip (pass) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
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
        eprintln!("DATABASE_URL unset — skipping");
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
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(5, 300, 300),
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
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(5, 300, 300),
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
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         SELECT 'group', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
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
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(2, 300, 300),
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
