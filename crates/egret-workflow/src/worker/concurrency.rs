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
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::claim::{promote_queued, ExecutionCtx, Stack};
use super::subflow::MAX_SPAWN_CHAIN;
use super::txn::{transaction, InTxn};
use crate::models::{Concurrency, ConcurrencyBehavior};

tokio::task_local! {
    /// The flow a [`concurrency_drain`] on this task is promoting for. A
    /// promoted execution that terminalizes inside its first settle must not
    /// start a drain of its own: the enclosing loop re-counts after every
    /// promotion and carries on iteratively, so nesting is what would make a
    /// backlog recurse.
    static DRAINING: Uuid;
}

/// What the gate decided for a new start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Under the limit: start as usual.
    Run,
    /// At the limit: park the row in `QUEUED` and start no task.
    Queue,
    /// At the limit with `behavior: CANCEL`: the row is born `CANCELLED`.
    Cancel,
    /// At the limit with `behavior: FAIL`: the row is born `FAILED`.
    Fail,
}

/// Decide whether a new execution of `flow_id` may run now, under the flow's
/// advisory lock (held to the end of `tx`'s transaction).
///
/// The running pool is every execution of the flow that is neither terminal
/// (`workflow.state_is_terminal`, the one list of terminal states) nor
/// `QUEUED`.
pub async fn concurrency_admit(
    tx: &mut PgConnection,
    flow_id: Uuid,
    limit: i32,
    behavior: ConcurrencyBehavior,
) -> Result<Admit> {
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
        match behavior {
            ConcurrencyBehavior::Queue => Admit::Queue,
            ConcurrencyBehavior::Cancel => Admit::Cancel,
            ConcurrencyBehavior::Fail => Admit::Fail,
        }
    })
}

/// Start the oldest `QUEUED` executions of `flow_id` while the flow is under
/// `limit`, under the same per-flow advisory lock as [`concurrency_admit`]
/// (held to the end of the caller's transaction). At most `limit - live`
/// are promoted per call; the ids come back oldest first.
///
/// **The drain's scope is the fresh-start rows 8a queues**: a promoted row
/// is started exactly as a fresh start (`claim::promote_queued`). `stack`
/// is the caller's: `Nested` inside a settle, `Fresh` from the sweep's own
/// transaction, so the first settle reads through the owner's prefetch cache.
pub(super) async fn concurrency_drain(
    tx: &mut PgConnection,
    stack: &Stack,
    flow_id: Uuid,
    limit: i32,
) -> Result<Vec<Uuid>> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(flow_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    let mut promoted = Vec::new();
    loop {
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow.executions \
             WHERE flow_id = $1 AND NOT workflow.state_is_terminal(state) AND state <> 'QUEUED'",
        )
        .bind(flow_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if live >= i64::from(limit) {
            break;
        }
        // Oldest first, `workflow_executions_queued_idx`. `SKIP LOCKED` is
        // belt and braces: every promoter holds the flow's lock, so a row
        // locked by someone else is not ours to start.
        let next: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM workflow.executions \
             WHERE flow_id = $1 AND state = 'QUEUED' \
             ORDER BY created_at, id LIMIT 1 FOR NO KEY UPDATE SKIP LOCKED",
        )
        .bind(flow_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(id) = next else { break };
        DRAINING
            .scope(flow_id, promote_queued(tx, id, stack.clone()))
            .await?;
        promoted.push(id);
    }
    Ok(promoted)
}

/// `source`'s `concurrency: {limit, behavior: QUEUE}` limit, if it has one.
fn queue_limit(source: &str) -> Option<i32> {
    match crate::spec::parse(source).ok()?.concurrency? {
        Concurrency {
            limit: Some(limit),
            behavior: ConcurrencyBehavior::Queue,
        } => Some(limit),
        _ => None,
    }
}

/// The settle's hook: `ctx`'s execution just became terminal, so a slot of
/// its flow is free. Cheap when nothing waits (one indexed `EXISTS`, no
/// parse).
pub(super) async fn on_terminal(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<()> {
    let Some(flow_id) = ctx.flow_id else {
        return Ok(());
    };
    // A promoted execution that terminalizes INSIDE its first settle comes
    // back here. The drain that promoted it is still looping over its flow
    // (and re-counts), so a nested drain of the same flow would only recurse
    // down the whole backlog: skip. For any other nesting (a child of another
    // flow, say) the depth is capped as a spawn chain is, and the sweep
    // (`concurrency_sweep`, next reaper tick) starts what was skipped.
    if DRAINING.try_with(|f| *f == flow_id).unwrap_or(false) || ctx.depth + 1 > MAX_SPAWN_CHAIN {
        return Ok(());
    }
    let waiting: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workflow.executions WHERE flow_id = $1 AND state = 'QUEUED')",
    )
    .bind(flow_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    if !waiting {
        return Ok(());
    }
    let Some(limit) = queue_limit(&ctx.source) else {
        return Ok(());
    };
    concurrency_drain(conn, &Stack::Nested(ctx.caller()), flow_id, limit).await?;
    Ok(())
}

/// **The safety sweep**: for every flow with a `QUEUED` execution waiting,
/// drain it up to its limit. Recovers a drain that never happened (a crash
/// between the terminal and its drain, a terminal written outside a settle).
/// The limit is the oldest waiting row's own stored `concurrency:`. Returns
/// how many executions it started. Rides the lease reaper's tick
/// (`runtime::reaper_pass`).
pub async fn concurrency_sweep(pool: &PgPool) -> Result<usize> {
    let waiting = sqlx::query(
        "SELECT DISTINCT ON (flow_id) flow_id, source FROM workflow.executions \
         WHERE state = 'QUEUED' AND flow_id IS NOT NULL \
         ORDER BY flow_id, created_at, id",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    let mut started = 0;
    for row in waiting {
        let flow_id: Uuid = row.get("flow_id");
        let source: String = row.get("source");
        let Some(limit) = queue_limit(&source) else {
            continue;
        };
        let promoted = transaction!(pool, |at| {
            let (conn, gate) = at.parts();
            concurrency_drain(conn, &Stack::Fresh(gate.clone()), flow_id, limit).await
        })?;
        started += promoted.len();
    }
    Ok(started)
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
