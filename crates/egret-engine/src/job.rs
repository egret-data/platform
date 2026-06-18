//! JobManager: async query execution with a Postgres-persisted lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egret_database::DatabaseRepo;
use egret_storage::ObjectStore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::connector::ConnectorFactory;
use crate::error::EngineError;
use crate::model::Limits;
use crate::store::QueryStore;

#[derive(Clone)]
pub struct JobManager {
    store: QueryStore,
    databases: DatabaseRepo,
    factory: Arc<dyn ConnectorFactory>,
    storage: Arc<dyn ObjectStore>,
    bucket_uri: Arc<str>,     // e.g. "s3://warehouse"
    results_prefix: Arc<str>, // e.g. "query-results"
    limits: Limits,
    preview_rows: usize,
    cancels: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: QueryStore,
        databases: DatabaseRepo,
        factory: Arc<dyn ConnectorFactory>,
        storage: Arc<dyn ObjectStore>,
        bucket_uri: String,
        results_prefix: String,
        limits: Limits,
        preview_rows: usize,
    ) -> Self {
        JobManager {
            store,
            databases,
            factory,
            storage,
            bucket_uri: bucket_uri.into(),
            results_prefix: results_prefix.into(),
            limits,
            preview_rows,
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Insert a queued row, spawn the execution task, return the id.
    pub async fn submit(
        &self,
        ws: Uuid,
        database_id: Uuid,
        user: Option<Uuid>,
        sql: &str,
        limit: Option<usize>,
    ) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?;
        let max_rows = limit
            .map(|l| l.clamp(1, self.limits.max_rows))
            .unwrap_or(self.limits.max_rows);
        let limits = Limits {
            max_rows,
            timeout: self.limits.timeout,
        };
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let sql = sql.to_string();
        tokio::spawn(async move {
            this.run(id, database_id, sql, limits, token).await;
            this.cancels.lock().unwrap().remove(&id);
        });
        Ok(id)
    }

    /// Cancel a running query (best-effort): trigger the token; the connector aborts.
    pub async fn cancel(&self, id: Uuid) -> Result<(), EngineError> {
        let tok = self.cancels.lock().unwrap().get(&id).cloned();
        if let Some(tok) = tok {
            tok.cancel();
        }
        Ok(())
    }

    async fn run(
        &self,
        id: Uuid,
        database_id: Uuid,
        sql: String,
        limits: Limits,
        token: CancellationToken,
    ) {
        if let Err(e) = self.store.set_running(id).await {
            tracing::warn!(%id, error=%e, "failed to mark query running");
            return;
        }
        match self.execute(id, database_id, &sql, &limits, &token).await {
            Ok(()) => {} // finish() already persisted inside execute()
            Err(EngineError::Cancelled) => {
                let _ = self.store.set_cancelled(id).await;
            }
            Err(e) => {
                let _ = self.store.fail(id, &e.to_string()).await;
            }
        }
    }

    /// Resolve the connection, run with the outer timeout, write the result, persist finished.
    async fn execute(
        &self,
        id: Uuid,
        database_id: Uuid,
        sql: &str,
        limits: &Limits,
        token: &CancellationToken,
    ) -> Result<(), EngineError> {
        let db = self.databases.get(database_id).await?;
        let secret = self.databases.open_secret(database_id).await?;
        let connector = self.factory.build(&db, secret).await?;

        let fut = connector.execute(sql, limits, token.clone());
        let result = match tokio::time::timeout(limits.timeout, fut).await {
            Ok(r) => r?,
            Err(_) => {
                token.cancel();
                return Err(EngineError::Timeout);
            }
        };

        let uri = format!(
            "{}/{}/{}.jsonl",
            self.bucket_uri,
            self.results_prefix.trim_matches('/'),
            id
        );
        self.store
            .write_result(&self.storage, &uri, &result)
            .await?;
        self.store
            .finish(id, &result, &uri, self.preview_rows)
            .await?;
        Ok(())
    }
}
