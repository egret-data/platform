//! **Restart an execution in place** -- workflow package 7a (spec
//! `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md` D1,
//! D5, D7, D10, D12; Kestra `ExecutionService.restart`, `ES:281-373`).
//!
//! The executor half is pure (`executor::restart`): it reads the loaded
//! state and returns the commands that reset the runs in memory. This half
//! is the transaction around it, the shape of [`claim::resume_by_hand`]:
//!
//! 1. `load` takes the execution's lock (`FOR NO KEY UPDATE`), and the run
//!    rows are locked `FOR UPDATE` in `run_ix` order, as the kill takes them
//!    (`kill::kill_loaded`).
//! 2. The checks, under the lock, in the spec's order: a loop iteration, the
//!    files pruned, an `afterExecution:` still running (D10), then the
//!    executor's own refusals. A refusal writes nothing.
//! 3. `before = state.clone()`, the commands applied, then the side writes
//!    (D7): `attempt_number + 1`, the `system.restarted` label, the reset and
//!    emptied runs' `outputs`/`message`/`secret_outputs`/`outputs_bytes`
//!    cleared, and each emptied run's new task-run id (D4).
//! 4. `settle`, which persists the runs, appends `RESTARTED` and `RUNNING`
//!    (the outbox writes one Flow-trigger event for each) and drives the
//!    executor on from there.
//!
//! A deadlock's victim is run again, as the kill's is
//! ([`kill::DEADLOCK_RETRIES`]).

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::claim::{self, Stack};
use super::kill::{is_deadlock, DEADLOCK_RETRIES};
use super::txn::{transaction, InTxn};
use crate::executor::{apply, RestartCommands, RestartRefusal, RunState};
use crate::models::Label;

/// The label a restart puts on the execution (spec D7, Kestra `ES:360-364`).
/// Only the engine writes `system.*` labels (`labels.rs`).
pub const RESTARTED_LABEL_KEY: &str = "system.restarted";
/// The value of [`RESTARTED_LABEL_KEY`].
pub const RESTARTED_LABEL_VALUE: &str = "true";

/// What [`restart_execution`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restarted {
    /// The execution was restarted in place and settled on from there.
    Done,
    /// Nothing was written; why.
    Refused(RestartRefused),
}

/// Why a restart wrote nothing. The route maps each to its coded 409 (D13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartRefused {
    /// The execution is not `FAILED` or `PAUSED`; its state's wire name.
    NotRestartable(String),
    /// No run is `FAILED` or `PAUSED` (Kestra's "No task found to restart
    /// execution from!", D2).
    NothingToRestart,
    /// The execution has ended but its `afterExecution:` still runs (D10).
    AfterExecutionRunning,
    /// The pruner deleted the execution's files (`files_pruned_at`, D10).
    FilesPruned,
    /// The execution is an iteration of a `Loop`; restart its loop root (D10).
    LoopIteration { root: Uuid },
}

