//! HTTP round-trip test for the Iceberg REST catalog router.
//!
//! Self-skips without `DATABASE_URL` (Postgres). MinIO is NOT required: storage is an
//! in-memory `ObjectStore` test double, so create/load table round-trips work offline.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_catalog::service::{CatalogService, CatalogStore};
use egret_catalog::{iceberg_router, CatalogState};
use egret_iceberg_ext::catalog::CreateTableRequest;
use egret_metastore::Db;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

const TOKEN: &str = "test-secret-token";

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

async fn state() -> Option<CatalogState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let service = CatalogService {
        store: CatalogStore::new(db),
        storage: Arc::new(MemStore(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        ))),
    };
    Some(CatalogState {
        service,
        auth_token: Arc::from(TOKEN),
        warehouse: Arc::from("s3://warehouse"),
        storage_cfg: Arc::new(storage_cfg()),
        base_url: Arc::from("http://localhost:8080/catalog"),
    })
}

/// Build a minimal CreateTableRequest with a single `id: long` field.
fn create_table_request(name: &str) -> CreateTableRequest {
    use iceberg::spec::{NestedField, PrimitiveType, Schema, Type};
    let schema = Schema::builder()
        .with_fields(vec![Arc::new(NestedField::required(
            1,
            "id",
            Type::Primitive(PrimitiveType::Long),
        ))])
        .build()
        .unwrap();
    CreateTableRequest {
        name: name.to_string(),
        location: Some(format!(
            "s3://warehouse/test/{name}-{}",
            uuid::Uuid::new_v4()
        )),
        schema,
        partition_spec: None,
        write_order: None,
        stage_create: None,
        properties: None,
    }
}

#[tokio::test]
async fn config_requires_bearer() {
    let Some(st) = state().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let app = iceberg_router(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn config_with_bearer_returns_warehouse_and_signer_keys() {
    let Some(st) = state().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let app = iceberg_router(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/config")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["overrides"]["warehouse"], "s3://warehouse");
    assert_eq!(json["overrides"]["s3.remote-signing-enabled"], "true");
    assert_eq!(json["overrides"]["s3.signer"], "S3V4RestSigner");
    assert!(json["overrides"]["s3.signer.uri"].is_string());
    assert!(json["overrides"]["signer.uri"].is_string());
    assert_eq!(json["overrides"]["s3.region"], "us-east-1");
    assert_eq!(json["overrides"]["s3.path-style-access"], "true");
}

#[tokio::test]
async fn create_namespace_then_table_then_load() {
    let Some(st) = state().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let app = iceberg_router(st);
    let ns = format!("t_{}", uuid::Uuid::new_v4().simple());

    // Create namespace.
    let body = serde_json::json!({ "namespace": [ns], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/namespaces")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "create namespace");

    // Create table.
    let req = create_table_request("orders");
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/namespaces/{ns}/tables"))
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "create table; body={}",
        String::from_utf8_lossy(&bytes)
    );
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(created["metadata-location"].is_string());

    // Load table.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/namespaces/{ns}/tables/orders"))
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "load table; body={}",
        String::from_utf8_lossy(&bytes)
    );
    let loaded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(loaded["metadata"]["table-uuid"].is_string());
    assert_eq!(loaded["metadata"]["location"], req.location.unwrap());
}
