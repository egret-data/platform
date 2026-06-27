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
/// provision its lakekeeper warehouse; return `(workspace_id, slug, warehouse_uuid)`.
async fn seed_workspace_with_warehouse(
    db: &Db,
    store: &CatalogStore,
    engine_state: &lakekeeper::api::ApiContext<egret_catalog_engine::EngineState>,
    storage_cfg: &StorageConfig,
    tag: &str,
) -> (Uuid, String, Uuid) {
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

    (workspace_id, slug, warehouse)
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
    let engine_state = build_state(&dsn).await.expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();

    // --- Two workspaces, both with real warehouses --------------------------
    let (ws_a, slug_a, wh_a) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "t5a").await;
    let (_ws_b, _slug_b, wh_b) =
        seed_workspace_with_warehouse(&db, &store, &engine_state, &storage_cfg, "t5b").await;
    assert_ne!(wh_a, wh_b, "the two workspaces get distinct warehouses");

    // --- Mint an egret catalog token for workspace A ------------------------
    let (_token_row, token_a) = store
        .create_token(
            ws_a,
            "t5-token-a",
            &["catalog:read".to_string(), "catalog:write".to_string()],
        )
        .await
        .expect("mint catalog token for workspace A");

    // --- The LIVE mounted router (front-auth + lakekeeper), same as prod -----
    let app = mount_catalog_engine(engine_state.clone(), store.clone());

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
