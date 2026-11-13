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
//! restarted in place by the same function ([`prepare`] and [`open`] with
//! [`Nested::LoopIteration`]), in the same transaction, the parent locked
//! first (D8). A reset `Subflow` run's earlier child is restarted the same
//! way, with [`Nested::SubflowChild`], when the walk opens the run
//! (`subflow::spawn_one`, D9).
//!
//! **Every restart is opened by the walk** (`claim::Job::Restart`): the
//! commands are applied and the side writes made when the walk reaches the
//! job, and the settle is a frame of that walk. A restart reached from
//! inside a settle -- a `Subflow` child's, which `spawn_one` asks for while
//! its parent settles -- cannot run a settle of its own, since only the walk
//! runs one (`claim::walk`).
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Nested {
    /// The execution a person named: every check of D10 applies.
    No,
    /// The last failing iteration of a `Loop` run its parent's restart
    /// reopened (D8). The loop-iteration refusal is skipped: this IS the
    /// restart through the loop root that refusal points to.
    LoopIteration,
    /// The earlier child of a reset `Subflow` run, which the walk restarts in
    /// place when it opens that run (D9, `subflow::spawn_one`). Every check
    /// of D10 applies: a refusal leaves the child alone and a new child is
    /// spawned, as before D9.
    SubflowChild,
}

/// The facts of the execution row the checks read, beside what `load` holds.
struct Row0220 {
    loop_root: Option<Uuid>,
    files_pruned: bool,
    idle: bool,
    labels: Option<serde_json::Value>,
}

/// One execution's restart, checked and computed under its lock, nothing
/// written yet: what [`prepare`] answers and [`open`] writes.
struct Prepared {
    ctx: claim::ExecutionCtx,
    state: crate::executor::ExecutionState,
    now: crate::executor::Tick,
    cmds: RestartCommands,
    labels: Option<serde_json::Value>,
    op: Op,
}

/// Which pure function computed a [`Prepared`]'s commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `executor::restart`: the execution ends on `RESTARTED`.
    Restart,
    /// `executor::mark_as` (D11): `RESTARTED`, `CANCELLED`, or unchanged
    /// while a run is still `PAUSED`.
    MarkAs,
}

/// **Restart `id` in place, its lock taken here** -- the body of
/// [`restart_execution`], which owns the transaction: prepared, then opened
/// and settled by a walk of its own (`claim::run_job`). A refusal writes
/// nothing.
async fn restart_locked(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    nested: Nested,
) -> Result<Restarted> {
    match prepare(conn, id, stack, nested).await? {
        Ok(prepared) => {
            claim::run_job(
                conn,
                claim::Job::Restart(Box::new(Opening { id, prepared })),
            )
            .await?;
            Ok(Restarted::Done)
        }
        Err(refused) => Ok(Restarted::Refused(refused)),
    }
}

/// A restart checked and computed under its execution's lock, for the walk
/// to open (`claim::Job::Restart`, [`open`]).
pub(super) struct Opening {
    id: Uuid,
    prepared: Prepared,
}

/// **D9: restart a reset `Subflow` run's earlier child in place**, from
/// inside its parent's settle (`subflow::spawn_one`). The child is locked
/// and checked now, the parent already locked by its own settle: parent,
/// then child, the order kill's cascade takes (`kill.rs`). The answer is the
/// job that restarts it, for the walk to run where a fresh child's first
/// settle would run, or why its restart is refused (nothing written).
pub(super) async fn restart_subflow_child(
    conn: &mut PgConnection,
    child: Uuid,
    caller: claim::Caller,
) -> Result<std::result::Result<claim::Job, RestartRefused>> {
    Ok(
        prepare(conn, child, Stack::Nested(caller), Nested::SubflowChild)
            .await?
            .map(|prepared| {
                claim::Job::Restart(Box::new(Opening {
                    id: child,
                    prepared,
                }))
            }),
    )
}

