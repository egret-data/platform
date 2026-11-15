//! Pause by hand and resume with `onResume` inputs -- workflow package 7c
//! (spec `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D3, D5,
//! D6). Both run inside the execution's lock (`claim::load`) and end in
//! `claim::settle`, as every writer does.

use egret_core::{Error, Result};
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::{self, Stack};
use super::txn::{transaction, InTxn};
use crate::executor::{apply, pause_by_hand_commands, PauseRefusal};

/// What [`pause_by_hand`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paused {
    /// The execution is `PAUSED`; no task run changed.
    Done,
    /// Nothing written: the execution is not `RUNNING` or `RESTARTED`; its
    /// state's wire name (D3).
    NotRunning(String),
    /// Nothing written: a `Pause` or `LoopUntil` is already waiting (Q5).
    TaskWaiting,
}

/// **Pause by hand** (D3; Kestra `ES.pause`, `ES:907-916`): one
/// `SetExecutionState { PAUSED }` under the execution's lock, then the
/// settle, whose `advance` answers nothing (D2). Runs already `RUNNING` are
/// left to finish. The state is re-checked under the lock, so a pause that
/// loses the race to a kill, to the execution's end or to a `Pause` that
/// has just started waiting writes nothing.
///
/// Scoped by `workspace`: an execution of another workspace is `NotFound`.
pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused> {
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
        let (ctx, mut state, now) = claim::load(conn, id, Stack::Fresh(gate.clone())).await?;
        if ctx.workspace_id != workspace {
            return Err(Error::NotFound("execution".into()));
        }
        let cmds = match pause_by_hand_commands(&state) {
            Ok(cmds) => cmds,
            Err(PauseRefusal::NotRunning(s)) => {
                return Ok(Paused::NotRunning(
                    s.map_or("UNKNOWN", |s| s.wire()).to_string(),
                ));
            }
            Err(PauseRefusal::TaskWaiting) => return Ok(Paused::TaskWaiting),
        };
        let before = state.clone();
        for c in &cmds {
            apply(&mut state, c);
        }
        claim::settle(conn, &ctx, &before, state, now, None).await?;
        Ok(Paused::Done)
    })
}
