//! Lazy warehouse provisioning + effective-default storage read (Workspace Admin
//! track). Proves a workspace that predates catalog enablement (NO
//! `workspace_warehouse` mapping, NO custom `workspace_storage` row) becomes
//! usable on first catalog/storage access:
//!
//!   - `GET /api/ws/{ws}/catalog/storage` (admin) → 200, AND afterwards
//!     `warehouse_for_workspace` is `Some` (lazily provisioned); the body is the
//!     EFFECTIVE global-default config with NO secret value, `has_secret`
//!     reflecting the default's non-empty secret_key.
//!   - a second GET is idempotent (same warehouse id).
//!   - the catalog `summary` for that workspace → 200 (provisioned), not an error.
//!   - `PUT` on a separate never-visited workspace → provisions + re-points → 200.
//!   - a non-admin (`ws_viewer`) → 403.
//!
//! DB + MinIO gated; self-skips without `DATABASE_URL`. Needs MinIO reachable at
//! `localhost:9000` with a writable `warehouse` bucket (the egret dev stack ships
//! it) because provisioning runs a live `validate_access` S3 round-trip.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test catalog_provision_storage_api

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};
use egret_storage::StorageConfig;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

struct Fixture {
    app: axum::Router,
    store: CatalogStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    owner: UserId,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Build the fixture WITHOUT pre-provisioning any warehouse — every workspace a
/// test creates starts warehouse-less (predates catalog enablement).
async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap");

    let store = CatalogStore::new(db.clone());
    let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = SecretStore::new(db.clone(), Some(crypto));
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("lpowner-{suffix}@egret.test"), "LP Owner", "x")
        .await
        .expect("seed owner");

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            secret_store.clone(),
        ),
        secret_store,
        default_storage: dev_storage_config(),
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        store,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        db,
        owner,
    })
}

/// Create a warehouse-LESS workspace, returning its `(id, slug)`.
async fn seed_warehouseless_ws(f: &Fixture) -> (WorkspaceId, String) {
    let suffix = Uuid::now_v7().simple().to_string();
    let slug = format!("lpws{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Lazy Provision Test", f.owner)
        .await
        .expect("seed workspace");
    assert!(
        f.store
            .warehouse_for_workspace(ws.id.0)
            .await
            .expect("read mapping")
            .is_none(),
        "precondition: workspace starts with NO warehouse"
    );
    (ws.id, slug)
}

/// Add a new user as a workspace member with `role`, returning a session token.
async fn add_member(f: &Fixture, ws: WorkspaceId, role: &str) -> String {
    let email = format!("m_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(ws, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

fn get(uri: &str, token: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("GET")
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap()
}

fn put(uri: &str, token: &str, body: serde_json::Value) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("PUT")
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn get_storage_lazily_provisions_and_returns_effective_default() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws_id, slug) = seed_warehouseless_ws(&f).await;
    let token = add_member(&f, ws_id, "ws_admin").await;
    let uri = format!("/ws/{slug}/catalog/storage");

    // --- GET on a warehouse-less workspace: lazily provisions + returns the
    //     effective DEFAULT config (no custom row), with NO secret value. ---
    let resp = f.app.clone().oneshot(get(&uri, &token)).await.unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET storage lazily provisions → 200; body={json}"
    );
    // Effective default config.
    assert_eq!(json["provider"], serde_json::json!("s3"));
    assert_eq!(json["bucket"], serde_json::json!("warehouse"));
    assert_eq!(json["region"], serde_json::json!("local-01"));
    assert_eq!(json["access_key"], serde_json::json!("egret"));
    assert_eq!(
        json["key_prefix"],
        serde_json::json!(format!("workspaces/{slug}/warehouse")),
        "default falls back to the canonical warehouse key prefix; body={json}"
    );
    // The default's secret_key is non-empty → has_secret true, but NO value.
    assert_eq!(json["has_secret"], serde_json::json!(true));
    assert!(
        json.get("secret_key").is_none() && json.get("secret").is_none(),
        "GET storage must never return a secret value; body={json}"
    );
    assert!(
        !json.to_string().contains("egretsecret"),
        "the plaintext default secret must never appear in the body; body={json}"
    );

    // The warehouse is now provisioned.
    let wh1 = f
        .store
        .warehouse_for_workspace(ws_id.0)
        .await
        .expect("read mapping")
        .expect("warehouse lazily provisioned by GET");

    // --- A second GET is idempotent (same warehouse id, still 200). ---
    let resp = f.app.clone().oneshot(get(&uri, &token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "second GET is idempotent");
    let wh2 = f
        .store
        .warehouse_for_workspace(ws_id.0)
        .await
        .expect("read mapping")
        .expect("warehouse still present");
    assert_eq!(
        wh1, wh2,
        "lazy provisioning is idempotent (same warehouse id)"
    );

    // --- The catalog summary for that workspace → 200 (provisioned). ---
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{slug}/catalog"), &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "catalog summary on a (now) provisioned workspace → 200; body={json}"
    );
    assert_eq!(
        json["id"],
        serde_json::json!(wh1.to_string()),
        "summary reports the lazily-provisioned warehouse id; body={json}"
    );
}

#[tokio::test]
async fn summary_lazily_provisions_a_warehouseless_workspace() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws_id, slug) = seed_warehouseless_ws(&f).await;
    let token = add_member(&f, ws_id, "ws_admin").await;

    // Summary itself provisions (it only needs `catalog:read`, which ws_admin has).
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{slug}/catalog"), &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "catalog summary self-provisions a warehouse-less workspace → 200; body={json}"
    );
    assert!(
        f.store
            .warehouse_for_workspace(ws_id.0)
            .await
            .expect("read mapping")
            .is_some(),
        "summary lazily provisioned the warehouse"
    );
}

#[tokio::test]
async fn put_on_warehouseless_workspace_provisions_and_repoints() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws_id, slug) = seed_warehouseless_ws(&f).await;
    let token = add_member(&f, ws_id, "ws_admin").await;
    let uri = format!("/ws/{slug}/catalog/storage");

    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": "egretsecret",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PUT on a never-visited workspace provisions + re-points → 200; body={json}"
    );
    assert_eq!(json["region"], serde_json::json!("us-east-1"));
    assert!(
        f.store
            .warehouse_for_workspace(ws_id.0)
            .await
            .expect("read mapping")
            .is_some(),
        "PUT lazily provisioned the warehouse"
    );
}

#[tokio::test]
async fn get_storage_non_admin_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws_id, slug) = seed_warehouseless_ws(&f).await;
    let token = add_member(&f, ws_id, "ws_viewer").await;

    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{slug}/catalog/storage"), &token))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not read workspace storage"
    );
    // And it must NOT have provisioned a warehouse as a side effect (authz first).
    assert!(
        f.store
            .warehouse_for_workspace(ws_id.0)
            .await
            .expect("read mapping")
            .is_none(),
        "a forbidden GET must not provision a warehouse"
    );
}
