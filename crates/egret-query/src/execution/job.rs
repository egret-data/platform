//! JobManager: async query execution with a Postgres-persisted lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::registry::DatabaseRepo;
use egret_storage::ObjectStore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::execution::connector::ConnectorFactory;
use crate::execution::error::EngineError;
use crate::execution::limit::{apply_sql_limit, unshift_error_lines};
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
    #[allow(clippy::too_many_arguments)]
    pub async fn submit(
        &self,
        ws: Uuid,
        database_id: Uuid,
        user: Option<Uuid>,
        session_user: Option<String>,
        bearer_token: Option<String>,
        sql: &str,
        limit: Option<usize>,
    ) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?; // original sql stored
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let run_sql = sql.to_string();
        tokio::spawn(async move {
            this.run(
                id,
                database_id,
                run_sql,
                limit,
                session_user,
                bearer_token,
                token,
            )
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

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        id: Uuid,
        database_id: Uuid,
        sql: String,
        limit: Option<usize>,
        session_user: Option<String>,
        bearer_token: Option<String>,
        token: CancellationToken,
    ) {
        if let Err(e) = self.store.set_running(id).await {
            tracing::warn!(%id, error=%e, "failed to mark query running");
            return;
        }
        match self
            .execute(
                id,
                database_id,
                &sql,
                limit,
                session_user,
                bearer_token,
                &token,
            )
            .await
        {
            Ok(()) => {} // finish() already persisted inside execute()
            Err(EngineError::Cancelled) => {
                let _ = self.store.set_cancelled(id).await;
            }
            Err(EngineError::Timeout) => {
                let _ = self
                    .store
                    .fail(id, &timeout_message(self.limits.timeout, &sql))
                    .await;
            }
            // Trino's own stop at the same limit (egret sends it as
            // `query_max_run_time`): its words, plus where the limit comes from.
            Err(EngineError::Engine(m)) if m.contains("exceeded maximum time limit") => {
                let msg = format!(
                    "engine: {m}. The limit is SQL Lab's {} s, the server setting `engine.query_timeout_secs`.{}",
                    self.limits.timeout.as_secs(),
                    ddl_may_still_complete(&sql)
                );
                let _ = self.store.fail(id, &msg).await;
            }
            Err(e) => {
                let _ = self.store.fail(id, &e.to_string()).await;
            }
        }
    }

    /// Run the query to completion via the shared `run_query` path, write the
    /// result, persist finished.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        id: Uuid,
        database_id: Uuid,
        sql: &str,
        limit: Option<usize>,
        session_user: Option<String>,
        bearer_token: Option<String>,
        token: &CancellationToken,
    ) -> Result<(), EngineError> {
        let result = run_query(
            &self.databases,
            &*self.factory,
            database_id,
            sql,
            limit,
            session_user,
            bearer_token,
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
/// `database_id`, applies the row `limit` (appended to the query, see
/// `execution::limit`, plus the connector's fetch cap, clamped to
/// `limits.max_rows`), builds the connector via `factory`, and runs
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
    bearer_token: Option<String>,
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
    let db = databases.get(database_id).await?;
    let limited = apply_sql_limit(sql, db.engine, limit, max_rows);
    let secret = databases.open_secret(database_id).await?;
    let connector = factory
        .build(&db, secret, session_user, bearer_token)
        .await?;

    let fut = connector.execute(&limited.sql, &effective, cancel.clone());
    match tokio::time::timeout(effective.timeout, fut).await {
        Ok(Err(EngineError::Engine(m))) if limited.wrapped => {
            Err(EngineError::Engine(unshift_error_lines(&m)))
        }
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err(EngineError::Timeout)
        }
    }
}

/// What a SQL Lab run that outran its time limit says (KI-211): the limit, and
/// the setting an operator raises it with. A bare "timeout" said neither.
fn timeout_message(limit: std::time::Duration, sql: &str) -> String {
    format!(
        "timeout: the query ran longer than the {} s limit for SQL Lab queries and was stopped. \
         The limit is the server setting `engine.query_timeout_secs`.{}",
        limit.as_secs(),
        ddl_may_still_complete(sql)
    )
}

/// KI-318: SQL Lab stops WAITING at its limit, but a schema change it sent may
/// still commit: egret's catalog holds a rename of a classified or restricted
/// table for up to 90 s while the engines confirm the moved restrictions, and
/// the engine finishes the statement after SQL Lab gave up. So the timeout of a
/// DDL statement says the change may still complete and where to look before
/// running it again. A query's timeout is unchanged: a stopped SELECT changed
/// nothing. Empty for anything that is not DDL.
fn ddl_may_still_complete(sql: &str) -> &'static str {
    if is_ddl(sql) {
        " This is a schema change: SQL Lab stopped waiting for it, but the engine may still \
         complete it (a rename can be held while the engines confirm the moved restrictions). \
         Check Governance → Recent changes, or list the tables, before you run it again."
    } else {
        ""
    }
}

/// Whether the statement's first keyword, after comments and whitespace, starts
/// a DDL statement. Tokenized (not a prefix match), so a leading `-- ALTER`
/// comment or `"alter"` identifier does not count; text the tokenizer cannot
/// read is not DDL, which keeps the old message.
fn is_ddl(sql: &str) -> bool {
    use sqlparser::dialect::GenericDialect;
    use sqlparser::tokenizer::{Token, Tokenizer};
    let Ok(tokens) = Tokenizer::new(&GenericDialect {}, sql).tokenize() else {
        return false;
    };
    let first = tokens
        .into_iter()
        .find(|t| !matches!(t, Token::Whitespace(_)));
    matches!(
        first,
        Some(Token::Word(w)) if w.quote_style.is_none()
            && ["ALTER", "CREATE", "DROP", "RENAME", "TRUNCATE"]
                .iter()
                .any(|k| w.value.eq_ignore_ascii_case(k))
    )
}

#[cfg(test)]
mod ddl_tests {
    use super::*;

    #[test]
    fn ddl_is_recognised_by_its_first_keyword() {
        for sql in [
            "ALTER TABLE a RENAME TO b",
            "  -- rename\n alter table a rename to b",
            "/* x */ CREATE TABLE t (i int)",
            "drop view v",
            "RENAME TABLE a TO b",
            "TRUNCATE TABLE t",
        ] {
            assert!(is_ddl(sql), "{sql}");
        }
        for sql in [
            "SELECT 1",
            "-- ALTER TABLE a\nSELECT 1",
            "\"alter\" ",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "INSERT INTO t VALUES (1)",
            "",
        ] {
            assert!(!is_ddl(sql), "{sql}");
        }
    }

    #[test]
    fn only_a_ddl_timeout_points_to_recent_changes() {
        let limit = std::time::Duration::from_secs(60);
        let ddl = timeout_message(limit, "ALTER TABLE a RENAME TO b");
        assert!(
            ddl.contains("60 s") && ddl.contains("may still complete"),
            "{ddl}"
        );
        assert!(ddl.contains("Recent changes"), "{ddl}");
        let query = timeout_message(limit, "SELECT 1");
        assert!(!query.contains("Recent changes"), "{query}");
    }
}
