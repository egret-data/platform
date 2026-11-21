//! Handing leases back at a graceful stop (KI-237).
//!
//! [`reap`](super::reap) owns the hard crash: a replica that dies leaves its
//! leases to expire after `working_timeout`. A **graceful** stop is different
//! and cheaper to get right: the process knows which runs it holds, so it can
//! give exactly those back and the next claim, by this replica after its
//! restart or by another, picks them up at once instead of an hour later.
//!
//! # Why a held-lease book and not a node id on the row
//!
//! A `claimed_by` column would also cover SIGKILL, but only if a restarted
//! process kept the node's identity, which a Kubernetes Deployment's pods do
//! not. Reclaiming "the previous incarnation's" leases on a guessed identity is
//! exactly the thing that hands a *live* worker's run to a second handler.
//! Releasing only what this process itself holds is safe by construction, and
//! needs no migration and no change to the claim or renew statements.
//!
//! # Safety: never release another worker's lease
//!
//! The book maps a run to the **latest** `claimed_at` this process wrote for it
//! (the claim's, then each renewal's: `claim::renew_lease` updates it), and the
//! release is fenced on that exact value, as every other write under a lease
//! is. A run this process lost (reaped, then re-claimed by someone else) has a
//! newer `claimed_at` on its row, so the fence matches nothing and the other
//! worker's lease stands. A release that races a renewal that landed after the
//! book was read matches nothing either, and the reaper stays the backstop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::PgPool;
use uuid::Uuid;

tokio::task_local! {
    /// The book of the run task being executed, so `renew_lease` can record the
    /// lease it moved without every caller between it and the claim loop
    /// carrying one. Unset outside the claim loop (tests, `tick`), where
    /// recording is a no-op.
    static HELD: HeldLeases;
}

/// The runs this process holds a lease on, and the `claimed_at` of each.
#[derive(Clone, Debug, Default)]
pub struct HeldLeases(Arc<Mutex<HashMap<Uuid, DateTime<Utc>>>>);

impl HeldLeases {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, DateTime<Utc>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A claim has just taken `run` at `claimed_at`.
    pub fn hold(&self, run: Uuid, claimed_at: DateTime<Utc>) {
        self.map().insert(run, claimed_at);
    }

    /// The run's task has ended, one way or another; it is no longer held here.
    pub fn forget(&self, run: Uuid) {
        self.map().remove(&run);
    }

    /// How many runs are held.
    pub fn len(&self) -> usize {
        self.map().len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.map().is_empty()
    }

    /// Run `work` (one claimed run) with this book as the one renewals record
    /// into.
    pub async fn scoped<F: std::future::Future>(self, work: F) -> F::Output {
        HELD.scope(self, work).await
    }

    /// A renewal moved `run`'s lease to `claimed_at`. Only a run already held
    /// is updated, so a late renewal cannot re-add a forgotten one.
    pub(super) fn renewed(run: Uuid, claimed_at: DateTime<Utc>) {
        let _ = HELD.try_with(|book| {
            if let Some(held) = book.map().get_mut(&run) {
                *held = claimed_at;
            }
        });
    }

    /// Clear the lease of every held run that is still leased at exactly the
    /// `claimed_at` this process wrote, in one bounded statement, and empty the
    /// book. Returns how many rows were released.
    ///
    /// State is left as it is and the attempt stays open, as [`reap`](super::reap)
    /// leaves them: the run was never finished, and the next claim runs the same
    /// attempt again (a script task re-attaches to its pod or container).
    pub async fn release(&self, pool: &PgPool) -> Result<usize> {
        let held: Vec<(Uuid, DateTime<Utc>)> = self.map().drain().collect();
        if held.is_empty() {
            return Ok(0);
        }
        let (ids, at): (Vec<Uuid>, Vec<DateTime<Utc>>) = held.into_iter().unzip();
        let n = sqlx::query(
            "UPDATE workflow.task_runs t \
                SET claimed_at = NULL, updated_at = now() \
               FROM unnest($1::uuid[], $2::timestamptz[]) AS h(id, claimed_at) \
              WHERE t.id = h.id \
                AND t.claimed_at = h.claimed_at \
                AND t.state IN ('RUNNING', 'KILLING')",
        )
        .bind(ids)
        .bind(at)
        .execute(pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("{e}")))?
        .rows_affected();
        Ok(n as usize)
    }
}
