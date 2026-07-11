//! DB-gated JobManager tests with an injected fake connector.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use egret_core::{ColumnMeta, TabularResult};
use egret_metastore::Db;
use egret_query::execution::connector::{Connector, ConnectorFactory};
use egret_query::execution::model::{Limits, QueryStatus};
use egret_query::registry::model::Database;
use egret_query::{JobManager, QueryStore};
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
    ) -> Result<TabularResult, egret_query::EngineError> {
        for _ in 0..(self.sleep_ms / 10).max(1) {
            if cancel.is_cancelled() {
                return Err(egret_query::EngineError::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(TabularResult {
            columns: vec![ColumnMeta {
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
    /// Records the last `session_user` passed to `build` for test assertions.
    last_session_user: Arc<Mutex<Option<Option<String>>>>,
    /// Records the last `bearer_token` passed to `build` for test assertions.
    last_bearer_token: Arc<Mutex<Option<Option<String>>>>,
}

impl FakeFactory {
    fn new(sleep_ms: u64) -> Self {
        FakeFactory {
            sleep_ms,
            last_session_user: Arc::new(Mutex::new(None)),
            last_bearer_token: Arc::new(Mutex::new(None)),
        }
    }

    fn last_session_user(&self) -> Option<Option<String>> {
        self.last_session_user.lock().unwrap().clone()
    }

    fn last_bearer_token(&self) -> Option<Option<String>> {
        self.last_bearer_token.lock().unwrap().clone()
    }
}

#[async_trait]
impl ConnectorFactory for FakeFactory {
    async fn build(
        &self,
        _db: &Database,
        _secret: Option<String>,
        session_user: Option<String>,
        bearer_token: Option<String>,
    ) -> Result<Box<dyn Connector>, egret_query::EngineError> {
        *self.last_session_user.lock().unwrap() = Some(session_user);
        *self.last_bearer_token.lock().unwrap() = Some(bearer_token);
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

async fn setup(sleep_ms: u64) -> Option<(JobManager, QueryStore, Uuid, Uuid, Arc<FakeFactory>)> {
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
        "INSERT INTO query.databases (name,engine,host,port,workspace_id) \
         VALUES ($1,'trino','h',8080,$2) RETURNING id",
    )
    .bind(format!("db_{}", Uuid::new_v4().simple()))
    .bind(wid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let store = QueryStore::new(db.clone());
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
    let factory = Arc::new(FakeFactory::new(sleep_ms));
    let jobs = JobManager::new(
        store.clone(),
        egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        factory.clone(),
        storage,
        "s3://warehouse".into(),
        "query-results".into(),
        Limits {
            max_rows: 100,
            timeout: Duration::from_secs(10),
        },
        100,
    );
    Some((jobs, store, wid, did, factory))
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
    let Some((jobs, store, wid, did, _factory)) = setup(0).await else {
        eprintln!("skip");
        return;
    };
    let id = jobs
        .submit(wid, did, None, None, None, "SELECT 1", None)
        .await
        .unwrap();
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
    let Some((jobs, store, wid, did, _factory)) = setup(2000).await else {
        eprintln!("skip");
        return;
    };
    let id = jobs
        .submit(wid, did, None, None, None, "SELECT 1", None)
        .await
        .unwrap();
    wait_status(&store, id, QueryStatus::Running, 20).await;
    jobs.cancel(id).await.unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Cancelled, 40).await,
        QueryStatus::Cancelled
    );
}

/// DB-gated (needs `DATABASE_URL`), engine-free: exercises the shared
/// `run_query` path end to end with the fake connector — resolve the database,
/// build the connector via the factory, run to completion, return the coerced
/// `TabularResult`. Skips (does not fail) when `DATABASE_URL` is unset.
#[tokio::test]
async fn run_query_returns_tabular_result() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let wid: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug,name) VALUES ($1,'t') RETURNING id")
            .bind(format!("w-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let did: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name,engine,host,port,workspace_id) \
         VALUES ($1,'trino','h',8080,$2) RETURNING id",
    )
    .bind(format!("db_{}", Uuid::new_v4().simple()))
    .bind(wid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let databases = egret_query::DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let factory = FakeFactory::new(0);
    let limits = Limits {
        max_rows: 100,
        timeout: Duration::from_secs(10),
    };
    let result = egret_query::execution::run_query(
        &databases,
        &factory,
        did,
        "SELECT 1",
        Some(50),
        Some("alice".to_string()),
        None,
        &limits,
        CancellationToken::new(),
    )
    .await
    .expect("run_query ok");

    // The fake connector returns one bigint column `n` with a single row [1].
    assert_eq!(result.columns.len(), 1);
    assert_eq!(result.columns[0].name, "n");
    assert_eq!(result.row_count, 1);
    assert_eq!(result.rows, vec![vec![serde_json::json!(1)]]);
    assert!(!result.truncated);
    // The resolved connection carried the session_user through the factory.
    assert_eq!(factory.last_session_user(), Some(Some("alice".to_string())));
}

/// DB-gated: the `bearer_token` passed to `run_query` reaches `factory.build`
/// unchanged (B3a plumbing). Non-delegated callers pass `None`; the delegation
/// resolvers (Tasks 3/4) pass a real per-user JWT into this same slot.
#[tokio::test]
async fn bearer_token_reaches_factory() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let wid: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug,name) VALUES ($1,'t') RETURNING id")
            .bind(format!("w-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let did: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name,engine,host,port,workspace_id) \
         VALUES ($1,'trino','h',8080,$2) RETURNING id",
    )
    .bind(format!("db_{}", Uuid::new_v4().simple()))
    .bind(wid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let databases = egret_query::DatabaseRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), None),
        egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        ),
    );
    let factory = FakeFactory::new(0);
    let limits = Limits {
        max_rows: 100,
        timeout: Duration::from_secs(10),
    };
    egret_query::execution::run_query(
        &databases,
        &factory,
        did,
        "SELECT 1",
        Some(50),
        Some("alice".to_string()),
        Some("tok".to_string()),
        &limits,
        CancellationToken::new(),
    )
    .await
    .expect("run_query ok");

    // The bearer_token flowed through run_query into factory.build unchanged.
    assert_eq!(
        factory.last_bearer_token(),
        Some(Some("tok".to_string())),
        "factory.build must receive bearer_token=Some(\"tok\")"
    );
    assert_eq!(factory.last_session_user(), Some(Some("alice".to_string())));
}

#[tokio::test]
async fn session_user_reaches_factory() {
    let Some((jobs, store, wid, did, factory)) = setup(0).await else {
        eprintln!("skip");
        return;
    };
    let id = jobs
        .submit(
            wid,
            did,
            None,
            Some("alice".to_string()),
            None,
            "SELECT 1",
            None,
        )
        .await
        .unwrap();
    // Wait for the job to run (factory.build is called inside the spawned task).
    wait_status(&store, id, QueryStatus::Finished, 40).await;
    // Assert the factory captured the session_user we passed.
    assert_eq!(
        factory.last_session_user(),
        Some(Some("alice".to_string())),
        "factory.build must receive session_user=Some(\"alice\")"
    );
}
