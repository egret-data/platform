//! Engine-gated test that a per-query limit caps the result. Self-skip without env.
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{Connector, ConnectorFactory};
use egret_engine::model::{Limits, QueryStatus};
use egret_engine::{JobManager, QueryStore};
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

struct TrinoFactory {
    host: String,
    port: i32,
}
#[async_trait::async_trait]
impl ConnectorFactory for TrinoFactory {
    async fn build(
        &self,
        _db: &egret_database::model::Database,
        _s: Option<String>,
    ) -> Result<Box<dyn Connector>, egret_engine::EngineError> {
        Ok(egret_engine::connector::build_connector(
            &egret_engine::connector::ConnSpec {
                engine: Engine::Trino,
                host: self.host.clone(),
                port: self.port,
                options: ConnOptions {
                    user: Some("admin".into()),
                    http_scheme: Some("http".into()),
                    ..Default::default()
                },
                secret: None,
            },
        ))
    }
}
#[derive(Clone, Default)]
struct MemStore(Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>);
#[async_trait::async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, u: &str, b: Vec<u8>) -> Result<(), StorageError> {
        self.0.lock().unwrap().insert(u.into(), b);
        Ok(())
    }
    async fn get(&self, u: &str) -> Result<Vec<u8>, StorageError> {
        self.0
            .lock()
            .unwrap()
            .get(u)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(u.into()))
    }
    async fn delete(&self, u: &str) -> Result<(), StorageError> {
        self.0.lock().unwrap().remove(u);
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

#[tokio::test]
async fn per_query_limit_caps_rows() {
    let (Some(url), Some(trino)) = (
        std::env::var("DATABASE_URL").ok(),
        std::env::var("EGRET_TEST_TRINO_URL").ok(),
    ) else {
        eprintln!("skip");
        return;
    };
    let u = url::Url::parse(&trino).unwrap();
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.unwrap();
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
        ),
        Arc::new(TrinoFactory {
            host: u.host_str().unwrap().into(),
            port: u.port().unwrap_or(8080) as i32,
        }),
        storage,
        "s3://warehouse".into(),
        "query-results".into(),
        Limits {
            max_rows: 100_000,
            timeout: Duration::from_secs(30),
        },
        100,
    );
    let id = jobs
        .submit(
            wid,
            did,
            None,
            "SELECT * FROM UNNEST(SEQUENCE(1,100)) AS t(n)",
            Some(5),
        )
        .await
        .unwrap();
    for _ in 0..40 {
        if store.get(id).await.unwrap().unwrap().status == QueryStatus::Finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Finished);
    assert_eq!(q.row_count, Some(5));
    assert!(
        !q.truncated,
        "a SQL-LIMIT'd query returns exactly N and is not 'truncated'"
    );
}
