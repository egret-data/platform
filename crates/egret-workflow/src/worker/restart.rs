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
//! A `Loop` run the restart reopens has its last failing iteration
//! restarted in place by the same function ([`restart_locked`] with
//! [`Nested::LoopIteration`]), in the same transaction, the parent locked
//! first (D8).
//!
//! A deadlock's victim is run again, as the kill's is
//! ([`kill::DEADLOCK_RETRIES`]).

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::claim::{self, Stack};
use super::kill::{is_deadlock, DEADLOCK_RETRIES};
use super::txn::{transaction, InTxn};
use crate::executor::{apply, Command, RestartCommands, RestartRefusal, RunState};
use crate::logs::LogLevel;
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
            restart_locked(conn, id, Stack::Fresh(gate.clone()), Nested::No).await
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

/// **Which restart this is**: of an execution a person named, or of one the
/// restart of its parent reaches into, in the same transaction and under
/// the parent's lock (taken first: parent, then the nested execution, the
/// order kill's cascade takes, `kill.rs`).
///
/// A4 (`Subflow`, D9) adds its child here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Nested {
    /// The execution a person named: every check of D10 applies.
    No,
    /// The last failing iteration of a `Loop` run its parent's restart
    /// reopened (D8). The loop-iteration refusal is skipped: this IS the
    /// restart through the loop root that refusal points to.
    LoopIteration,
}

/// The facts of the execution row the checks read, beside what `load` holds.
struct Row0220 {
    loop_root: Option<Uuid>,
    files_pruned: bool,
    idle: bool,
    labels: Option<serde_json::Value>,
}

/// One execution's restart, checked and computed under its lock, nothing
/// written yet: what [`prepare`] answers and [`commit`] writes.
struct Prepared {
    ctx: claim::ExecutionCtx,
    state: crate::executor::ExecutionState,
    now: crate::executor::Tick,
    cmds: RestartCommands,
    labels: Option<serde_json::Value>,
}

/// **Restart `id` in place, its lock already held or taken here** -- the
/// body of [`restart_execution`], and of every restart it reaches into
/// (D8; A4's `Subflow` child, D9). `stack` is [`Stack::Fresh`] for the
/// execution a person named and [`Stack::Nested`] for one reached from its
/// parent's restart. A refusal writes nothing.
pub(super) async fn restart_locked(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    nested: Nested,
) -> Result<Restarted> {
    match prepare(conn, id, stack, nested).await? {
        Ok(prepared) => {
            Box::pin(commit(conn, id, prepared)).await?;
            Ok(Restarted::Done)
        }
        Err(refused) => Ok(Restarted::Refused(refused)),
    }
}

/// Lock `id` and its runs, run the checks and compute the commands. Writes
/// nothing, so a refusal leaves the execution as it was.
async fn prepare(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    nested: Nested,
) -> Result<std::result::Result<Prepared, RestartRefused>> {
    let (ctx, state, now) = claim::load(conn, id, stack).await?;
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
    if let (Some(root), Nested::No) = (row.loop_root, nested) {
        return Ok(Err(RestartRefused::LoopIteration { root }));
    }
    if row.files_pruned {
        return Ok(Err(RestartRefused::FilesPruned));
    }
    let terminal = state.history.last().is_some_and(|s| s.is_terminal());
    if terminal && !row.idle {
        return Ok(Err(RestartRefused::AfterExecutionRunning));
    }
    let cmds = match crate::executor::restart(&ctx.plan, &state, now) {
        Ok(cmds) => cmds,
        Err(RestartRefusal::NothingToRestart) => return Ok(Err(RestartRefused::NothingToRestart)),
        Err(RestartRefusal::NotRestartable(s)) => {
            return Ok(Err(RestartRefused::NotRestartable(s.wire().to_string())))
        }
        Err(RestartRefusal::InsideWorkingDirectory) => {
            return Err(Error::Internal(anyhow::anyhow!(
                "executor::restart refused InsideWorkingDirectory, which only mark_as returns \
                 (execution {id})"
            )))
        }
    };
    // **Temporary refusal, removed by A4 (Subflow, D9).** Restarting a
    // `Subflow`'s failed child in place is the next task; until it lands, a
    // restart that would need it writes nothing rather than half of one.
    if !cmds.subflows.is_empty() {
        return Err(Error::Internal(anyhow::anyhow!(
            "restarting execution {id} needs a Subflow child restarted in place, which is not \
             built yet (handled by A4 (Subflow))"
        )));
    }
    Ok(Ok(Prepared {
        ctx,
        state,
        now,
        cmds,
        labels: row.labels,
    }))
}

