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
    /// The SQL the engine was sent.
    last_sql: Arc<Mutex<Option<String>>>,
}

/// How many rows the fake engine's table `t` holds.
const TABLE_ROWS: usize = 25;

#[async_trait]
impl Connector for FakeConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, egret_query::EngineError> {
        *self.last_sql.lock().unwrap() = Some(sql.to_string());
        for _ in 0..(self.sleep_ms / 10).max(1) {
            if cancel.is_cancelled() {
                return Err(egret_query::EngineError::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // A query of table `t`: the engine applies a trailing `LIMIT k` (as the
        // real engines do), and the connector fetches `max_rows + 1` and flags
        // `truncated`, as the real connectors do.
        if sql.contains(" FROM t") {
            if sql.contains("_egret_lim") && sql.contains("nosuch") {
                // Trino's position for a name on the user's first line, one line
                // down inside the wrapper.
                return Err(egret_query::EngineError::Engine(
                    "line 2:15: Table 'nosuch' does not exist".into(),
                ));
            }
            let engine_rows = sql
                .rsplit_once("LIMIT ")
                .and_then(|(_, k)| k.trim().parse::<usize>().ok())
                .map_or(TABLE_ROWS, |k| k.min(TABLE_ROWS));
            let cap = limits.max_rows;
            let mut rows: Vec<Vec<serde_json::Value>> = (0..engine_rows.min(cap + 1))
                .map(|i| vec![serde_json::json!(TABLE_ROWS - i)])
                .collect();
            let truncated = rows.len() > cap;
            rows.truncate(cap);
            return Ok(TabularResult {
                columns: vec![ColumnMeta {
                    name: "n".into(),
                    ty: "bigint".into(),
                }],
                row_count: rows.len() as u64,
                rows,
                bytes_scanned: None,
                truncated,
            });
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
    /// The SQL the last connector was sent.
    last_sql: Arc<Mutex<Option<String>>>,
}

impl FakeFactory {
    fn new(sleep_ms: u64) -> Self {
        FakeFactory {
            sleep_ms,
            last_session_user: Arc::new(Mutex::new(None)),
            last_bearer_token: Arc::new(Mutex::new(None)),
            last_sql: Arc::new(Mutex::new(None)),
        }
    }

    fn last_sql(&self) -> Option<String> {
        self.last_sql.lock().unwrap().clone()
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
            last_sql: self.last_sql.clone(),
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
    setup_with_timeout(sleep_ms, Duration::from_secs(10)).await
}

async fn setup_with_timeout(
    sleep_ms: u64,
    timeout: Duration,
) -> Option<(JobManager, QueryStore, Uuid, Uuid, Arc<FakeFactory>)> {
    let url = egret_test_db::url().await?;
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
            timeout,
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
    let Some(url) = egret_test_db::url().await else {
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
    let Some(url) = egret_test_db::url().await else {
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

/// KI-154 + KI-081: SQL Lab's row limit keeps the query's own `ORDER BY`
/// outermost (no subquery for Trino to drop the ordering of), and asks for one
/// row more, so a cut result says it was cut.
#[tokio::test]
async fn the_row_limit_keeps_order_by_outermost_and_reports_a_cut_result() {
    let Some((jobs, store, wid, did, factory)) = setup(0).await else {
        eprintln!("skip");
        return;
    };
    let sql = "SELECT n FROM t ORDER BY n DESC";
    let id = jobs
        .submit(wid, did, None, None, None, sql, Some(5))
        .await
        .unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Finished, 40).await,
        QueryStatus::Finished
    );
    assert_eq!(
        factory.last_sql().as_deref(),
        Some("SELECT n FROM t ORDER BY n DESC\nLIMIT 6")
    );
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.row_count, Some(5));
    assert!(
        q.truncated,
        "25 rows exist, 5 were returned: the result is cut"
    );

    // A limit the result fits in is not flagged.
    let id = jobs
        .submit(wid, did, None, None, None, sql, Some(30))
        .await
        .unwrap();
    wait_status(&store, id, QueryStatus::Finished, 40).await;
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.row_count, Some(TABLE_ROWS as i64));
    assert!(!q.truncated);
}

/// KI-087: when the query is wrapped (SQL the parser cannot read), the engine's
/// error positions are mapped back to the user's lines.
#[tokio::test]
async fn a_wrapped_query_reports_errors_on_the_users_line() {
    let Some((jobs, store, wid, did, factory)) = setup(0).await else {
        eprintln!("skip");
        return;
    };
    // `/* a /* b */` nests for sqlparser and not for Trino: not decided on, wrapped.
    let id = jobs
        .submit(
            wid,
            did,
            None,
            None,
            None,
            "SELECT n FROM t, nosuch /* a /* b */",
            Some(5),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Failed, 40).await,
        QueryStatus::Failed
    );
    assert!(factory.last_sql().unwrap().contains("_egret_lim"));
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(
        q.error.as_deref(),
        Some("engine: line 1:15: Table 'nosuch' does not exist")
    );
}

/// KI-211: a query that outruns SQL Lab's time limit says what the limit is and
/// which setting sets it, not a bare "timeout".
#[tokio::test]
async fn a_timeout_names_the_limit_and_the_setting() {
    let Some((jobs, store, wid, did, _factory)) =
        setup_with_timeout(5000, Duration::from_secs(1)).await
    else {
        eprintln!("skip");
        return;
    };
    let id = jobs
        .submit(wid, did, None, None, None, "SELECT 1", None)
        .await
        .unwrap();
    assert_eq!(
        wait_status(&store, id, QueryStatus::Failed, 80).await,
        QueryStatus::Failed
    );
    let err = store.get(id).await.unwrap().unwrap().error.unwrap();
    assert!(err.contains("1 s"), "{err}");
    assert!(err.contains("engine.query_timeout_secs"), "{err}");
}

/// KI-318: a DDL statement (a rename egret holds while engines confirm moved
/// restrictions) can outlive SQL Lab's limit and still commit on the engine.
/// Its timeout says so and points to Recent changes; a query's does not.
#[tokio::test]
async fn a_ddl_timeout_says_the_change_may_still_complete() {
    let Some((jobs, store, wid, did, _factory)) =
        setup_with_timeout(5000, Duration::from_secs(1)).await
    else {
        eprintln!("skip");
        return;
    };
    let ddl = jobs
        .submit(
            wid,
            did,
            None,
            None,
            None,
            "-- move it\nalter TABLE lake.s.orders RENAME TO lake.s.orders_old",
            None,
        )
        .await
        .unwrap();
    let query = jobs
        .submit(wid, did, None, None, None, "SELECT 1", None)
        .await
        .unwrap();
    for id in [ddl, query] {
        assert_eq!(
            wait_status(&store, id, QueryStatus::Failed, 80).await,
            QueryStatus::Failed
        );
    }
    let ddl_err = store.get(ddl).await.unwrap().unwrap().error.unwrap();
    assert!(ddl_err.contains("engine.query_timeout_secs"), "{ddl_err}");
    assert!(ddl_err.contains("may still complete"), "{ddl_err}");
    assert!(ddl_err.contains("Recent changes"), "{ddl_err}");
    let query_err = store.get(query).await.unwrap().unwrap().error.unwrap();
    assert!(!query_err.contains("Recent changes"), "{query_err}");
}