/// Lock `id` and its runs, run the checks and compute the commands. Writes
/// nothing, so a refusal leaves the execution as it was.
async fn prepare(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    nested: Nested,
) -> Result<std::result::Result<Prepared, RestartRefused>> {
    let Locked {
        ctx,
        state,
        now,
        row,
    } = lock(conn, id, stack).await?;

    // The checks, in the spec's order (D10, then the executor's).
    if let Some(refused) = d10_refusal(&row, &state, nested) {
        return Ok(Err(refused.into()));
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
    // A reset `Subflow` run (`cmds.subflows`) needs nothing here: the
    // settle's walk opens it, and `subflow::spawn_one` looks up its earlier
    // child there (D9), keyed off the label the side writes add.
    Ok(Ok(Prepared {
        ctx,
        state,
        now,
        cmds,
        labels: row.labels,
        op: Op::Restart,
    }))
}

/// An execution loaded under its lock, its runs locked too, and the facts
/// of its row the checks read.
struct Locked {
    ctx: claim::ExecutionCtx,
    state: crate::executor::ExecutionState,
    now: crate::executor::Tick,
    row: Row0220,
}

/// `load` takes the execution's lock (`FOR NO KEY UPDATE`); the run rows are
/// then locked `FOR UPDATE` in `run_ix` order, as the kill takes them.
async fn lock(conn: &mut PgConnection, id: Uuid, stack: Stack) -> Result<Locked> {
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
    Ok(Locked {
        ctx,
        state,
        now,
        row: Row0220 {
            loop_root: row.get("loop_root"),
            files_pruned: row.get("files_pruned"),
            idle: row.get("idle"),
            labels: row.get("labels"),
        },
    })
}

/// D10's refusals, shared by a restart and a task run's state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum D10 {
    LoopIteration { root: Uuid },
    FilesPruned,
    AfterExecutionRunning,
}

impl From<D10> for RestartRefused {
    fn from(d: D10) -> Self {
        match d {
            D10::LoopIteration { root } => Self::LoopIteration { root },
            D10::FilesPruned => Self::FilesPruned,
            D10::AfterExecutionRunning => Self::AfterExecutionRunning,
        }
    }
}

impl From<D10> for StateChangeRefused {
    fn from(d: D10) -> Self {
        match d {
            D10::LoopIteration { root } => Self::LoopIteration { root },
            D10::FilesPruned => Self::FilesPruned,
            D10::AfterExecutionRunning => Self::AfterExecutionRunning,
        }
    }
}

/// D10's refusals, in the spec's order: a loop iteration (unless this IS
/// the restart through its root), the files pruned, an `afterExecution:`
/// still running.
fn d10_refusal(
    row: &Row0220,
    state: &crate::executor::ExecutionState,
    nested: Nested,
) -> Option<D10> {
    if let (Some(root), Nested::No) = (row.loop_root, nested) {
        return Some(D10::LoopIteration { root });
    }
    if row.files_pruned {
        return Some(D10::FilesPruned);
    }
    if is_terminal(state) && !row.idle {
        return Some(D10::AfterExecutionRunning);
    }
    None
}

fn is_terminal(state: &crate::executor::ExecutionState) -> bool {
    state.history.last().is_some_and(|s| s.is_terminal())
}

/// Kestra's line when a reopened `Loop` has no iteration to restart
/// (`ExecutorService:735-762`; spec D8).
pub const NO_RESTARTABLE_ITERATION: &str = "No restartable loop sub-execution found";

