//! Flow concurrency (package 8a): the admission gate.
//!
//! A flow with `concurrency: {limit: N, behavior: QUEUE}` runs at most `N`
//! executions at once; a start over the limit is parked in `QUEUED`.
//!
//! **Scope is `executions.flow_id`**, the flow's stable surrogate (one per
//! logical flow, shared by every version), which is Kestra's per-logical-flow
//! scope. It is NULL after the flow is deleted; such a start never gates.
//!
//! **Race-safety.** [`concurrency_admit`] takes a transaction-scoped advisory
//! lock keyed on the flow, then counts. The caller inserts its row in the SAME
//! transaction, so two starts at the boundary serialise: the second waits for
//! the first's commit, then (READ COMMITTED: each statement takes a fresh
//! snapshot) counts the first's row and queues. The lock is released at
//! commit or rollback.

use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

/// What the gate decided for a new start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Under the limit: start as usual.
    Run,
    /// At the limit: park the row in `QUEUED` and start no task.
    Queue,
}

/// Decide whether a new execution of `flow_id` may run now, under the flow's
/// advisory lock (held to the end of `tx`'s transaction).
///
/// The running pool is every execution of the flow that is neither terminal
/// (`workflow.state_is_terminal`, the one list of terminal states) nor
/// `QUEUED`.
pub async fn concurrency_admit(tx: &mut PgConnection, flow_id: Uuid, limit: i32) -> Result<Admit> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(flow_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions \
         WHERE flow_id = $1 AND NOT workflow.state_is_terminal(state) AND state <> 'QUEUED'",
    )
    .bind(flow_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    Ok(if live < i64::from(limit) {
        Admit::Run
    } else {
        Admit::Queue
    })
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