/// Kestra's line when a reopened `Loop` has no iteration to restart
/// (`ExecutorService:735-762`; spec D8).
pub const NO_RESTARTABLE_ITERATION: &str = "No restartable loop sub-execution found";

/// Apply a prepared restart: the `Loop` iterations first chosen and locked
/// (D8), then this execution's commands, side writes and `settle`, then each
/// chosen iteration's own restart.
///
/// **Why the iterations settle after the parent.** An iteration's restart
/// settles it, and a settle that ends it reports to its parent
/// (`spawn::on_execution_terminal`), which reads the parent's rows and
/// returns at once while the `Loop` run is terminal. So the parent's reopened
/// `Loop` run must be persisted (`RUNNING`) before any iteration can end
/// again. They are *locked and checked* before the parent settles, so a
/// refused iteration's `Loop` run can still be failed in the parent's own
/// settle.
async fn commit(conn: &mut PgConnection, id: Uuid, prepared: Prepared) -> Result<()> {
    let Prepared {
        ctx,
        mut state,
        now,
        mut cmds,
        labels,
    } = prepared;

    // D8: for each reopened `Loop` run, its last failing iteration -- the
    // highest `loop_index` whose own state is `FAILED` or `PAUSED`
    // (Kestra `ES:985-991`, `findLastFailingLoopSubExecution`) -- locked and
    // checked by the same function. None, or one whose own restart is
    // refused (nothing in it to restart from), fails the `Loop` run with
    // Kestra's line, and the parent's restart goes on.
    let mut iterations = Vec::new();
    let mut no_iteration = Vec::new();
    for &run in &cmds.loops {
        let last: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM workflow.executions \
              WHERE parent_execution_id = $1 AND parent_run_ix = $2 \
                AND loop_index IS NOT NULL AND state IN ('FAILED', 'PAUSED') \
              ORDER BY loop_index DESC LIMIT 1",
        )
        .bind(id)
        .bind(i32::try_from(run).expect("a run index fits the column"))
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?;
        let prepared = match last {
            Some(iteration) => {
                let stack = Stack::Nested(ctx.caller());
                match prepare(conn, iteration, stack, Nested::LoopIteration).await? {
                    Ok(p) => Some((iteration, p)),
                    Err(refused) => {
                        tracing::info!(
                            execution = %id, %iteration, ?refused,
                            "a Loop's last failing iteration cannot be restarted"
                        );
                        None
                    }
                }
            }
            None => None,
        };
        match prepared {
            Some(p) => iterations.push(p),
            None => {
                cmds.commands.push(Command::SetRunState {
                    run,
                    state: RunState::Failed,
                });
                no_iteration.push(run);
            }
        }
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
    side_writes(conn, id, &cmds, labels.as_ref()).await?;
    for run in no_iteration {
        let run_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
        )
        .bind(id)
        .bind(i32::try_from(run).expect("a run index fits the column"))
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
        super::spawn::log_line(
            conn,
            ctx.workspace_id,
            id,
            Some(run_id),
            LogLevel::Error,
            NO_RESTARTABLE_ITERATION,
            &[],
        )
        .await?;
    }
    claim::settle(conn, &ctx, &before, state, now, None).await?;

    for (iteration, p) in iterations {
        Box::pin(commit(conn, iteration, p)).await?;
    }
    Ok(())
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
    //
    // **Except a reopened `Loop` run's `outputs`** (D8, A3): they hold its
    // counters and its #6 list, which the iterations that are not restarted
    // still account for. They stay; the restarted iteration's end replaces
    // its own index's count and entry (`spawn::on_execution_terminal`).
    let cleared: Vec<i32> = cmds
        .reset
        .iter()
        .chain(&cmds.emptied)
        .map(|&ix| i32::try_from(ix).expect("a run index fits the column"))
        .collect();
    let loops: Vec<i32> = cmds
        .loops
        .iter()
        .map(|&ix| i32::try_from(ix).expect("a run index fits the column"))
        .collect();
    if !cleared.is_empty() {
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET outputs = CASE WHEN run_ix = ANY($3) THEN outputs END, \
                    secret_outputs = CASE WHEN run_ix = ANY($3) THEN secret_outputs END, \
                    outputs_bytes = CASE WHEN run_ix = ANY($3) THEN outputs_bytes END, \
                    message = NULL, claimed_at = NULL \
              WHERE execution_id = $1 AND run_ix = ANY($2)",
        )
        .bind(id)
        .bind(&cleared)
        .bind(&loops)
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
