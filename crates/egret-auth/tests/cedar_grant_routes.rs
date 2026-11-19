//! EO4 operator endpoints for the cedar grant-source switch, driven over the real
//! `auth_router` (login) + `cedar_grant_router`. DB-gated via egret_test_db.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_auth::cedar_grant_routes::{cedar_grant_router, CedarGrantState};
use egret_auth::password::hash_password;
use egret_auth::routes::{auth_router, AuthState};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_config::Config;
use egret_core::UserId;
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Serialises tests that mutate the global switch row.
static SWITCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db() -> Db {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

async fn seed_local(db: &Db, tag: &str) -> (UserId, String) {
    let email = format!("{tag}-{}@egret.test", uuid::Uuid::new_v4().simple());
    let id = UserRepo::new(db.clone())
        .insert_local(&email, "T", &hash_password("pw").unwrap())
        .await
        .unwrap();
    (id, email)
}

async fn grant_platform_admin(db: &Db, uid: UserId) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
         WHERE scope = 'platform' AND name = 'platform_admin' \
         ON CONFLICT DO NOTHING",
    )
    .bind(uid.0)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn app(db: &Db) -> axum::Router {
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let auth = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: sessions.clone(),
        config: Config::default(),
        rate_limiter: egret_core::ratelimit::RateLimiter::new(50, 300, 300),
        audit: egret_audit::AuditSink::new(16).0,
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: None,
        cookie_key: None,
    };
    let cg = CedarGrantState {
        db: db.clone(),
        sessions,
        bus: egret_events::EventBus::new(),
    };
    axum::Router::new()
        .nest("/api/auth", auth_router().with_state(auth))
        .nest("/api/control-plane", cedar_grant_router().with_state(cg))
}

async fn login(app: &axum::Router, email: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({"email": email, "password": "pw"})).unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login should succeed");
    let mut kvs = vec![];
    for v in resp.headers().get_all("set-cookie").iter() {
        let kv = v.to_str().unwrap().split(';').next().unwrap().to_string();
        if kv.starts_with("egret_session=") || kv.starts_with("egret_csrf=") {
            kvs.push(kv);
        }
    }
    kvs.join("; ")
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/control-plane/perm/cedar-grants{path}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn cedar_grant_endpoints_gate_and_step() {
    let _guard = SWITCH_LOCK.lock().await;
    let db = db().await;
    sqlx::query("DELETE FROM perm.cedar_grant_source")
        .execute(&db.pool)
        .await
        .unwrap();
    let app = app(&db);

    let (admin, admin_email) = seed_local(&db, "admin").await;
    grant_platform_admin(&db, admin).await;
    let (_user, user_email) = seed_local(&db, "plain").await;
    let admin_c = login(&app, &admin_email).await;
    let user_c = login(&app, &user_email).await;

    for (m, p) in [
        ("GET", ""),
        ("POST", "/report"),
        ("POST", "/fold"),
        ("POST", "/switch"),
        ("POST", "/undo"),
    ] {
        let (s, _) = call(&app, &user_c, m, p).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {p} as non-admin");
    }

    let (s, j) = call(&app, &admin_c, "GET", "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j["source"], "legacy");

    // Fold populates the mirror and stays legacy.
    let (s, j) = call(&app, &admin_c, "POST", "/fold").await;
    assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("legacy")));
    let mirrored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.cedar_grants m JOIN access_grants a ON a.id = m.id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(mirrored > 0, "fold populated the mirror");

    // Report is read-only.
    let (s, rep) = call(&app, &admin_c, "POST", "/report").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(rep, serde_json::json!([]), "faithful mirror reports clean");

    // Faithful mirror: switch succeeds.
    let (s, j) = call(&app, &admin_c, "POST", "/switch").await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["source"], "unified");
    assert_eq!(
        j["switched_by"].as_str().unwrap().to_lowercase(),
        admin_email.to_lowercase()
    );

    let (s, j) = call(&app, &admin_c, "POST", "/undo").await;
    assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("legacy")));

    // A mirror-only ghost row refuses the switch.
    let ghost = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO perm.cedar_grants (id, principal_type, principal_id, role_id, scope_type, effect) \
         SELECT $1, 'user', $2, id, 'platform', 'allow' FROM roles \
         WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(ghost)
    .bind(uuid::Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();
    let (s, _) = call(&app, &admin_c, "POST", "/switch").await;
    let _ = sqlx::query("DELETE FROM perm.cedar_grants WHERE id = $1")
        .bind(ghost)
        .execute(&db.pool)
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "ghost divergence refuses switch");

    // Idempotent undo at legacy.
    let (s, j) = call(&app, &admin_c, "POST", "/undo").await;
    assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("legacy")));
}