/// **Open a prepared restart, when the walk reaches it**: the `Loop`
/// iterations first chosen and locked (D8), then this execution's commands
/// and side writes. The answer is this execution's settle, as a frame, and
/// each chosen iteration's own restart, as jobs the walk runs AFTER that
/// frame and its whole tail (`claim::walk_from`).
///
/// **Why the iterations settle after the parent.** An iteration's restart
/// settles it, and a settle that ends it reports to its parent
/// (`spawn::on_execution_terminal`), which reads the parent's rows and
/// returns at once while the `Loop` run is terminal. So the parent's reopened
/// `Loop` run must be persisted (`RUNNING`) before any iteration can end
/// again. They are *locked and checked* before the parent settles, so a
/// refused iteration's `Loop` run can still be failed in the parent's own
/// settle.
pub(super) async fn open(
    conn: &mut PgConnection,
    opening: Opening,
) -> Result<(claim::Frame<'static>, Vec<claim::Job>)> {
    let Opening { id, prepared } = opening;
    let Prepared {
        ctx,
        mut state,
        now,
        mut cmds,
        labels,
        op,
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
                match prepare(conn, iteration, stack, Nested::LoopIteration).await {
                    Ok(Ok(p)) => Some((iteration, p)),
                    Ok(Err(refused)) => {
                        tracing::warn!(
                            execution = %id, %iteration, ?refused,
                            "a Loop's last failing iteration cannot be restarted"
                        );
                        None
                    }
                    // **An iteration row that cannot be loaded** (its frozen
                    // source no longer plans, a history it cannot parse)
                    // falls back as D9's child does (final review M3): the
                    // `Loop` run fails with Kestra's line and the parent's
                    // restart goes on, instead of the whole call failing.
                    // Only a fault in the row's own content: a SQL error has
                    // aborted the transaction, and a deadlock must reach
                    // `restart_execution`'s retry.
                    Err(e) if claim::row_content_fault(&e) => {
                        tracing::warn!(
                            execution = %id, %iteration, error = %e,
                            "a Loop's last failing iteration cannot be loaded"
                        );
                        None
                    }
                    Err(e) => return Err(e),
                }
            }
            None => None,
        };
        match prepared {
            Some((iteration, prepared)) => {
                iterations.push(claim::Job::Restart(Box::new(Opening {
                    id: iteration,
                    prepared,
                })))
            }
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
    debug_assert!(
        op != Op::Restart || state.history.last() == Some(&RunState::Restarted),
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
    Ok((claim::Frame::loaded(ctx, before, state, now), iterations))
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

/// Why a task run's state change ([`set_task_run_state`]) or an execution's
/// status change ([`set_execution_status`]) wrote nothing (spec D11). The
/// route maps each to its coded 409.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateChangeRefused {
    /// The execution has not ended, or ended `KILLED`; its state's wire name.
    NotChangeable(String),
    /// The target is the state the run or the execution is already in.
    StateUnchanged,
    /// The execution is an iteration of a `Loop`; change it through its root.
    LoopIteration { root: Uuid },
    /// No run of this execution has that id, or the run with it was emptied
    /// (Kestra removes it) and never rebuilt.
    NoSuchTaskRun,
    /// The execution has ended but its `afterExecution:` still runs (D10).
    AfterExecutionRunning,
    /// The pruner deleted the execution's files (`files_pruned_at`, D10).
    FilesPruned,
    /// The run has a `WorkingDirectory` ancestor, whose directory is closed;
    /// restart the execution instead, which re-runs the block whole.
    InsideWorkingDirectory,
}

/// D11's four targets, the ones Kestra's UI offers. The route refuses any
/// other with 400 first; this is the same check, one layer down.
fn check_target(target: RunState) -> Result<()> {
    if matches!(
        target,
        RunState::Success | RunState::Warning | RunState::Failed | RunState::Cancelled
    ) {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "the state can only be changed to SUCCESS, WARNING, FAILED or CANCELLED, not {}",
            target.wire()
        )))
    }
}

/// D11's source state: ended, and not `KILLED` (`State.java:189-191`).
fn not_changeable(state: &crate::executor::ExecutionState) -> Option<StateChangeRefused> {
    let current = state.history.last().copied().unwrap_or(RunState::Created);
    (!current.is_terminal() || current == RunState::Killed)
        .then(|| StateChangeRefused::NotChangeable(current.wire().to_string()))
}

