//! Router smoke tests. Unauthenticated requests must be rejected.
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_database::DatabaseRepo;
use egret_engine::model::Limits;
use egret_engine::{engine_router, EngineState, JobManager, QueryStore};
use egret_storage::{ObjectStore, StorageError};
use tower::ServiceExt;

#[derive(Default)]
struct MemStore(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.lock().unwrap().insert(uri.into(), bytes);
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(uri.into()))
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.lock().unwrap().remove(uri);
        Ok(())
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        let mut m = self.0.lock().unwrap();
        let bytes = m
            .get(src)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(src.into()))?;
        m.insert(dst.into(), bytes);
        Ok(())
    }
    async fn list(
        &self,
        _prefix: &str,
        _delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        Ok(egret_storage::Listing {
            folders: Vec::new(),
            objects: Vec::new(),
        })
    }
}

fn build_state(db: egret_metastore::Db) -> EngineState {
    let store = QueryStore::new(db.clone());
    let databases = DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
    let limits = Limits {
        max_rows: 100_000,
        timeout: std::time::Duration::from_secs(60),
    };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let jobs = JobManager::new(
        store.clone(),
        databases.clone(),
        Arc::new(egret_engine::connector::DefaultConnectorFactory),
        storage.clone(),
        "s3://warehouse".into(),
        "query-results".into(),
        limits.clone(),
        100,
    );
    EngineState {
        store,
        jobs,
        databases,
        storage,
        bucket_uri: "s3://warehouse".into(),
        results_prefix: "query-results".into(),
        limits,
        sessions,
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db),
    }
}

async fn state() -> Option<EngineState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    Some(build_state(db))
}

#[tokio::test]
async fn submit_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ws/eng/queries")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"database_id":"00000000-0000-0000-0000-000000000000","sql":"SELECT 1"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn history_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/queries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
