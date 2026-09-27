//! W11: the retention ticker for `workflow.task_logs` (spec §7, D2).
//!
//! One global setting, `workflow.log_retention_days` (default 30; **`0` keeps
//! lines for ever**, so no pruner is started). Every [`PruneConfig::every`]
//! (an hour), with `MissedTickBehavior::Skip`, one pass calls
//! [`LogRepo::prune_batch`] batch after batch until a batch deletes fewer than
//! [`PruneConfig::batch`] rows or the pass has run for
//! [`PruneConfig::pass_budget`]. Each batch is one short `DELETE`, so no
//! statement holds locks for long, and replicas pruning together are harmless:
//! deleting a row twice is a no-op.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::LogRepo;

/// The pruner's settings. [`PruneConfig::new`] fills in the spec's cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneConfig {
    /// Lines produced more than this many days ago are deleted. `0` means
    /// keep for ever: [`spawn_pruner`] starts nothing.
    pub retention_days: i32,
    /// Between passes.
    pub every: Duration,
    /// Rows per `DELETE`.
    pub batch: i64,
    /// A pass stops starting new batches once it has run this long.
    pub pass_budget: Duration,
}

impl PruneConfig {
    /// `retention_days`, pruned every hour in batches of 10,000 with a 30 s
    /// pass budget.
    pub fn new(retention_days: i32) -> Self {
        Self {
            retention_days,
            every: Duration::from_secs(3600),
            batch: 10_000,
            pass_budget: Duration::from_secs(30),
        }
    }
}

/// Start the pruner, or return `None` when `cfg.retention_days` is `0` (keep
/// for ever) or negative (refused and logged; nothing is pruned). Stops on
/// `cancel`, between batches.
pub fn spawn_pruner(
    repo: LogRepo,
    cfg: PruneConfig,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    if cfg.retention_days == 0 {
        tracing::info!("workflow log retention is 0: log lines are kept for ever, no pruner");
        return None;
    }
    if cfg.retention_days < 0 {
        tracing::warn!(
            retention_days = cfg.retention_days,
            "workflow log retention is negative; no pruner started"
        );
        return None;
    }
    tracing::info!(
        retention_days = cfg.retention_days,
        every_secs = cfg.every.as_secs(),
        "workflow log pruner started"
    );
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => pass(&repo, &cfg, &cancel).await,
            }
        }
        tracing::info!("workflow log pruner stopped");
    }))
}

/// One pass: batches until a short one, the budget, cancellation or an error.
async fn pass(repo: &LogRepo, cfg: &PruneConfig, cancel: &CancellationToken) {
    let start = Instant::now();
    let mut deleted = 0u64;
    while !cancel.is_cancelled() {
        match repo.prune_batch(cfg.retention_days, cfg.batch).await {
            Ok(n) => {
                deleted += n;
                if (n as i64) < cfg.batch || start.elapsed() >= cfg.pass_budget {
                    break;
                }
            }
            // The next pass tries again; the lines are only older by then.
            Err(e) => {
                tracing::warn!(error = %e, "workflow log prune batch failed");
                break;
            }
        }
    }
    if deleted > 0 {
        tracing::info!(
            deleted,
            elapsed_ms = start.elapsed().as_millis() as u64,
            "workflow log lines pruned"
        );
    }
}
