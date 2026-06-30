//! DB + MinIO gated SECURITY test for the CE-2 T5 front-auth + ownership
//! middleware that fronts the embedded lakekeeper Iceberg REST engine.
//!
//! Drives the LIVE mounted router (`mount_catalog_engine` — the same wiring
//! `egret-bin` uses in prod) with real egret `catalog_tokens` and asserts the
//! A-3 per-workspace token-ownership boundary holds end-to-end:
//!
//!   * token A on workspace A's `{prefix}` (= A's warehouse UUID): create
//!     namespace + table → 2xx (the request reaches lakekeeper and round-trips).
//!   * token A on workspace B's `{prefix}` → **403 FORBIDDEN** (cross-tenant
//!     guard: the token resolves to workspace A but the path names B's
//!     warehouse).
//!   * unknown / garbage token → **401 UNAUTHORIZED**.
//!   * `/config` with a valid token (no `{prefix}`) → allowed (no ownership
//!     check), discovers the caller's warehouse UUID.
//!
//! Two workspaces are seeded and BOTH get real lakekeeper warehouses provisioned
//! (live MinIO `validate_access`), so the prefixes are genuine warehouse UUIDs in
//! lakekeeper's catalog — the 403 is the egret middleware refusing, not
//! lakekeeper 404ing on an unknown warehouse.
//!
//! Needs MinIO reachable at `localhost:9000` with a writable `warehouse` bucket
//! (the egret dev stack ships it). Self-skips when `DATABASE_URL` is unset.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test front_auth

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use egret_auth::users::UserRepo;
use egret_catalog::service::tokens::Grant;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    build_state, ensure_bootstrapped, mount_catalog_engine, provision_workspace_warehouse,
    run_lakekeeper_migrations,
};
use egret_metastore::Db;
use egret_storage::StorageConfig;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

/// A single all-namespaces grant (token scope v2 equivalent of the old empty
/// `namespace_scope` = "all namespaces") with the given full perm strings.
fn star_scope(perms: &[&str]) -> Vec<Grant> {
    vec![Grant {
        namespace: "*".to_string(),
        table: None,
        permissions: perms.iter().map(|p| p.to_string()).collect(),
    }]
}

/// A single whole-namespace grant for `ns` with the given full perm strings.
fn ns_scope(ns: &str, perms: &[&str]) -> Vec<Grant> {
    vec![Grant {
        namespace: ns.to_string(),
        table: None,
        permissions: perms.iter().map(|p| p.to_string()).collect(),
    }]
}

/// Dev MinIO config matching the egret dev stack (bucket `warehouse`,
/// egret/egretsecret, S3-compat at localhost:9000).
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

