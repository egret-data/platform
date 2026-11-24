//! Unqueue -- workflow package 8b. Takes a `QUEUED` execution out of its
//! flow's queue by moving it to a terminal state. The execution had no task
//! run, so this is one `SetExecutionState` under the execution's lock and the
//! settle every writer ends in, which stamps `ended_at` and fires the terminal
//! hooks (outbox, flow triggers, alerts) as for any other terminal transition.
//! It never goes through the drain's start path: the execution does not run.

use egret_core::{Error, Result};
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::{self, Stack};
use super::concurrency::take_flow_lock;
use super::txn::{transaction, InTxn};
use crate::executor::{apply, Command, ExecutionState, RunState};

/// What [`unqueue`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unqueued {
    /// The execution is now in the requested terminal state.
    Done,
    /// Nothing written: the execution is not `QUEUED`; its state's wire name.
    NotQueued(String),
}

/// The one rule of what may be unqueued: the execution's last state is
/// `QUEUED`. `Some(state wire name)` is the refusal. [`unqueue`] (under the
/// lock) and the routes' read-only pre-flight both read this, so they cannot
/// drift.
pub fn unqueue_refusal(last: Option<RunState>) -> Option<String> {
    match last {
        Some(RunState::Queued) => None,
        other => Some(other.map_or("UNKNOWN", |s| s.wire()).to_string()),
    }
}

/// **Unqueue** `id` of `workspace` to `target` (which must be terminal; the
/// route validates it). The state is re-checked under the lock, so an
/// execution a drain has just promoted is refused, not clobbered.
pub async fn unqueue(
    pool: &PgPool,
    workspace: Uuid,
    id: Uuid,
    target: RunState,
) -> Result<Unqueued> {
    debug_assert!(target.is_terminal(), "unqueue target must be terminal");
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
        // Lock order: the flow's advisory lock BEFORE the row lock, as
        // `force_run`, the drain and the admission gate take them. The settle
        // below drains the flow (re-taking the same lock, which is re-entrant
        // in a transaction); row-first here would be an ABBA with `force_run`.
        let flow_id: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT flow_id FROM workflow.executions WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| Error::Internal(e.into()))?;
        let Some(flow_id) = flow_id else {
            return Err(Error::NotFound("execution".into()));
        };
        if let Some(flow_id) = flow_id {
            take_flow_lock(conn, flow_id).await?;
        }
        let (ctx, mut state, now) = claim::load(conn, id, Stack::Fresh(gate.clone())).await?;
        if ctx.workspace_id != workspace {
            return Err(Error::NotFound("execution".into()));
        }
        if let Some(s) = unqueue_refusal(last_state(&state)) {
            return Ok(Unqueued::NotQueued(s));
        }
        let before = state.clone();
        apply(&mut state, &Command::SetExecutionState { state: target });
        claim::settle(conn, &ctx, &before, state, now, None).await?;
        Ok(Unqueued::Done)
    })
}

fn last_state(state: &ExecutionState) -> Option<RunState> {
    state.history.last().copied()
}
