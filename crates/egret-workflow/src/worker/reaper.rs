//! Reclaiming leases a worker did not release.
//!
//! This is the failure the claim's design cannot prevent, and
//! `crates/egret-automation/src/scheduler.rs`'s module doc states it for the
//! identical statement: a replica that dies mid-run leaves its row leased
//! forever and nothing else would ever pick it up.
//!
//! It is a **live** path here rather than a precaution, and that is a
//! consequence of the two-commit design the `worker` module doc gives: the
//! claim commits on its own, so a worker killed between the claim and the
//! commit leaves a durable lease behind. With one transaction end to end the
//! crash would roll the claim back too and there would be nothing to reclaim —
//! which is the shape of a guard nobody can ever see fail.

use egret_core::{Error, Result};
use sqlx::PgPool;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Clear every lease older than `timeout`, returning how many were reclaimed.
///
/// `timeout` is a parameter rather than
/// [`DEFAULT_WORKING_TIMEOUT`](super::DEFAULT_WORKING_TIMEOUT) so a test can
/// drive it, which is what the plan's interface asks for.
///
/// **Both directions matter and only one of them is obvious.** Reclaiming a
/// lease past the timeout is what unsticks a dead worker's run; *not*
/// reclaiming one inside it is what stops a live worker's run being handed to a
/// second handler while the first is still in its handler. A predicate widened
/// to everything passes the first test and fails the second, which is why
/// `reap_reclaims_the_expired_lease_and_leaves_the_live_one` asserts both.
///
/// The run's state is left as it is and its attempt is left open: the run
/// did not finish, it was never told anything, and the next claim starts the
/// same attempt again. Failing it here would be this module inventing a
/// terminal `advance()` never asked for.
///
/// # `KILLING` leases are reclaimed too (workflow kill spec §4.3)
///
/// A worker can die, or give its lease up, while its run is `KILLING`: a
/// leaf the kill reached under its lease, or a `WorkingDirectory` block's
/// discard-only finish (`KILLING`, `FINISHING`). Nothing renews such a lease
/// (`renew_lease` is fenced on the state the holder expects), so without this
/// arm the run would stay `KILLING` under a dead lease for ever and its
/// execution would never end. Cleared, it is claimable again: a leaf as a
/// kill-only claim, a block as its discard-only finish (`claim::claim`).
/// Driven by `the_reaper_frees_an_expired_killing_lease` (`tests/worker.rs`)
/// and `a_reaped_discard_only_block_claim_is_claimed_again_and_discards`
/// (`tests/working_dir_claims.rs`).
pub async fn reap(pool: &PgPool, timeout: std::time::Duration) -> Result<usize> {
    let secs = timeout.as_secs_f64();
    let n = sqlx::query(
        "UPDATE workflow.task_runs \
            SET claimed_at = NULL, updated_at = now() \
          WHERE claimed_at IS NOT NULL \
            AND state IN ('RUNNING', 'KILLING') \
            AND claimed_at < now() - make_interval(secs => $1)",
    )
    .bind(secs)
    .execute(pool)
    .await
    .map_err(db_err)?
    .rows_affected();
    Ok(n as usize)
}
