//! JobManager: async query execution with a Postgres-persisted lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::registry::DatabaseRepo;
use egret_storage::ObjectStore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::execution::connector::ConnectorFactory;
use crate::execution::error::EngineError;
use crate::execution::model::Limits;
use crate::execution::store::QueryStore;
use egret_core::TabularResult;

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
        session_user: Option<String>,
        sql: &str,
        limit: Option<usize>,
    ) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?; // original sql stored
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let run_sql = sql.to_string();
        tokio::spawn(async move {
            this.run(id, database_id, run_sql, limit, session_user, token)
                .await;
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
        limit: Option<usize>,
        session_user: Option<String>,
        token: CancellationToken,
    ) {
        if let Err(e) = self.store.set_running(id).await {
            tracing::warn!(%id, error=%e, "failed to mark query running");
            return;
        }
        match self
            .execute(id, database_id, &sql, limit, session_user, &token)
            .await
        {
            Ok(()) => {} // finish() already persisted inside execute()
            Err(EngineError::Cancelled) => {
                let _ = self.store.set_cancelled(id).await;
            }
            Err(e) => {
                let _ = self.store.fail(id, &e.to_string()).await;
            }
        }
    }

    /// Run the query to completion via the shared `run_query` path, write the
    /// result, persist finished.
    async fn execute(
        &self,
        id: Uuid,
        database_id: Uuid,
        sql: &str,
        limit: Option<usize>,
        session_user: Option<String>,
        token: &CancellationToken,
    ) -> Result<(), EngineError> {
        let result = run_query(
            &self.databases,
            &*self.factory,
            database_id,
            sql,
            limit,
            session_user,
            &self.limits,
            token.clone(),
        )
        .await?;

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

/// Run `sql` on `database_id` to completion and return the full result.
///
/// This is the single shared execution path. It resolves the connection for
/// `database_id`, applies the row `limit` (Superset-style SQL wrap + fetch cap,
/// clamped to `limits.max_rows`), builds the connector via `factory`, and runs
/// the query to completion under the outer `limits.timeout`, returning the
/// coerced [`TabularResult`] (columns + row-major rows, values already typed by
/// the connector). Async engines (Trino) are polled to completion inside their
/// connector, so the caller gets finished rows from a single `await` — no
/// client-side polling handle.
///
/// Both the `submit`/[`JobManager`] path (which then persists the result) and
/// the BI data endpoints (which return the rows directly) call this, so there
/// is exactly one place that turns `(database_id, sql, limit)` into rows.
///
/// `cancel` is honored cooperatively by the connector; on the outer timeout it
/// is triggered so the engine query is aborted best-effort. Callers with no
/// cancellation concern can pass `CancellationToken::new()`.
#[allow(clippy::too_many_arguments)]
pub async fn run_query(
    databases: &DatabaseRepo,
    factory: &dyn ConnectorFactory,
    database_id: Uuid,
    sql: &str,
    limit: Option<usize>,
    session_user: Option<String>,
    limits: &Limits,
    cancel: CancellationToken,
) -> Result<TabularResult, EngineError> {
    let max_rows = limit
        .map(|l| l.clamp(1, limits.max_rows))
        .unwrap_or(limits.max_rows);
    let effective = Limits {
        max_rows,
        timeout: limits.timeout,
    };
    let effective_sql = apply_sql_limit(sql, limit, max_rows); // wrapped for SELECT

    let db = databases.get(database_id).await?;
    let secret = databases.open_secret(database_id).await?;
    let connector = factory.build(&db, secret, session_user, None).await?;

    let fut = connector.execute(&effective_sql, &effective, cancel.clone());
    match tokio::time::timeout(effective.timeout, fut).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err(EngineError::Timeout)
        }
    }
}

/// Wrap a single SELECT/WITH statement with a SQL `LIMIT n` (Superset-style) so the
/// engine returns exactly `n` rows. Non-SELECT / multi-statement SQL is returned
/// unchanged (the fetch-cap remains the backstop).
fn apply_sql_limit(sql: &str, limit: Option<usize>, max_rows: usize) -> String {
    let Some(l) = limit else {
        return sql.to_string();
    };
    let n = l.clamp(1, max_rows);
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    let is_select = lower.starts_with("select") || lower.starts_with("with");
    let single_statement = !trimmed.contains(';');
    if is_select && single_statement {
        format!("SELECT * FROM (\n{trimmed}\n) AS _egret_lim LIMIT {n}")
    } else {
        sql.to_string()
    }
}

#[cfg(test)]
mod limit_tests {
    use super::apply_sql_limit;
    #[test]
    fn wraps_single_select() {
        let out = apply_sql_limit("SELECT * FROM t", Some(5), 100);
        assert_eq!(
            out,
            "SELECT * FROM (\nSELECT * FROM t\n) AS _egret_lim LIMIT 5"
        );
    }
    #[test]
    fn wraps_with_cte_and_strips_trailing_semicolon() {
        let out = apply_sql_limit("WITH a AS (SELECT 1) SELECT * FROM a;", Some(10), 100);
        assert_eq!(
            out,
            "SELECT * FROM (\nWITH a AS (SELECT 1) SELECT * FROM a\n) AS _egret_lim LIMIT 10"
        );
    }
    #[test]
    fn clamps_to_max_rows() {
        let out = apply_sql_limit("SELECT 1", Some(999), 100);
        assert!(out.ends_with("LIMIT 100"));
    }
    #[test]
    fn does_not_wrap_show() {
        assert_eq!(
            apply_sql_limit("SHOW CATALOGS", Some(5), 100),
            "SHOW CATALOGS"
        );
    }
    #[test]
    fn does_not_wrap_multi_statement() {
        assert_eq!(
            apply_sql_limit("SELECT 1; SELECT 2", Some(5), 100),
            "SELECT 1; SELECT 2"
        );
    }
    #[test]
    fn no_limit_unchanged() {
        assert_eq!(apply_sql_limit("SELECT 1", None, 100), "SELECT 1");
    }
}