/// Collect a response body into JSON, surfacing the status + raw body on failure.
async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Seed a fresh owner + workspace through the egret-side (sqlx-0.8) repos and
/// provision its lakekeeper warehouse; return
/// `(workspace_id, slug, warehouse_uuid, warehouse_row_id)`. The last element is
/// the egret warehouse-ROW id a token is now minted FOR (its `lakekeeper_id`
/// resolves back to `warehouse_uuid`).
async fn seed_workspace_with_warehouse(
    db: &Db,
    store: &CatalogStore,
    engine_state: &lakekeeper::api::ApiContext<egret_catalog_engine::EngineState>,
    storage_cfg: &StorageConfig,
    tag: &str,
) -> (Uuid, String, Uuid, Uuid) {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("{tag}-{suffix}@egret.test"), "T5 Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("{tag}{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "T5 Front-Auth Test", owner)
        .await
        .expect("seed workspace row");
    let workspace_id: Uuid = ws.id.0;

    let warehouse = provision_workspace_warehouse(
        engine_state.clone(),
        store,
        workspace_id,
        &slug,
        storage_cfg,
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    let wid = store
        .get_warehouse_by_name(workspace_id, "default")
        .await
        .expect("default warehouse row")
        .id;

    (workspace_id, slug, warehouse, wid)
}

#[tokio::test]
async fn front_auth_enforces_per_workspace_ownership() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping front_auth ownership test");
        return;
    };

    // --- Engine + egret schema setup ----------------------------------------
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();

    // --- Two workspaces, both with real warehouses --------------------------
    let (ws_a, slug_a, wh_a, wid_a) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "t5a").await;
    let (_ws_b, _slug_b, wh_b, _wid_b) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "t5b").await;
    assert_ne!(wh_a, wh_b, "the two workspaces get distinct warehouses");

    // --- Mint an egret catalog token for workspace A's warehouse ------------
    let (_token_row, token_a) = store
        .create_token(
            ws_a,
            wid_a,
            "t5-token-a",
            &star_scope(&["catalog:read", "catalog:write"]),
        )
        .await
        .expect("mint catalog token for workspace A");

    // --- The LIVE mounted router (front-auth + lakekeeper), same as prod -----
    let app = mount_catalog_engine(engine_state.clone(), store.clone(), None);

    // Helper to fire a request and collect (status, json).
    let prefix_a = wh_a.to_string();
    let prefix_b = wh_b.to_string();

    // === 1. token A on A's prefix → create namespace 2xx ====================
    let ns_name = format!("demo_{}", Uuid::now_v7().simple());
    let body = serde_json::json!({ "namespace": [ns_name], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", format!("Bearer {token_a}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "token A on A's warehouse creates a namespace; body={ns}"
    );
    assert_eq!(ns["namespace"][0], ns_name, "namespace echoed; body={ns}");

    // ...and a table (deeper path, still A's prefix) → 2xx.
    let create_table = serde_json::json!({
        "name": "t",
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces/{ns_name}/tables"))
                .header("Authorization", format!("Bearer {token_a}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_table).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, created) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "token A on A's warehouse creates a table; body={created}"
    );
    assert!(
        created["metadata-location"].is_string(),
        "created table has a metadata-location; body={created}"
    );

    // === 2. token A on B's prefix → 403 FORBIDDEN (cross-tenant guard) =======
    let cross_ns = format!("demo_{}", Uuid::now_v7().simple());
    let body = serde_json::json!({ "namespace": [cross_ns], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_b}/namespaces"))
                .header("Authorization", format!("Bearer {token_a}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "token A may NOT touch workspace B's warehouse (A-3 ownership)"
    );

    // === 3. unknown / garbage token → 401 UNAUTHORIZED ======================
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", "Bearer egcat_not_a_real_token")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "namespace": ["x"] })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "garbage token is rejected before reaching lakekeeper"
    );

    // ...and a missing Authorization header is likewise 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "namespace": ["x"] })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a missing Bearer token is 401"
    );

    // === 4. /config (no prefix) with a valid token → allowed, discovers prefix
    // The prefix-less discovery call carries no `{prefix}` to ownership-check, so
    // the middleware passes any valid token through to lakekeeper, which resolves
    // `?warehouse=<slug>` and hands back that warehouse's UUID as the prefix to use.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/config?warehouse={slug_a}"))
                .header("Authorization", format!("Bearer {token_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, config) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/config with a valid token is allowed (no ownership check); body={config}"
    );
    assert_eq!(
        config["defaults"]["prefix"].as_str(),
        Some(prefix_a.as_str()),
        "/config resolves the caller's warehouse UUID as the prefix; body={config}"
    );
}

#[tokio::test]
async fn front_auth_enforces_per_token_permissions() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping front_auth permission scope test");
        return;
    };

    // --- Engine + egret schema setup ----------------------------------------
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();

    // --- One workspace with a real warehouse --------------------------------
    let (ws_a, slug_a, wh_a, wid_a) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "t6a").await;

    let prefix_a = wh_a.to_string();

    // --- Mint read-only and read-write tokens for workspace A ---------------
    let (_row, token_ro) = store
        .create_token(ws_a, wid_a, "t6-ro", &star_scope(&["catalog:read"]))
        .await
        .expect("mint read-only token");

    let (_row, token_rw) = store
        .create_token(
            ws_a,
            wid_a,
            "t6-rw",
            &star_scope(&["catalog:read", "catalog:write"]),
        )
        .await
        .expect("mint read-write token");

    let app = mount_catalog_engine(engine_state.clone(), store.clone(), None);

    // === A. Read-only token on A's prefix — GET namespaces → 200 ===========
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", format!("Bearer {token_ro}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "read-only token may list namespaces (GET → catalog:read); body={body}"
    );

    // === B. Read-only token — POST create namespace → 403 ===================
    let ro_ns = format!("ro_demo_{}", Uuid::now_v7().simple());
    let create_ns_body = serde_json::json!({ "namespace": [ro_ns], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", format!("Bearer {token_ro}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_ns_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "read-only token MUST NOT create a namespace (POST → catalog:write)"
    );

    // === C. Read-only token — POST create table → 403 =======================
    // First need a namespace to attempt table creation under. Use token_rw.
    let rw_ns = format!("rw_base_{}", Uuid::now_v7().simple());
    let ns_body = serde_json::json!({ "namespace": [rw_ns], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", format!("Bearer {token_rw}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&ns_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (setup_status, setup_body) = into_json(resp).await;
    assert_eq!(
        setup_status,
        StatusCode::OK,
        "rw token creates namespace for subsequent tests; body={setup_body}"
    );

    // Now try to create a table with the read-only token → must 403.
    let create_table = serde_json::json!({
        "name": "ro_table",
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces/{rw_ns}/tables"))
                .header("Authorization", format!("Bearer {token_ro}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_table).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "read-only token MUST NOT create a table (POST → catalog:write)"
    );

    // === D. Read-write token — create namespace + table → 200 each ==========
    let rw_ns2 = format!("rw_demo_{}", Uuid::now_v7().simple());
    let ns_body2 = serde_json::json!({ "namespace": [rw_ns2], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces"))
                .header("Authorization", format!("Bearer {token_rw}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&ns_body2).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns_resp) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "rw token creates namespace; body={ns_resp}"
    );

    let rw_table = serde_json::json!({
        "name": "rw_t",
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix_a}/namespaces/{rw_ns2}/tables"))
                .header("Authorization", format!("Bearer {token_rw}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&rw_table).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, tbl_resp) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "rw token creates table; body={tbl_resp}"
    );
    assert!(
        tbl_resp["metadata-location"].is_string(),
        "created table has a metadata-location; body={tbl_resp}"
    );

    // === E. Regression: /config with read-only token → 200 (classified:read) =
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/config?warehouse={slug_a}"))
                .header("Authorization", format!("Bearer {token_ro}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, cfg) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/config is classified catalog:read so a read-only token may call it; body={cfg}"
    );
    assert_eq!(
        cfg["defaults"]["prefix"].as_str(),
        Some(prefix_a.as_str()),
        "/config with read-only token returns the warehouse prefix; body={cfg}"
    );
}

// ===========================================================================
// CE-4: EgretAuthorizer enforces the per-token egret scope (perms +
// namespace_scope) INSIDE the engine. The front layer (above) is workspace-
// ownership + coarse read/write; these tests prove the authorizer additionally
// enforces the per-namespace `namespace_scope` boundary (out-of-scope access is
// denied — lakekeeper surfaces it as 404, hiding existence; see the scoped test
// for why), and — critically — that a FULL-scope token still round-trips
// normally (the regression guard that swapping AllowAll → EgretAuthorizer did
// not break the happy path).
// ===========================================================================

/// JSON body to create a namespace `name`.
fn create_ns_body(name: &str) -> serde_json::Value {
    serde_json::json!({ "namespace": [name], "properties": {} })
}

/// JSON body to create a minimal single-column table `name`.
fn create_table_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    })
}

/// Fire a POST `{path}` with a JSON `body` under `Bearer token`; return status.
async fn post(app: &axum::Router, token: &str, path: &str, body: &serde_json::Value) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Fire a GET `{path}` under `Bearer token`; return status.
async fn get(app: &axum::Router, token: &str, path: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// REGRESSION GUARD: a FULL-scope (no `namespace_scope`) read-write token must
/// behave exactly as before the EgretAuthorizer swap — create/read/commit
/// across ANY namespace, no per-namespace restriction. If the authorizer ever
/// over-denies the normal flow, this fails.
#[tokio::test]
async fn front_auth_full_scope_token_is_unrestricted() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping front_auth full-scope test");
        return;
    };

    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();
    let (ws_a, _slug_a, wh_a, wid_a) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "ce4full").await;
    let prefix = wh_a.to_string();

    // Full-scope rw token: read + write, EMPTY namespace_scope (= all namespaces).
    let (_row, token) = store
        .create_token(
            ws_a,
            wid_a,
            "ce4-full",
            &star_scope(&["catalog:read", "catalog:write"]),
        )
        .await
        .expect("mint full-scope rw token");

    let app = mount_catalog_engine(engine_state.clone(), store.clone(), None);

    // marts: create ns, create + load table.
    assert_eq!(
        post(
            &app,
            &token,
            &format!("/{prefix}/namespaces"),
            &create_ns_body("marts")
        )
        .await,
        StatusCode::OK,
        "full-scope token creates namespace `marts`"
    );
    assert_eq!(
        post(
            &app,
            &token,
            &format!("/{prefix}/namespaces/marts/tables"),
            &create_table_body("orders"),
        )
        .await,
        StatusCode::OK,
        "full-scope token creates table `marts.orders`"
    );
    assert_eq!(
        get(
            &app,
            &token,
            &format!("/{prefix}/namespaces/marts/tables/orders"),
        )
        .await,
        StatusCode::OK,
        "full-scope token loads table `marts.orders`"
    );

    // A DIFFERENT namespace (marketing): also fully allowed (no ns restriction).
    assert_eq!(
        post(
            &app,
            &token,
            &format!("/{prefix}/namespaces"),
            &create_ns_body("marketing"),
        )
        .await,
        StatusCode::OK,
        "full-scope token creates namespace `marketing`"
    );
    assert_eq!(
        post(
            &app,
            &token,
            &format!("/{prefix}/namespaces/marketing/tables"),
            &create_table_body("campaigns"),
        )
        .await,
        StatusCode::OK,
        "full-scope token creates table `marketing.campaigns`"
    );
}

/// CORE CE-4 BOUNDARY: a token scoped to `namespace_scope = ["marts"]` may
/// operate inside `marts` but is DENIED on `marketing` — even though it owns the
/// workspace and holds `catalog:write`. The denial comes from the
/// EgretAuthorizer (the front layer is namespace-agnostic).
///
/// lakekeeper surfaces the denial as **404 NOT_FOUND**, not 403: before checking
/// the requested action it runs a "can you SEE this namespace" predicate (a
/// read-class check our scoped token also fails for `marketing`), and a
/// principal that cannot see a namespace is told it does not exist — deliberate
/// existence-hiding (no leak of out-of-scope namespaces). Either way the data
/// action does NOT execute: the boundary holds. We assert the exact 404 so the
/// test fails loudly if that behavior ever changes (e.g. silently starts
/// leaking 403-with-existence, or — worse — starts succeeding).
#[tokio::test]
async fn front_auth_namespace_scope_is_enforced() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping front_auth namespace-scope test");
        return;
    };

    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();
    let (ws_a, _slug_a, wh_a, wid_a) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "ce4ns").await;
    let prefix = wh_a.to_string();

    let app = mount_catalog_engine(engine_state.clone(), store.clone(), None);

    // A full-scope token pre-creates BOTH namespaces (so the marts-scoped token's
    // marketing denial is the authorizer refusing, not lakekeeper 404ing).
    let (_row, full) = store
        .create_token(
            ws_a,
            wid_a,
            "ce4ns-full",
            &star_scope(&["catalog:read", "catalog:write"]),
        )
        .await
        .expect("mint full-scope token");
    assert_eq!(
        post(
            &app,
            &full,
            &format!("/{prefix}/namespaces"),
            &create_ns_body("marts")
        )
        .await,
        StatusCode::OK,
        "setup: create namespace `marts`"
    );
    assert_eq!(
        post(
            &app,
            &full,
            &format!("/{prefix}/namespaces"),
            &create_ns_body("marketing"),
        )
        .await,
        StatusCode::OK,
        "setup: create namespace `marketing`"
    );

    // The marts-scoped rw token: write perm + workspace ownership, BUT ns scope
    // = ["marts"].
    let (_row, scoped) = store
        .create_token(
            ws_a,
            wid_a,
            "ce4ns-marts",
            &ns_scope("marts", &["catalog:read", "catalog:write"]),
        )
        .await
        .expect("mint marts-scoped token");

    // IN scope: create a table under `marts` → allowed.
    assert_eq!(
        post(
            &app,
            &scoped,
            &format!("/{prefix}/namespaces/marts/tables"),
            &create_table_body("scoped_ok"),
        )
        .await,
        StatusCode::OK,
        "marts-scoped token creates a table under `marts`"
    );
    // IN scope: read `marts` (list tables) → allowed.
    assert_eq!(
        get(&app, &scoped, &format!("/{prefix}/namespaces/marts/tables")).await,
        StatusCode::OK,
        "marts-scoped token lists tables under `marts`"
    );

    // OUT of scope: create a table under `marketing` → DENIED by the authorizer
    // (404: lakekeeper hides the out-of-scope namespace's existence; see doc).
    let blocked_write = post(
        &app,
        &scoped,
        &format!("/{prefix}/namespaces/marketing/tables"),
        &create_table_body("blocked"),
    )
    .await;
    assert_eq!(
        blocked_write,
        StatusCode::NOT_FOUND,
        "marts-scoped token MUST be denied writing under `marketing` (namespace_scope); got {blocked_write}"
    );
    assert!(
        blocked_write.is_client_error(),
        "out-of-scope write is a client-error denial, never a success; got {blocked_write}"
    );
    // OUT of scope: list tables under `marketing` → DENIED (likewise 404).
    let blocked_read = get(
        &app,
        &scoped,
        &format!("/{prefix}/namespaces/marketing/tables"),
    )
    .await;
    assert_eq!(
        blocked_read,
        StatusCode::NOT_FOUND,
        "marts-scoped token MUST be denied listing under `marketing` (namespace_scope); got {blocked_read}"
    );
    assert!(
        blocked_read.is_client_error(),
        "out-of-scope read is a client-error denial, never a success; got {blocked_read}"
    );
}
