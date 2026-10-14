//! A service account is a `users` row (Data API P2), so the human-facing user
//! routes must refuse to treat it as a person.
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Follows the harness in
//! `routes_api.rs` (same `Fixture`/`setup`/`session_for`/`req`/`into_json`
//! shape); the router here is `control_plane_router()` UNNESTED, so routes
//! are relative (`/users/{id}`, not `/api/control-plane/users/{id}`).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState};
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
}

struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);
    let (audit, _audit_rx) = egret_audit::AuditSink::new(16);

    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        browser: None,
        pdp: std::sync::Arc::new(AllowAllPdp),
        audit,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: std::sync::Arc::new(NoListing),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: std::sync::Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: std::sync::Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

/// Create a user (optionally a platform_admin) and return its session token.
/// Mirrors `routes_api.rs::session_for`.
async fn session_for(f: &Fixture, tag: &str, platform_admin: bool) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    if platform_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("grant platform_admin");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@acme.service.egret.local", Uuid::new_v4())
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

#[tokio::test]
async fn password_reset_rejects_a_service_account() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "svcsurf_admin", true).await;
    let svc = f
        .users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .expect("insert service account");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{}/password", svc.0),
            &admin_token,
            Some(serde_json::json!({ "password": "hunter22!" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a service account has no password to reset; body={j}"
    );
    // `reset_password` also has a pre-existing `provider != "local"` check
    // (written for SSO/OIDC accounts) that independently 400s a service
    // account with a DIFFERENT message ("non-local (SSO) account"). Asserting
    // only the status code would pass even if `refuse_service_account` were
    // removed entirely. Pin the message so this test actually exercises the
    // new guard, not the SSO fallback.
    let msg = j["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("service account"),
        "expected the service-account guard's message, got: {msg}"
    );
}

#[tokio::test]
async fn profile_update_rejects_a_service_account() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "svcsurf_admin", true).await;
    let svc = f
        .users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .expect("insert service account");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{}", svc.0),
            &admin_token,
            Some(serde_json::json!({ "name": "Person" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a service account is not editable as a user; body={j}"
    );
}

/// Disabling a service account through `PATCH /users/{id}` must WORK.
///
/// Spec §6.1 names `users.status <> 'active'` as the way to stop a compromised
/// service account inside the 600s token window, and `ClientStore::verify`
/// honours it — but this route is the only thing that can set it. Refusing the
/// whole PATCH for a service account (rather than just its profile fields) would
/// delete the documented kill switch while leaving the spec claiming it exists.
///
/// Asserted against the DATABASE, not just the status code: a 200 that changed
/// nothing would look identical from the response.
#[tokio::test]
async fn a_service_account_can_be_disabled_through_the_users_route() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "svcsurf_admin", true).await;
    let svc = f
        .users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .expect("insert service account");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/users/{}", svc.0),
            &admin_token,
            Some(serde_json::json!({ "status": "disabled" })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "disabling a service account is the documented kill switch; body={j}"
    );

    let stored: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(svc.0)
        .fetch_one(&f.db.pool)
        .await
        .expect("read status");
    assert_eq!(
        stored, "disabled",
        "the kill switch must actually have been thrown, not merely acknowledged"
    );
}

#[tokio::test]
async fn service_account_is_absent_from_the_users_route() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "svcsurf_admin", true).await;
    let svc = f
        .users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .expect("insert service account");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/users", &admin_token, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&svc.0.to_string().as_str()),
        "GET /users leaked a service account"
    );
}

/// These tests never reach the materialized-view report; a lister that is asked
/// anyway fails loudly instead of reporting an empty engine.
struct NoListing;

#[async_trait::async_trait]
impl egret_controlplane::MaterializedViewLister for NoListing {
    async fn list(
        &self,
        _engine: &egret_controlplane::ExposureEngine,
    ) -> Result<Vec<egret_controlplane::MvRow>, String> {
        Err("no StarRocks in this test".to_string())
    }
}
