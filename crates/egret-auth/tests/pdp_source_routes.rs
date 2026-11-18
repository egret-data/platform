//! EO3 operator endpoints for the PDP policy-source switch, driven over the real
//! `auth_router` (login) + `pdp_source_router`. DB-gated via egret_test_db.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::pdp_source_routes::{pdp_source_router, PdpSourceState};
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
    let pdp = PdpSourceState {
        db: db.clone(),
        sessions,
        bus: egret_events::EventBus::new(),
    };
    axum::Router::new()
        .nest("/api/auth", auth_router().with_state(auth))
        .nest("/api/control-plane", pdp_source_router().with_state(pdp))
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
                .uri(format!("/api/control-plane/perm/pdp-source{path}"))
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
async fn pdp_source_endpoints_gate_and_step() {
    let _guard = SWITCH_LOCK.lock().await;
    let db = db().await;
    sqlx::query("DELETE FROM perm.pdp_policy_source")
        .execute(&db.pool)
        .await
        .unwrap();
    let app = app(&db);

    let (admin, admin_email) = seed_local(&db, "admin").await;
    grant_platform_admin(&db, admin).await;
    let (_user, user_email) = seed_local(&db, "plain").await;
    let admin_c = login(&app, &admin_email).await;
    let user_c = login(&app, &user_email).await;

    // Non-admin: 403 on read and on every mutation.
    for (m, p) in [
        ("GET", ""),
        ("POST", "/report"),
        ("POST", "/shadow"),
        ("POST", "/switch"),
        ("POST", "/undo"),
    ] {
        let (s, _) = call(&app, &user_c, m, p).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {p} as non-admin");
    }

    // Admin: default legacy.
    let (s, j) = call(&app, &admin_c, "GET", "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j["source"], "legacy");

    // Switch from legacy is refused.
    let (s, _) = call(&app, &admin_c, "POST", "/switch").await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Report is read-only and does not change state.
    let (s, rep) = call(&app, &admin_c, "POST", "/report").await;
    assert_eq!(s, StatusCode::OK);
    assert!(rep.is_array());
    let (_, j) = call(&app, &admin_c, "GET", "").await;
    assert_eq!(j["source"], "legacy");

    // legacy -> shadow (+ audit), idempotent.
    for _ in 0..2 {
        let (s, j) = call(&app, &admin_c, "POST", "/shadow").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(j["source"], "shadow");
        assert_eq!(
            j["switched_by"].as_str().unwrap().to_lowercase(),
            admin_email.to_lowercase()
        );
    }
    let (_, j) = call(&app, &admin_c, "GET", "").await;
    assert_eq!(j["source"], "shadow");

    // shadow -> compiled iff the live report is clean.
    let (s, j) = call(&app, &admin_c, "POST", "/switch").await;
    if rep.as_array().unwrap().is_empty() {
        assert_eq!(s, StatusCode::OK, "{j}");
        assert_eq!(j["source"], "compiled");

        let (s, j) = call(&app, &admin_c, "POST", "/undo").await;
        assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("shadow")));
    } else {
        assert_eq!(
            s,
            StatusCode::CONFLICT,
            "divergent report refuses the switch"
        );
    }
    let (s, j) = call(&app, &admin_c, "POST", "/undo").await;
    assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("legacy")));
    // Idempotent at legacy.
    let (s, j) = call(&app, &admin_c, "POST", "/undo").await;
    assert_eq!((s, j["source"].as_str()), (StatusCode::OK, Some("legacy")));
}
