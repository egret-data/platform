//! `workflow.task_logs`: batch insert, the cursor page, batch prune.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{LogLevel, LogLine, SETTLE_LAG};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
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
    /// Ids follow the slice's order. Returns the rows written; an empty slice
    /// writes nothing and takes no connection.
    pub async fn insert_batch(&self, lines: &[LogLine]) -> Result<u64> {
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
             SELECT w, e, r, t, a, lv, ts, m \
               FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::text[], \
                           $5::int4[], $6::int2[], $7::timestamptz[], $8::text[]) \
                    WITH ORDINALITY AS l(w, e, r, t, a, lv, ts, m, ord) \
              ORDER BY ord",
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
        .map_err(db_err)?;
        Ok(done.rows_affected())
    }

    /// The lines of `execution` in workspace `ws` after the cursor, in `id`
    /// order, at most `q.limit` of them, narrowed by the optional filters.
    ///
    /// **Both ids are in the predicate**: an execution id queried under the
    /// wrong workspace returns nothing, which the route turns into a 404.
    ///
    /// **Only rows older than [`SETTLE_LAG`] are returned** (by `inserted_at`,
    /// the database's clock). Without it a reader can move its cursor past a
    /// lower id that commits later and never see that line -- see
    /// [`SETTLE_LAG`] and `tests/task_logs.rs`'s
    /// `out_of_order_commits_are_not_skipped_by_a_polling_reader`.
    pub async fn page(&self, ws: Uuid, execution: Uuid, q: &LogQuery) -> Result<Vec<StoredLine>> {
        let rows = sqlx::query(
            "SELECT id, task_run_id, task_id, attempt, level, ts, message \
               FROM workflow.task_logs \
              WHERE workspace_id = $1 \
                AND execution_id = $2 \
                AND id > COALESCE($3, 0) \
                AND inserted_at < now() - make_interval(secs => $4) \
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

    /// Delete at most `batch` lines produced (`ts`) more than
    /// `older_than_days` days ago; returns how many went. The retention
    /// ticker calls this until it returns less than `batch`.
    ///
    /// **`0` is not "keep forever"**: `prune_batch(0, _)` deletes every line
    /// produced before now. Keep-forever (`log_retention_days = 0`) is the
    /// ticker's decision, and it must not call this with `0`.
    pub async fn prune_batch(&self, older_than_days: i32, batch: i64) -> Result<u64> {
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
