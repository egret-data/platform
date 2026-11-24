//! Force-run -- workflow package 8b. Starts a `QUEUED` execution NOW,
//! bypassing its flow's concurrency limit (the "jump the queue" action).
//!
//! Deliberate divergence from Kestra, which allows force-run on any
//! non-terminated execution: egret's only held state is `QUEUED` (a flow has
//! no other "waiting" state), so force-run is `QUEUED`-only.

use egret_core::{Error, Result};
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::{self, Stack};
use super::concurrency::{concurrency_force_start, take_flow_lock};
use super::txn::{transaction, InTxn};
use crate::executor::{ExecutionState, RunState};

/// What [`force_run`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForceRun {
    /// The execution was promoted and started, limit bypassed.
    Started,
    /// Nothing written: the execution is not `QUEUED`; its state's wire name.
    NotQueued(String),
}

/// The one rule of what may be force-run: the execution's last state is
/// `QUEUED`. `Some(state wire name)` is the refusal. [`force_run`] (under the
/// lock) and the routes' read-only pre-flight both read this, so they cannot
/// drift.
pub fn force_run_refusal(last: Option<RunState>) -> Option<String> {
    match last {
        Some(RunState::Queued) => None,
        other => Some(other.map_or("UNKNOWN", |s| s.wire()).to_string()),
    }
}

/// **Force-run** `id` of `workspace`. The flow's advisory lock is taken
/// before the execution's row lock (the drain's order), then the state is
/// re-checked under both, so an execution a drain has just promoted is
/// refused, not started twice.
pub async fn force_run(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<ForceRun> {
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
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
        let stack = Stack::Fresh(gate.clone());
        let (ctx, state, _now) = claim::load(conn, id, stack.clone()).await?;
        if ctx.workspace_id != workspace {
            return Err(Error::NotFound("execution".into()));
        }
        if let Some(s) = force_run_refusal(last_state(&state)) {
            return Ok(ForceRun::NotQueued(s));
        }
        match ctx.flow_id {
            Some(flow_id) => concurrency_force_start(conn, &stack, flow_id, id).await?,
            // A queued row always has a flow; start it plainly if not.
            None => claim::promote_queued(conn, id, stack).await?,
        }
        Ok(ForceRun::Started)
    })
}

fn last_state(state: &ExecutionState) -> Option<RunState> {
    state.history.last().copied()
}
