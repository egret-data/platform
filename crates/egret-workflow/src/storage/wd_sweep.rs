//! Workflow debt D6 (`docs/en/specs/2026-10-19-workflow-debt-design.md`
//! §3.1): **the `_wd` crash sweep** -- a `WorkingDirectory` block's
//! `_wd/{block}/` snapshots that no discard removed.
//!
//! A block's snapshots ([`FileStore::working_dir_prefix`]) are removed by
//! `worker::working_dir::discard` when its finish claim runs, which then
//! stamps `workflow.task_runs.working_dir_swept_at` (migration 0196). Three
//! things leave them behind, unstamped:
//! - a crash: the execution ended without the block's finish claim;
//! - a kill that closed the execution by force (debt D4,
//!   `worker::force_close`), which never plans and so never discards;
//! - a discard whose `delete_prefix` failed.
//!
//! [`sweep`] -- run hourly by [`spawn_wd_sweeper`] -- removes the snapshots
//! of every unstamped block run whose execution is terminal and whose run
//! ended more than [`WdSweepConfig::older_than`] (24 hours) ago, then stamps
//! it, so each block is listed once rather than on every pass. The age is
//! the database's clock. The selection reads the block's `working_dir` and
//! its execution's state, never the run's own state: a `KILLED` block closed
//! by force is swept like a `FAILED` one.

use std::sync::Arc;
use std::time::Duration;

use egret_core::{Error, Result};
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::lock::LockedSession;
use super::FileStore;

/// The `pg_try_advisory_lock` key a pass takes, so one `_wd` sweep runs per
/// database at a time ("egretwds" as eight ASCII bytes).
pub const WD_SWEEP_LOCK_KEY: i64 = 0x6567_7265_7477_6473;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// The `_wd` sweep's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WdSweepConfig {
    /// Between passes.
    pub every: Duration,
    /// A block run that ended longer ago than this, in a terminal execution,
    /// has nothing left that reads its snapshots.
    pub older_than: Duration,
    /// Rows per query.
    pub batch: i64,
}

impl Default for WdSweepConfig {
    /// Hourly, 24 hours, 100 rows per query.
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3600),
            older_than: Duration::from_secs(24 * 3600),
            batch: 100,
        }
    }
}

/// Stamp block run `block`'s snapshots as removed. Called after a
/// `delete_prefix` that succeeded, by the discard and by [`sweep`].
pub(crate) async fn stamp(pool: &PgPool, block: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE workflow.task_runs SET working_dir_swept_at = now() \
          WHERE id = $1 AND working_dir_swept_at IS NULL",
    )
    .bind(block)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// One pass: every unstamped block run of a terminal execution that ended
/// more than `cfg.older_than` ago -- its `_wd/{block}/` deleted, then
/// stamped. Answers how many blocks it swept. A store failure ends the pass
/// (logged; the row stays unstamped for the next one); `Err` is the
/// database's.
///
/// **One sweep per database.** The pass first takes the session-level
/// advisory lock [`WD_SWEEP_LOCK_KEY`] with `pg_try_advisory_lock`; when
/// another replica holds it, the pass does nothing and answers 0.
pub async fn sweep(pool: &PgPool, fs: &FileStore, cfg: &WdSweepConfig) -> Result<u64> {
    let Some(session) = LockedSession::try_acquire(pool, WD_SWEEP_LOCK_KEY).await? else {
        tracing::debug!("the _wd sweep pass skipped: another sweep holds the lock");
        return Ok(0);
    };
    let result = sweep_locked(pool, fs, cfg).await;
    if let Err(e) = session.unlock().await {
        tracing::warn!(error = %e, "the _wd sweep could not release its lock; its connection is closed");
    }
    result
}

async fn sweep_locked(pool: &PgPool, fs: &FileStore, cfg: &WdSweepConfig) -> Result<u64> {
    let mut swept = 0u64;
    let mut done: Vec<Uuid> = Vec::new();
    loop {
        let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT t.id, e.id, e.workspace_id \
               FROM workflow.task_runs t JOIN workflow.executions e ON e.id = t.execution_id \
              WHERE t.working_dir IS NOT NULL AND t.working_dir_swept_at IS NULL \
                AND workflow.state_is_terminal(e.state) \
                AND COALESCE(t.ended_at, t.updated_at) < now() - make_interval(secs => $1) \
                AND NOT (t.id = ANY($2)) \
              ORDER BY COALESCE(t.ended_at, t.updated_at), t.id LIMIT $3",
        )
        .bind(cfg.older_than.as_secs_f64())
        .bind(&done)
        .bind(cfg.batch)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        let short = (rows.len() as i64) < cfg.batch;
        for (block, execution, workspace) in rows {
            done.push(block);
            let prefix = fs.working_dir_prefix(workspace, execution, block);
            if let Err(e) = fs.store.delete_prefix(&prefix).await {
                tracing::warn!(key = %prefix, error = %e, "the _wd sweep could not delete a block's snapshots; the next pass retries");
                return Ok(swept);
            }
            stamp(pool, block).await?;
            swept += 1;
        }
        if short {
            return Ok(swept);
        }
    }
}

/// Start the hourly `_wd` sweep. Stops on `cancel`, between passes. Every
/// replica may run one: the lock makes one pass per database at a time, and
/// a second delete and stamp of one block are no-ops.
pub fn spawn_wd_sweeper(
    pool: PgPool,
    fs: Arc<FileStore>,
    cfg: WdSweepConfig,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => match sweep(&pool, &fs, &cfg).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(swept = n, "WorkingDirectory snapshots a crash left were swept"),
                    Err(e) => tracing::warn!(error = %e, "the WorkingDirectory snapshot sweep failed"),
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key is "egretwds" in ASCII, and differs from every other advisory
    /// lock constant in the workspace, so the sweep never waits on, or
    /// shuts out, another lock's holder.
    #[test]
    fn the_lock_key_is_its_own() {
        assert_eq!(WD_SWEEP_LOCK_KEY.to_be_bytes(), *b"egretwds");
        for other in [
            crate::storage::prune::PRUNE_LOCK_KEY,
            0x4547_5245_5453_4b31, // egret-machine-idp SIGNING_KEY_CREATE_LOCK
            0x_E6E7_C0DE_6A11,     // egret-auth ROLE_GRAPH_LOCK_KEY
        ] {
            assert_ne!(WD_SWEEP_LOCK_KEY, other);
        }
    }

    /// The spec's cadence: hourly, 24 hours, 100 rows.
    #[test]
    fn the_default_config_is_the_specs() {
        let cfg = WdSweepConfig::default();
        assert_eq!(cfg.every, Duration::from_secs(3600));
        assert_eq!(cfg.older_than, Duration::from_secs(24 * 3600));
        assert_eq!(cfg.batch, 100);
    }
}
