//! DB-gated JobManager tests with an injected fake connector.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use egret_database::model::Database;
use egret_engine::connector::{Connector, ConnectorFactory};
use egret_engine::model::{Column, Limits, QueryResult, QueryStatus};
use egret_engine::{JobManager, QueryStore};
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct FakeConnector {
    sleep_ms: u64,
}
#[async_trait]
impl Connector for FakeConnector {
    async fn execute(
        &self,
        _sql: &str,
        _limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, egret_engine::EngineError> {
        for _ in 0..(self.sleep_ms / 10).max(1) {
            if cancel.is_cancelled() {
                return Err(egret_engine::EngineError::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(QueryResult {
            columns: vec![Column {
                name: "n".into(),
                ty: "bigint".into(),
            }],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}
struct FakeFactory {
    sleep_ms: u64,
}
#[async_trait]
impl ConnectorFactory for FakeFactory {
    async fn build(
        &self,
        _db: &Database,
        _secret: Option<String>,
        _session_user: Option<String>,
    ) -> Result<Box<dyn Connector>, egret_engine::EngineError> {
        Ok(Box::new(FakeConnector {
            sleep_ms: self.sleep_ms,
        }))
    }
}
#[derive(Clone, Default)]
struct MemStore(Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>);
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

async fn setup(sleep_ms: u64) -> Option<(JobManager, QueryStore, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let wid: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug,name) VALUES ($1,'t') RETURNING id")
            .bind(format!("w-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let did: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (name,engine,host,port) VALUES ($1,'trino','h',8080) RETURNING id",
    )
    .bind(format!("db_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let store = QueryStore::new(db.clone());
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
    let jobs = JobManager::new(
        store.clone(),
        egret_database::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        Arc::new(FakeFactory { sleep_ms }),
        storage,
        "s3://warehouse".into(),
        "query-results".into(),
        Limits {
            max_rows: 100,
            timeout: Duration::from_secs(10),
        },
        100,
    );
    Some((jobs, store, wid, did))
}

async fn wait_status(store: &QueryStore, id: Uuid, want: QueryStatus, tries: u32) -> QueryStatus {
    for _ in 0..tries {
        let s = store.get(id).await.unwrap().unwrap().status;
        if s == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    store.get(id).await.unwrap().unwrap().status
}

#[tokio::test]
async fn submit_runs_to_finished() {
    let Some((jobs, store, wid, did)) = setup(0).await else {
        eprintln!("skip");
        return;
    };
    let id = jobs.submit(wid, did, None, "SELECT 1", None).await.unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Finished, 40).await,
        QueryStatus::Finished
    );
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.row_count, Some(1));
    assert!(q.result_ref.is_some());
}

#[tokio::test]
async fn cancel_sets_cancelled() {
    let Some((jobs, store, wid, did)) = setup(2000).await else {
        eprintln!("skip");
        return;
    };
    let id = jobs.submit(wid, did, None, "SELECT 1", None).await.unwrap();
    wait_status(&store, id, QueryStatus::Running, 20).await;
    jobs.cancel(id).await.unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Cancelled, 40).await,
        QueryStatus::Cancelled
    );
}
