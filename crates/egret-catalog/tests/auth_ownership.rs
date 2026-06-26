//! Per-workspace catalog-token ownership tests (SECURITY-CRITICAL).
//!
//! Verifies the two-plane invariant of `require_bearer` + `resolve_catalog`:
//! - a workspace token reaches its OWN `{prefix}` (200),
//! - a workspace token on a FOREIGN `{prefix}` is rejected (403 FORBIDDEN),
//! - a garbage/unknown token is rejected (401 UNAUTHORIZED),
//! - the static admin token bypasses ownership and reaches any `{prefix}` (200).
//!
//! Self-skips without `DATABASE_URL` (Postgres). MinIO is NOT required: storage is
//! an in-memory `ObjectStore` test double.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_catalog::service::{CatalogService, CatalogStore};
use egret_catalog::{iceberg_router, CatalogState};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

const ADMIN_TOKEN: &str = "test-admin-token";
const WAREHOUSE: &str = "s3://warehouse";

/// In-memory object store so the round-trip doesn't need MinIO.
struct MemStore(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

#[async_trait::async_trait]
impl egret_storage::ObjectStore for MemStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), egret_storage::StorageError> {
        self.0.lock().unwrap().insert(uri.to_string(), bytes);
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
        self.0
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| egret_storage::StorageError::NotFound(uri.to_string()))
    }
    async fn delete(&self, uri: &str) -> Result<(), egret_storage::StorageError> {
        self.0.lock().unwrap().remove(uri);
        Ok(())
    }
}

fn storage_cfg() -> egret_storage::StorageConfig {
    egret_storage::StorageConfig {
        endpoint: Some("http://localhost:9000".into()),
        region: "us-east-1".into(),
        bucket: "warehouse".into(),
        access_key: "AKIDEXAMPLE".into(),
        secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
        path_style: true,
    }
}

/// Seed a workspace + its default catalog; return the workspace slug (the `{prefix}`).
async fn seed_ws(store: &CatalogStore) -> (uuid::Uuid, String) {
    let ws = uuid::Uuid::new_v4();
    let slug = format!("ws_{}", ws.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws)
        .bind(&slug)
        .bind(&slug)
        .execute(store.pool())
        .await
        .expect("seed workspace");
    store
        .create_catalog(ws, "default", WAREHOUSE)
        .await
        .expect("seed catalog");
    (ws, slug)
}

/// Build the catalog state + router and seed TWO workspaces (A and B).
/// Returns the router, A's slug, A's per-workspace raw token, and B's slug.
async fn setup() -> Option<(axum::Router, String, String, String)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = CatalogStore::new(db);

    let (ws_a, slug_a) = seed_ws(&store).await;
    let (_ws_b, slug_b) = seed_ws(&store).await;

    // Mint a real per-workspace token for workspace A.
    let (_row, token_a) = store.create_token(ws_a, "engine-a").await.expect("token A");

    let service = CatalogService {
        store,
        storage: Arc::new(MemStore(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        ))),
    };
    let st = CatalogState {
        service,
        auth_token: Arc::from(ADMIN_TOKEN),
        storage_cfg: Arc::new(storage_cfg()),
        base_url: Arc::from("http://localhost:8080/catalog"),
    };
    Some((iceberg_router(st), slug_a, token_a, slug_b))
}

/// GET `/v1/{prefix}/namespaces` with the given bearer token.
async fn get_namespaces(app: &axum::Router, prefix: &str, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/{prefix}/namespaces"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// (1) Workspace A's token reaches A's own prefix → 200.
#[tokio::test]
async fn workspace_token_reaches_own_prefix() {
    let Some((app, slug_a, token_a, _slug_b)) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    assert_eq!(
        get_namespaces(&app, &slug_a, &token_a).await,
        StatusCode::OK,
        "A's token on A's prefix must be allowed"
    );
}

/// (2) Workspace A's token on workspace B's prefix → 403 FORBIDDEN.
#[tokio::test]
async fn workspace_token_on_foreign_prefix_is_forbidden() {
    let Some((app, _slug_a, token_a, slug_b)) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    assert_eq!(
        get_namespaces(&app, &slug_b, &token_a).await,
        StatusCode::FORBIDDEN,
        "A's token on B's prefix must be 403"
    );
}

/// (3) A garbage/unknown token on any prefix → 401 UNAUTHORIZED.
#[tokio::test]
async fn garbage_token_is_unauthorized() {
    let Some((app, slug_a, _token_a, _slug_b)) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    assert_eq!(
        get_namespaces(&app, &slug_a, "garbage-token-no-such-row").await,
        StatusCode::UNAUTHORIZED,
        "unknown token must be 401"
    );
}

/// (4) The static admin token bypasses ownership and reaches any prefix → 200.
#[tokio::test]
async fn admin_token_bypasses_ownership() {
    let Some((app, slug_a, _token_a, slug_b)) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    assert_eq!(
        get_namespaces(&app, &slug_a, ADMIN_TOKEN).await,
        StatusCode::OK,
        "admin token must reach A"
    );
    assert_eq!(
        get_namespaces(&app, &slug_b, ADMIN_TOKEN).await,
        StatusCode::OK,
        "admin token must reach B"
    );
}

/// Sanity: the deny check fires on the signing endpoint too (403 on foreign prefix).
#[tokio::test]
async fn sign_on_foreign_prefix_is_forbidden() {
    let Some((app, _slug_a, token_a, slug_b)) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let body = serde_json::json!({
        "region": "us-east-1",
        "uri": "https://warehouse.s3.us-east-1.amazonaws.com/x",
        "method": "GET",
        "headers": {},
    });
    let status = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/{slug_b}/aws/s3/sign"))
                .header("Authorization", format!("Bearer {token_a}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "A's token signing on B's prefix must be 403 before any signing"
    );
}