/// **Restart `execution` in place** (spec D1–D7, D10). Unscoped by
/// workspace, like every worker entry point; the route scopes first.
pub async fn restart_execution(pool: &PgPool, id: Uuid) -> Result<Restarted> {
    let mut retries = 0;
    loop {
        let restarted = transaction!(pool, |at| {
            let (conn, gate) = at.parts();
            restart_in(conn, id, Stack::Fresh(gate.clone())).await
        });
        match restarted {
            Err(e) if retries < DEADLOCK_RETRIES && is_deadlock(&e) => {
                retries += 1;
                tracing::info!(execution = %id, "a restart was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

/// The facts of the execution row the checks read, beside what `load` holds.
struct Row0220 {
    loop_root: Option<Uuid>,
    files_pruned: bool,
    idle: bool,
    labels: Option<serde_json::Value>,
}

async fn restart_in(conn: &mut PgConnection, id: Uuid, stack: Stack) -> Result<Restarted> {
    let (ctx, mut state, now) = claim::load(conn, id, stack).await?;
    sqlx::query(
        "SELECT 1 FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix FOR UPDATE",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    let row = sqlx::query(
        "SELECT CASE WHEN loop_index IS NOT NULL THEN parent_execution_id END AS loop_root, \
                files_pruned_at IS NOT NULL AS files_pruned, \
                COALESCE(workflow.execution_is_idle(id), false) AS idle, \
                labels \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    let row = Row0220 {
        loop_root: row.get("loop_root"),
        files_pruned: row.get("files_pruned"),
        idle: row.get("idle"),
        labels: row.get("labels"),
    };

    // The checks, in the spec's order (D10, then the executor's).
    if let Some(root) = row.loop_root {
        return Ok(Restarted::Refused(RestartRefused::LoopIteration { root }));
    }
    if row.files_pruned {
        return Ok(Restarted::Refused(RestartRefused::FilesPruned));
    }
    let terminal = state.history.last().is_some_and(|s| s.is_terminal());
    if terminal && !row.idle {
        return Ok(Restarted::Refused(RestartRefused::AfterExecutionRunning));
    }
    let cmds = match crate::executor::restart(&ctx.plan, &state, now) {
        Ok(cmds) => cmds,
        Err(RestartRefusal::NothingToRestart) => {
            return Ok(Restarted::Refused(RestartRefused::NothingToRestart))
        }
        Err(RestartRefusal::NotRestartable(s)) => {
            return Ok(Restarted::Refused(RestartRefused::NotRestartable(
                s.wire().to_string(),
            )))
        }
        Err(RestartRefusal::InsideWorkingDirectory) => {
            return Err(Error::Internal(anyhow::anyhow!(
                "executor::restart refused InsideWorkingDirectory, which only mark_as returns \
                 (execution {id})"
            )))
        }
    };
    // **Temporary refusal, removed by A3 (Loop, D8) and A4 (Subflow, D9).**
    // Restarting a `Loop`'s last failing iteration and a `Subflow`'s failed
    // child in place are the next two tasks; until they land, a restart that
    // would need either writes nothing rather than half of one.
    if !cmds.loops.is_empty() || !cmds.subflows.is_empty() {
        return Err(Error::Internal(anyhow::anyhow!(
            "restarting execution {id} needs a Loop iteration or a Subflow child restarted in \
             place, which is not built yet (handled by A3 (Loop) / A4 (Subflow))"
        )));
    }

    let before = state.clone();
    for cmd in &cmds.commands {
        apply(&mut state, cmd);
    }
    debug_assert_eq!(
        state.history.last(),
        Some(&RunState::Restarted),
        "executor::restart ends on RESTARTED"
    );
    side_writes(conn, id, &cmds, row.labels.as_ref()).await?;
    claim::settle(conn, &ctx, &before, state, now, None).await?;
    Ok(Restarted::Done)
}

/// D7 and D4's writes beside the state, in the restart's transaction and
/// BEFORE `settle`: `settle`'s `Surface::load` reads each run's id, outputs
/// and the execution's labels from these rows, so what the walk renders
/// after the restart already sees them.
async fn side_writes(
    conn: &mut PgConnection,
    id: Uuid,
    cmds: &RestartCommands,
    labels: Option<&serde_json::Value>,
) -> Result<()> {
    sqlx::query("UPDATE workflow.executions SET attempt_number = attempt_number + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

    // The label, in the column's own shape (`[{"key", "value"}]`, as
    // `labels::own_labels` reads it back and `write_render_side` writes it).
    let mut list = crate::labels::own_labels(labels);
    if !list.iter().any(|l| l.key == RESTARTED_LABEL_KEY) {
        list.push(Label {
            key: RESTARTED_LABEL_KEY.to_string(),
            value: RESTARTED_LABEL_VALUE.to_string(),
        });
        sqlx::query("UPDATE workflow.executions SET labels = $2 WHERE id = $1")
            .bind(id)
            .bind(serde_json::to_value(&list).expect("labels serialise"))
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }

    // A reset or emptied run's earlier result is not its result any more
    // (recorded divergence §6 item 4: Kestra keeps it until the run writes
    // again). `persist` never touches these columns.
    //
    // **And its lease goes with it.** A run that ended through another run's
    // decision -- a `Parallel` sibling its rule 1 failed while a worker held
    // it -- keeps the `claimed_at` of a lease whose commit was fenced off, and
    // `claim` takes only `claimed_at IS NULL`, so the reset run would never be
    // claimed again (measured: `a_parallel_with_two_failed_branches_ends_success`
    // stalled). Clearing it is safe for the same reason: a worker still
    // holding that old lease commits fenced on `claimed_at = <its lease>`,
    // which no longer matches, and loses it.
    let cleared: Vec<i32> = cmds
        .reset
        .iter()
        .chain(&cmds.emptied)
        .map(|&ix| i32::try_from(ix).expect("a run index fits the column"))
        .collect();
    if !cleared.is_empty() {
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET outputs = NULL, message = NULL, secret_outputs = NULL, outputs_bytes = NULL, \
                    claimed_at = NULL \
              WHERE execution_id = $1 AND run_ix = ANY($2)",
        )
        .bind(id)
        .bind(&cleared)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }

    // **D4: an emptied run is a new task run**, with a new id, so its old
    // logs, files and kept containers stay with the old one -- what
    // `claim::renew_restarted_ids` does for a run `RestartLoop` empties.
    // That function cannot be reused here: it renews inside `drive`, on the
    // `Surface` a settle builds, for a command `advance` emits there. These
    // runs are emptied before `settle` begins, so the new id is written onto
    // the row now, and `settle`'s `Surface::load` reads it like any other.
    let emptied: Vec<i32> = cmds
        .emptied
        .iter()
        .map(|&ix| i32::try_from(ix).expect("a run index fits the column"))
        .collect();
    for ix in emptied {
        sqlx::query(
            "UPDATE workflow.task_runs SET id = $3 WHERE execution_id = $1 AND run_ix = $2",
        )
        .bind(id)
        .bind(ix)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