/// **Set one task run's state** (spec D11, Kestra `markAs`, `ES:603-662`).
/// The run takes `target` on its history and its last attempt, keeping its
/// outputs; its ancestors are reopened and the end-hook runs emptied
/// (`executor::mark_as`); the execution gets the restart's side writes (D7:
/// `attempt_number + 1`, the `system.restarted` label, the reset and emptied
/// runs' results cleared, the emptied runs' ids renewed) and settles on from
/// `RESTARTED`, or ends `CANCELLED` for a `CANCELLED` target. It is opened
/// by the walk as a restart is (`claim::Job::Restart`).
///
/// Unscoped by workspace, like every worker entry point; the route scopes
/// first. A refusal writes nothing.
pub async fn set_task_run_state(
    pool: &PgPool,
    id: Uuid,
    task_run_id: Uuid,
    target: RunState,
) -> Result<std::result::Result<(), StateChangeRefused>> {
    check_target(target)?;
    let mut retries = 0;
    loop {
        let changed = transaction!(pool, |at| {
            let (conn, gate) = at.parts();
            set_task_run_state_locked(conn, id, Stack::Fresh(gate.clone()), task_run_id, target)
                .await
        });
        match changed {
            Err(e) if retries < DEADLOCK_RETRIES && is_deadlock(&e) => {
                retries += 1;
                tracing::info!(execution = %id, "a task run's state change was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

async fn set_task_run_state_locked(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    task_run_id: Uuid,
    target: RunState,
) -> Result<std::result::Result<(), StateChangeRefused>> {
    let Locked {
        ctx,
        state,
        now,
        row,
    } = lock(conn, id, stack).await?;
    if let Some(refused) = d10_refusal(&row, &state, Nested::No) {
        return Ok(Err(refused.into()));
    }
    if let Some(refused) = not_changeable(&state) {
        return Ok(Err(refused));
    }
    // This execution's run only. `load` refuses a gap in `run_ix`, so a
    // run's `run_ix` is its index in `state.runs`.
    let run_ix: Option<i32> = sqlx::query_scalar(
        "SELECT run_ix FROM workflow.task_runs WHERE execution_id = $1 AND id = $2",
    )
    .bind(id)
    .bind(task_run_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let Some(run) = run_ix
        .and_then(|ix| usize::try_from(ix).ok())
        .filter(|&ix| state.runs.get(ix).is_some_and(|r| !r.history.is_empty()))
    else {
        return Ok(Err(StateChangeRefused::NoSuchTaskRun));
    };
    if state.runs[run].history.last() == Some(&target) {
        return Ok(Err(StateChangeRefused::StateUnchanged));
    }
    let cmds = match crate::executor::mark_as(&ctx.plan, &state, run, target, now) {
        Ok(cmds) => cmds,
        Err(RestartRefusal::NotRestartable(s)) => {
            return Ok(Err(StateChangeRefused::NotChangeable(s.wire().to_string())))
        }
        Err(RestartRefusal::NothingToRestart) => return Ok(Err(StateChangeRefused::NoSuchTaskRun)),
        Err(RestartRefusal::InsideWorkingDirectory) => {
            return Ok(Err(StateChangeRefused::InsideWorkingDirectory))
        }
    };
    // `mark_as` never resets a run to `CREATED`: the marked run takes the
    // target and its ancestors (flowables, never a `Subflow` or a `Loop`'s
    // own iteration) are reopened `RUNNING`. So `cmds.subflows` and
    // `cmds.loops` are empty; `open` handles them as a restart's if not.
    claim::run_job(
        conn,
        claim::Job::Restart(Box::new(Opening {
            id,
            prepared: Prepared {
                ctx,
                state,
                now,
                cmds,
                labels: row.labels,
                op: Op::MarkAs,
            },
        })),
    )
    .await?;
    Ok(Ok(()))
}

/// **Change a finished execution's status** (spec D11, Kestra `changeState`,
/// `ES:1385-1392`): `target` appended to its history, then `settle`. No run
/// is touched, no label is added, `attempt_number` is unchanged; the outbox
/// writes one Flow-trigger event for the appended state. A `Subflow` parent
/// whose run is already terminal ignores the child's new state
/// (`subflow::on_child_terminal`).
///
/// Unscoped by workspace, like every worker entry point; the route scopes
/// first. A refusal writes nothing.
pub async fn set_execution_status(
    pool: &PgPool,
    id: Uuid,
    target: RunState,
) -> Result<std::result::Result<(), StateChangeRefused>> {
    check_target(target)?;
    let mut retries = 0;
    loop {
        let changed = transaction!(pool, |at| {
            let (conn, gate) = at.parts();
            set_execution_status_locked(conn, id, Stack::Fresh(gate.clone()), target).await
        });
        match changed {
            Err(e) if retries < DEADLOCK_RETRIES && is_deadlock(&e) => {
                retries += 1;
                tracing::info!(execution = %id, "an execution's status change was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

async fn set_execution_status_locked(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    target: RunState,
) -> Result<std::result::Result<(), StateChangeRefused>> {
    let Locked {
        ctx,
        mut state,
        now,
        row,
    } = lock(conn, id, stack).await?;
    if let Some(root) = row.loop_root {
        return Ok(Err(StateChangeRefused::LoopIteration { root }));
    }
    if let Some(refused) = not_changeable(&state) {
        return Ok(Err(refused));
    }
    // An execution whose `afterExecution:` still runs has not ended (D10).
    // The pruned files do not matter here: nothing runs again.
    if !row.idle {
        return Ok(Err(StateChangeRefused::AfterExecutionRunning));
    }
    if state.history.last() == Some(&target) {
        return Ok(Err(StateChangeRefused::StateUnchanged));
    }
    let before = state.clone();
    apply(&mut state, &Command::SetExecutionState { state: target });
    claim::settle(conn, &ctx, &before, state, now, None).await?;
    Ok(Ok(()))
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
