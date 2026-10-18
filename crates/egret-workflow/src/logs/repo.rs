//! `workflow.task_logs`: batch insert, the cursor page, batch prune, and
//! `PurgeLogs`' scoped purge.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{LogLevel, LogLine, SETTLE_LAG};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Why [`LogRepo::insert_batch`] wrote nothing. Typed, not stringified, so
/// the writer can tell a batch that can never succeed from one worth a retry
/// without matching on message text.
#[derive(Debug, thiserror::Error)]
pub enum InsertError {
    /// SQLSTATE `23503` on `execution_id`: an execution the statement's join
    /// still saw was deleted by a transaction that committed before the
    /// foreign-key check ran. The whole statement is rolled back. Retrying
    /// the same batch is safe -- the join then drops that execution's lines
    /// -- and is the only way the other lines get written.
    #[error("an execution was deleted while its log batch was written: {0}")]
    ExecutionDeleted(#[source] sqlx::Error),
    /// SQLSTATE class `22` (data exception), e.g. `22021` for a U+0000 in a
    /// `text` value: some LINE of the batch can never be stored, whatever the
    /// database's health. Retrying the batch cannot help and would hold every
    /// other line of it hostage, so the writer isolates the offending line(s)
    /// instead (`logs::writer`).
    #[error("a log line cannot be stored as it is: {0}")]
    BadData(#[source] sqlx::Error),
    /// Anything else (connection lost, pool timed out, ...), unclassified.
    #[error("writing a log batch: {0}")]
    Db(#[source] sqlx::Error),
}

impl InsertError {
    fn classify(e: sqlx::Error) -> Self {
        let fk = e
            .as_database_error()
            .is_some_and(|d| d.is_foreign_key_violation());
        let data = e
            .as_database_error()
            .and_then(|d| d.code())
            .is_some_and(|c| c.starts_with("22"));
        if fk {
            InsertError::ExecutionDeleted(e)
        } else if data {
            InsertError::BadData(e)
        } else {
            InsertError::Db(e)
        }
    }

    /// The Postgres SQLSTATE, when the database answered with one.
    pub fn sqlstate(&self) -> Option<String> {
        let (InsertError::ExecutionDeleted(e) | InsertError::BadData(e) | InsertError::Db(e)) =
            self;
        e.as_database_error()
            .and_then(|d| d.code())
            .map(|c| c.into_owned())
    }
}

impl From<InsertError> for Error {
    fn from(e: InsertError) -> Self {
        Error::Internal(anyhow::Error::new(e))
    }
}

/// What [`LogRepo::page`] returns: `after` is the cursor (the last `id` the
/// caller has; `None` from the start), `limit` the most rows to return. The
/// caller bounds `limit` (the API's 500 default and 2,000 max are the route's).
#[derive(Clone, Debug, PartialEq)]
pub struct LogQuery {
    pub after: Option<i64>,
    pub limit: i64,
    pub task_id: Option<String>,
    pub task_run_id: Option<Uuid>,
    pub min_level: Option<LogLevel>,
}

/// A stored line: [`LogLine`]'s fields minus the two ids the caller queried
/// by, plus the row's `id` (the cursor).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredLine {
    pub id: i64,
    pub task_run_id: Option<Uuid>,
    pub task_id: Option<String>,
    pub attempt: Option<i32>,
    pub level: LogLevel,
    pub ts: DateTime<Utc>,
    pub message: String,
}

/// What [`LogRepo::purge`] deletes: `PurgeLogs`' scope (group C spec §2.3).
/// Every filter that is `None` is not applied; the workspace, the namespace
/// scope and `end` always are.
#[derive(Clone, Debug, PartialEq)]
pub struct PurgeQuery {
    pub workspace_id: Uuid,
    /// Each grant's namespace; a row matches when its execution's namespace is
    /// one of these or a dot-child of one (`ns = g OR ns LIKE g || '.%'` with
    /// `g` escaped for LIKE: `_` and `%` and `\`). With `flow_key` set the
    /// match is exact (`ns = g`), as Kestra's. Empty matches nothing.
    pub namespaces: Vec<String>,
    pub flow_key: Option<String>,
    pub execution_id: Option<Uuid>,
    pub levels: Option<Vec<LogLevel>>,
    pub start: Option<DateTime<Utc>>,
    pub end: DateTime<Utc>,
    /// `None`: one statement. `Some(n)`: repeat `LIMIT n` batches until one deletes fewer than n.
    pub batch: Option<i64>,
}

/// The lines [`LogRepo::purge`] matches, `l` joined to its execution `e`.
/// One macro so the single statement and the batch's subquery cannot
/// disagree; `concat!` keeps both `&'static str`. With a flow key (`$3`) the
/// namespace matches exactly, children only without one -- Kestra's rule
/// (`AbstractJdbcLogDataStore.buildDeleteCondition`). The window is
/// `[start, end]`, Kestra's `greaterOrEqual` / `lessOrEqual`. The LIKE pattern is
/// escaped in SQL (`\` first, then `%` and `_`), so a grant's text is never
/// a pattern: `a_b` does not reach `axb.c` (the `sql-like-underscore`
/// lesson). Both `e.workspace_id` and `l.workspace_id` are bound to `$1`.
macro_rules! purge_predicate {
    () => {
        "l.execution_id = e.id \
           AND e.workspace_id = $1 \
           AND l.workspace_id = $1 \
           AND EXISTS (SELECT 1 FROM unnest($2::text[]) AS g(ns) \
                        WHERE e.namespace = g.ns \
                           OR ($3::text IS NULL AND e.namespace LIKE \
                              replace(replace(replace(g.ns, '\\', '\\\\'), '%', '\\%'), '_', '\\_') \
                              || '.%' ESCAPE '\\')) \
           AND ($3::text IS NULL OR e.flow_key = $3) \
           AND ($4::uuid IS NULL OR l.execution_id = $4) \
           AND ($5::int2[] IS NULL OR l.level = ANY($5::int2[])) \
           AND ($6::timestamptz IS NULL OR l.ts >= $6) \
           AND l.ts <= $7"
    };
}

#[derive(Clone, Debug)]
pub struct LogRepo {
    pool: PgPool,
}

impl LogRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store `lines` in ONE statement (`INSERT ... SELECT unnest(...)`), so a
    /// batch is one short transaction -- what [`SETTLE_LAG`]'s bound assumes.
    /// Ids follow the slice's order. An empty slice writes nothing and takes
    /// no connection.
    ///
    /// **Only lines whose execution still exists AND belongs to the line's
    /// `workspace_id` are written**: the statement joins
    /// `workflow.executions` on both ids. So a line for an execution deleted
    /// since it was produced does not fail the whole batch on the foreign key,
    /// and a line carrying the wrong workspace can never become visible to
    /// that workspace (`page` filters on both ids). Returns the number
    /// written; **`returned < lines.len()` means the difference was dropped
    /// as orphaned or mismatched**, and the caller should count and log it.
    ///
    /// # Errors
    ///
    /// The join reads a snapshot, so an execution deleted by a transaction
    /// that commits after that snapshot but before the foreign-key check
    /// still fails the statement with `23503` (the insert waits on the
    /// delete's row lock, then finds the row gone). That is
    /// [`InsertError::ExecutionDeleted`]; nothing was written, and a retry
    /// drops the deleted execution's lines. A value Postgres refuses (SQLSTATE
    /// class `22`, e.g. a U+0000 byte) is [`InsertError::BadData`]; nothing was
    /// written. Any other failure is [`InsertError::Db`], with the
    /// `sqlx::Error` kept whole.
    pub async fn insert_batch(&self, lines: &[LogLine]) -> std::result::Result<u64, InsertError> {
        if lines.is_empty() {
            return Ok(0);
        }
        let n = lines.len();
        let mut workspace_ids = Vec::with_capacity(n);
        let mut execution_ids = Vec::with_capacity(n);
        let mut task_run_ids = Vec::with_capacity(n);
        let mut task_ids = Vec::with_capacity(n);
        let mut attempts = Vec::with_capacity(n);
        let mut levels = Vec::with_capacity(n);
        let mut tss = Vec::with_capacity(n);
        let mut messages = Vec::with_capacity(n);
        for l in lines {
            workspace_ids.push(l.workspace_id);
            execution_ids.push(l.execution_id);
            task_run_ids.push(l.task_run_id);
            task_ids.push(l.task_id.as_deref());
            attempts.push(l.attempt);
            levels.push(l.level.as_i16());
            tss.push(l.ts);
            messages.push(l.message.as_str());
        }
        let done = sqlx::query(
            "INSERT INTO workflow.task_logs \
               (workspace_id, execution_id, task_run_id, task_id, attempt, level, ts, message) \
             SELECT l.w, l.e, l.r, l.t, l.a, l.lv, l.ts, l.m \
               FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::text[], \
                           $5::int4[], $6::int2[], $7::timestamptz[], $8::text[]) \
                    WITH ORDINALITY AS l(w, e, r, t, a, lv, ts, m, ord) \
               JOIN workflow.executions x ON x.id = l.e AND x.workspace_id = l.w \
              ORDER BY l.ord",
        )
        .bind(workspace_ids)
        .bind(execution_ids)
        .bind(task_run_ids)
        .bind(task_ids)
        .bind(attempts)
        .bind(levels)
        .bind(tss)
        .bind(messages)
        .execute(&self.pool)
        .await
        .map_err(InsertError::classify)?;
        Ok(done.rows_affected())
    }

    /// The lines of `execution` in workspace `ws` after the cursor, in `id`
    /// order, at most `q.limit` of them, narrowed by the optional filters.
    ///
    /// **Both ids are in the predicate**: an execution id queried under the
    /// wrong workspace returns nothing, which the route turns into a 404.
    ///
    /// **A watermark, not a per-row filter: the page stops before the first
    /// row after the cursor that has not settled** (`inserted_at` younger
    /// than [`SETTLE_LAG`], the database's clock), whatever the filters. Rows
    /// with a higher id are held back even when they have settled, so the
    /// cursor can never pass a lower id that is still settling. Without that a
    /// reader can move past a lower id that commits late and never see that
    /// line -- see [`SETTLE_LAG`] and `tests/task_logs.rs`'s
    /// `out_of_order_commits_are_not_skipped_by_a_polling_reader` and
    /// `the_page_stops_at_the_first_unsettled_id`.
    pub async fn page(&self, ws: Uuid, execution: Uuid, q: &LogQuery) -> Result<Vec<StoredLine>> {
        let rows = sqlx::query(
            "SELECT id, task_run_id, task_id, attempt, level, ts, message \
               FROM workflow.task_logs \
              WHERE workspace_id = $1 \
                AND execution_id = $2 \
                AND id > COALESCE($3, 0) \
                AND id < COALESCE( \
                      (SELECT min(u.id) FROM workflow.task_logs u \
                        WHERE u.workspace_id = $1 \
                          AND u.execution_id = $2 \
                          AND u.id > COALESCE($3, 0) \
                          AND u.inserted_at >= now() - make_interval(secs => $4)), \
                      9223372036854775807) \
                AND ($5::text IS NULL OR task_id = $5) \
                AND ($6::uuid IS NULL OR task_run_id = $6) \
                AND ($7::int2 IS NULL OR level >= $7) \
              ORDER BY id \
              LIMIT $8",
        )
        .bind(ws)
        .bind(execution)
        .bind(q.after)
        .bind(SETTLE_LAG.as_secs_f64())
        .bind(q.task_id.as_deref())
        .bind(q.task_run_id)
        .bind(q.min_level.map(LogLevel::as_i16))
        .bind(q.limit.max(0))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(stored).collect()
    }

    /// The `ts` of the newest stored line of one task-run attempt, `None` when
    /// it has none.
    ///
    /// W5a Task 4: the Kubernetes runner's re-attach follows `main`'s log from
    /// here (`sinceTime`), so a worker that takes over a live pod repeats few of
    /// the lines its predecessor already stored (spec §5, *Re-attach*).
    /// `attempt` is the STORED numbering, 0-based
    /// ([`RunIdentity::stored_attempt`](super::RunIdentity::stored_attempt)).
    /// `execution_id` is in the predicate so the `(execution_id, id)` index
    /// narrows the scan; `task_run_id` has no index of its own.
    pub async fn last_ts(
        &self,
        execution: Uuid,
        task_run: Uuid,
        attempt: i32,
    ) -> Result<Option<DateTime<Utc>>> {
        sqlx::query_scalar(
            "SELECT max(ts) FROM workflow.task_logs \
              WHERE execution_id = $1 AND task_run_id = $2 AND attempt = $3",
        )
        .bind(execution)
        .bind(task_run)
        .bind(attempt)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    /// The messages of one task-run attempt stored at or after `from`, in
    /// `id` order, at most `limit` of them. W5a Task 4 fix round 1: a
    /// Kubernetes re-attach drops the overlap it replays by exact text.
    pub async fn messages_since(
        &self,
        execution: Uuid,
        task_run: Uuid,
        attempt: i32,
        from: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT message FROM workflow.task_logs \
              WHERE execution_id = $1 AND task_run_id = $2 AND attempt = $3 AND ts >= $4 \
              ORDER BY id LIMIT $5",
        )
        .bind(execution)
        .bind(task_run)
        .bind(attempt)
        .bind(from)
        .bind(limit.max(0))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    /// Delete at most `batch` lines produced (`ts`) more than
    /// `older_than_days` days ago; returns how many went. The retention
    /// ticker calls this until it returns less than `batch`.
    ///
    /// `older_than_days <= 0` is refused with [`Error::InvalidInput`] and
    /// deletes nothing. As a window, `0` would delete every line produced
    /// before now and a negative value lines stamped in the future, while the
    /// configuration's `log_retention_days = 0` means "keep forever" -- so the
    /// ticker's mistake of passing it through is an error, not a wipe.
    pub async fn prune_batch(&self, older_than_days: i32, batch: i64) -> Result<u64> {
        if older_than_days <= 0 {
            return Err(Error::InvalidInput(format!(
                "prune_batch: older_than_days must be positive, got {older_than_days} \
                 (0 means keep forever and must not reach the pruner)"
            )));
        }
        let done = sqlx::query(
            "DELETE FROM workflow.task_logs \
              WHERE id IN ( \
                SELECT id FROM workflow.task_logs \
                 WHERE ts < now() - make_interval(days => $1) \
                 LIMIT $2)",
        )
        .bind(older_than_days)
        .bind(batch.max(0))
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected())
    }

    /// `PurgeLogs`' delete (group C spec §2.3): the lines of `q.workspace_id`'s
    /// executions in `q.namespaces`' scope, narrowed by every filter that is
    /// set, with `ts` in `[start, end]` (Kestra's bounds, both inclusive).
    /// With `q.flow_key` the namespace matches exactly; without it, children
    /// too. Returns how many rows went.
    ///
    /// With `q.batch = Some(n)` the work is statements of at most `n` rows
    /// each, repeated until one deletes fewer than `n`; a failure part-way
    /// keeps what the earlier batches deleted. An `end` in the future over a
    /// busy namespace can keep the loop going while new lines arrive, each
    /// batch finding `n` more; Kestra's MySQL loop has the same property. `n < 1` is refused with
    /// [`Error::InvalidInput`] before anything is deleted (`LIMIT 0` would
    /// never end the loop).
    ///
    /// The caller decides the scope (`nsfiles::access::authorize` with
    /// `Resource::Logs`); this only applies it. Every value is bound.
    pub async fn purge(&self, q: &PurgeQuery) -> Result<u64> {
        let levels: Option<Vec<i16>> = q
            .levels
            .as_ref()
            .map(|ls| ls.iter().map(|l| l.as_i16()).collect());
        let Some(batch) = q.batch else {
            let done = sqlx::query(concat!(
                "DELETE FROM workflow.task_logs l USING workflow.executions e WHERE ",
                purge_predicate!()
            ))
            .bind(q.workspace_id)
            .bind(&q.namespaces)
            .bind(q.flow_key.as_deref())
            .bind(q.execution_id)
            .bind(levels.as_ref())
            .bind(q.start)
            .bind(q.end)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
            return Ok(done.rows_affected());
        };
        if batch < 1 {
            return Err(Error::InvalidInput(format!(
                "purge: batch must be 1 or more, got {batch}"
            )));
        }
        let mut total = 0u64;
        loop {
            let done = sqlx::query(concat!(
                "DELETE FROM workflow.task_logs WHERE id IN ( \
                   SELECT l.id FROM workflow.task_logs l, workflow.executions e WHERE ",
                purge_predicate!(),
                " LIMIT $8)"
            ))
            .bind(q.workspace_id)
            .bind(&q.namespaces)
            .bind(q.flow_key.as_deref())
            .bind(q.execution_id)
            .bind(levels.as_ref())
            .bind(q.start)
            .bind(q.end)
            .bind(batch)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
            let n = done.rows_affected();
            total += n;
            if n < batch as u64 {
                return Ok(total);
            }
        }
    }
}

fn stored(row: &sqlx::postgres::PgRow) -> Result<StoredLine> {
    let level: i16 = row.try_get("level").map_err(db_err)?;
    Ok(StoredLine {
        id: row.try_get("id").map_err(db_err)?,
        task_run_id: row.try_get("task_run_id").map_err(db_err)?,
        task_id: row.try_get("task_id").map_err(db_err)?,
        attempt: row.try_get("attempt").map_err(db_err)?,
        level: LogLevel::from_i16(level).ok_or_else(|| {
            Error::Internal(anyhow::anyhow!(
                "workflow.task_logs.level {level} out of range"
            ))
        })?,
        ts: row.try_get("ts").map_err(db_err)?,
        message: row.try_get("message").map_err(db_err)?,
    })
}
