//! Service-account credentials across both audiences (SA2): list, mint, rotate,
//! revoke, expiry, and the workspace-consistency guard.
//!
//! DB-gated; fails without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP`. The router
//! is `control_plane_router()` UNNESTED, so routes are relative.

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
        catalog_lister: std::sync::Arc::new(egret_controlplane::UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: Some(90),
        storage_scope: egret_controlplane::StorageScope::s3(
            egret_controlplane::SystemPrefixes::default(),
        ),
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

async fn call(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    into_json(
        f.app
            .clone()
            .oneshot(req(method, uri, token, body))
            .await
            .unwrap(),
    )
    .await
}

async fn create_platform_sa(f: &Fixture, token: &str) -> Uuid {
    let (s, j) = call(
        f,
        "POST",
        "/service-accounts",
        token,
        Some(serde_json::json!({
            "scope": "platform",
            "name": format!("bot {}", Uuid::new_v4().simple()),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    j["user_id"].as_str().unwrap().parse().unwrap()
}

async fn workspace_sa(f: &Fixture, token: &str, ws: Uuid) -> Uuid {
    let (s, j) = call(
        f,
        "POST",
        "/service-accounts",
        token,
        Some(serde_json::json!({
            "scope": "workspace", "workspace_id": ws,
            "name": format!("bot {}", Uuid::new_v4().simple()),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    j["user_id"].as_str().unwrap().parse().unwrap()
}

/// A warehouse (in its own fresh workspace) with one external catalog role.
async fn warehouse_with_role(f: &Fixture) -> (Uuid, Uuid, Uuid) {
    let wh = egret_test_db::seed_warehouse(&f.db.pool).await;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id = $1")
        .bind(wh)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
         VALUES ($1, 'r', 'external') RETURNING id",
    )
    .bind(wh)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (ws, wh, role)
}

fn catalog_body(wh: Uuid, role: Uuid) -> serde_json::Value {
    serde_json::json!({
        "audience": "catalog",
        "catalog": { "warehouse_id": wh, "catalog_role_id": role, "consumer_type": "external" },
    })
}

fn store(f: &Fixture) -> egret_catalog_engine::CatalogStore {
    egret_catalog_engine::CatalogStore::new(f.db.clone())
}

async fn verifies(f: &Fixture, j: &serde_json::Value) -> bool {
    store(f)
        .verify_client(
            j["client_id"].as_str().unwrap(),
            j["client_secret"].as_str().unwrap(),
        )
        .await
        .expect("verify")
        .is_some()
}

#[tokio::test]
async fn list_shows_both_audiences_with_derived_status() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "cred_admin", true).await;
    let (ws, wh, role) = warehouse_with_role(&f).await;
    let sa = workspace_sa(&f, &t, ws).await;
    let base = format!("/service-accounts/{sa}/credentials");

    let (s, d) = call(
        &f,
        "POST",
        &base,
        &t,
        Some(serde_json::json!({"audience": "data-api", "name": "da"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{d}");
    assert!(d["client_secret"].is_string());
    let (s, c) = call(&f, "POST", &base, &t, Some(catalog_body(wh, role))).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    assert_eq!(c["catalog"]["warehouse_id"], wh.to_string());
    assert_eq!(c["catalog"]["catalog_role_id"], role.to_string());
    assert_eq!(c["service_account_id"], sa.to_string());

    // Revoke the data-api one; expire the catalog one in the past.
    let did = d["id"].as_str().unwrap();
    let cid = c["id"].as_str().unwrap();
    let (s, _) = call(&f, "POST", &format!("{base}/{did}/revoke"), &t, None).await;
    assert_eq!(s, StatusCode::OK);
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let (s, _) = call(
        &f,
        "POST",
        &format!("{base}/{cid}/expiry"),
        &t,
        Some(serde_json::json!({ "expires_at": past })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // A third, untouched, active catalog credential.
    let (_, a) = call(&f, "POST", &base, &t, Some(catalog_body(wh, role))).await;

    let (s, list) = call(&f, "GET", &base, &t, None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    let find = |id: &serde_json::Value| arr.iter().find(|r| r["id"] == *id).unwrap();
    assert_eq!(find(&d["id"])["audience"], "data-api");
    assert_eq!(find(&d["id"])["status"], "revoked");
    assert_eq!(find(&c["id"])["audience"], "catalog");
    assert_eq!(find(&c["id"])["status"], "expired");
    assert_eq!(find(&a["id"])["status"], "active");
    assert!(arr.iter().all(|r| r.get("client_secret").is_none()));
}

#[tokio::test]
async fn catalog_credential_in_same_workspace_verifies_and_other_workspace_is_refused() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "cred_admin", true).await;
    let (ws, wh, role) = warehouse_with_role(&f).await;
    let (_, other_wh, other_role) = warehouse_with_role(&f).await;
    let sa = workspace_sa(&f, &t, ws).await;
    let base = format!("/service-accounts/{sa}/credentials");

    let (s, c) = call(&f, "POST", &base, &t, Some(catalog_body(wh, role))).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    assert!(verifies(&f, &c).await, "the new client passes the verifier");

    let (s, j) = call(
        &f,
        "POST",
        &base,
        &t,
        Some(catalog_body(other_wh, other_role)),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{j}");
    let (_, list) = call(&f, "GET", &base, &t, None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "nothing was minted");

    // A role from another warehouse is refused too.
    let (s, _) = call(&f, "POST", &base, &t, Some(catalog_body(wh, other_role))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A platform service account may mint in any workspace.
    let psa = create_platform_sa(&f, &t).await;
    let pbase = format!("/service-accounts/{psa}/credentials");
    let (s, p) = call(
        &f,
        "POST",
        &pbase,
        &t,
        Some(catalog_body(other_wh, other_role)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert!(verifies(&f, &p).await);
    // ...but cannot hold a data-api credential (the Data API is workspace-only).
    let (s, _) = call(
        &f,
        "POST",
        &pbase,
        &t,
        Some(serde_json::json!({"audience": "data-api"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rotate_revoke_and_expiry_reach_the_verifier() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "cred_admin", true).await;
    let (ws, wh, role) = warehouse_with_role(&f).await;
    let sa = workspace_sa(&f, &t, ws).await;
    let base = format!("/service-accounts/{sa}/credentials");

    // Rotate: new secret verifies, old no longer does and carries revoked_at.
    let (_, old) = call(&f, "POST", &base, &t, Some(catalog_body(wh, role))).await;
    assert!(verifies(&f, &old).await);
    let oid = old["id"].as_str().unwrap();
    let (s, new) = call(&f, "POST", &format!("{base}/{oid}/rotate"), &t, None).await;
    assert_eq!(s, StatusCode::OK, "{new}");
    assert_ne!(new["client_secret"], old["client_secret"]);
    assert_ne!(new["id"], old["id"]);
    assert_eq!(new["catalog"], old["catalog"]);
    assert!(verifies(&f, &new).await);
    assert!(
        !verifies(&f, &old).await,
        "the rotated-out client is refused"
    );
    let revoked_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM catalog.catalog_clients WHERE id = $1")
            .bind(oid.parse::<Uuid>().unwrap())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert!(revoked_at.is_some());
    // Rotating a revoked credential is a conflict.
    let (s, _) = call(&f, "POST", &format!("{base}/{oid}/rotate"), &t, None).await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Revoke makes the verifier refuse.
    let nid = new["id"].as_str().unwrap();
    let (s, r) = call(&f, "POST", &format!("{base}/{nid}/revoke"), &t, None).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "revoked");
    assert!(!verifies(&f, &new).await);

    // Expiry in the past makes the verifier refuse; clearing it restores it.
    let (_, e) = call(&f, "POST", &base, &t, Some(catalog_body(wh, role))).await;
    let eid = e["id"].as_str().unwrap();
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let (s, x) = call(
        &f,
        "POST",
        &format!("{base}/{eid}/expiry"),
        &t,
        Some(serde_json::json!({ "expires_at": past })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{x}");
    assert_eq!(x["status"], "expired");
    assert!(!verifies(&f, &e).await);
    let (_, x) = call(
        &f,
        "POST",
        &format!("{base}/{eid}/expiry"),
        &t,
        Some(serde_json::json!({ "expires_at": null })),
    )
    .await;
    assert_eq!(x["status"], "active");
    assert!(verifies(&f, &e).await);

    // A credential of another account is not reachable through this path.
    let other = workspace_sa(&f, &t, ws).await;
    let (s, _) = call(
        &f,
        "POST",
        &format!("/service-accounts/{other}/credentials/{eid}/revoke"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
