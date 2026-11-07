//! [`Command::SpawnExecution`](crate::executor::Command::SpawnExecution) — the
//! one command W3a emits that nothing has ever executed.
//!
//! # What a spawn is
//!
//! A `Loop` does not iterate inside its execution. Each value produces a
//! separate **sub-execution** that runs the loop's body, and the body's tasks
//! never get a run in the parent — measured over the whole golden, the 41
//! entries carrying a sub-execution carry 142 of them between them and not one
//! loop-body task appears in its parent's `taskRunList`
//! (`no_run_is_ever_created_for_a_task_inside_a_loop`). So this module creates
//! `workflow.executions` rows, not `task_runs` rows, and migration 0135 is the
//! four columns that say which loop iteration each row is.
//!
//! # The three facts this module is built on, all measured on the ARTIFACT
//!
//! Not on the `data-portal/kestra` checkout: W3b Task 3 established that the
//! image and the checkout are **different builds** (the image tagged
//! `v2.0.0-rc13` reports `version=nightly`, revision `169ca7f2…`, built
//! 2026-09-01; the checkout is `96893b589a` of 2026-08-17), and a class-level
//! citation and an artifact measurement have already disagreed once on exactly
//! the fact a task was asked to settle. `oracle-exec/traces.json` is the
//! artifact's own output, harvested from the pinned digest
//! `sha256:28f59bf1…`, and every count below is re-derived from it by
//! `oracle-exec/README.md`'s Task 4 section.
//!
//! 1. **`loopRun.index` is 0-based and contiguous.** Across the 670 loop
//!    sub-executions in `traces.json`, the index sets per spawning taskrun are
//!    `{0}`, `{0,1}`, `{0,1,2}` and `{0,1,2,3}` — 265 groups, not one of them
//!    starting at 1 or carrying a gap.
//! 2. **The creation instant carries no information at all.** All 670
//!    sub-executions have the `startDate` of their *parent execution*, and all
//!    220 multi-sibling groups share one instant between them. Ordering
//!    `spawned` by the instant is therefore not "unstable", it is undefined —
//!    which is why `normalise.py` orders by the index and bails when two
//!    iterations claim one, and why `workflow_executions_loop_run_idx` makes
//!    that collision unrepresentable rather than detectable. (Migration 0135
//!    created that index; **migration 0138 owns it now** -- it drops and
//!    recreates it under the same name with `NULLS NOT DISTINCT`, so that the
//!    same argument covers the replay child, whose key is all NULLs.)
//! 3. **`concurrencyLimit` is a live cap, not just a first-batch size.**
//!    `advance()` emits the first batch and cannot emit a second — it is pure
//!    over `(plan, state, now)` and `ExecutionState` records nothing about a
//!    sub-execution — so [`on_execution_terminal`] is where the rest of the
//!    loop happens. Three corpus entries pin all three branches of it:
//!    `loop-parallel-less` (limit 2, 4 values) ran **4** sub-executions,
//!    `loop-failed` (limit 1, `transmitFailed` defaulted) ran **1** and the
//!    loop FAILED, and `loop-failed-no-transmit` ran **3** failing iterations
//!    and the loop SUCCEEDED.
//!
//! # Where this is knowingly narrower than Kestra
//!
//! **Corrected 2026-10-27 (end hooks A6, D5): no longer true.** `models::Loop`
//! now carries `errors:` and `finally:`, and [`sub_flow_source`] makes them the
//! iteration's own `errors`/`finally` (the parent flow's are still not
//! inherited, and an iteration has no `afterExecution`). The paragraph below is
//! kept as history.
//!
//! A `Loop`'s own `errors:` and `finally:` run **in the sub-execution**, not in
//! the parent: `valids/all-flowable`'s sub-execution runs `1-each-1` (FAILED)
//! then `1-each-seq-error`, which is the `Loop` task's `errors:` list, while
//! the flow-level `errors:` (`error-1st`, `error-2nd`) run in the parent.
//! **W1's `models::Loop` carries neither field** — it holds `tasks`, `values`,
//! `concurrencyLimit` and `transmitFailed` and nothing else — so they are gone
//! before this module can see them and [`sub_flow_source`] cannot put them
//! back. Widening the model is W1's, not W3b's; the gap is named here and in
//! `oracle-exec/README.md` so Task 5 attributes the entries it costs rather
//! than discovering them.

use egret_core::{Error, Result};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{claim, WorkerError};
use crate::executor::{
    apply, plan_stored, resolve_loop_values, Command, ExecutionState, LoopValues, RunIx, RunState,
    TaskIx, TaskKind,
};
use crate::logs::LogLevel;
use crate::models::{Flow, Task, TaskKind as ModelTaskKind};

/// One [`Command::SpawnExecution`](crate::executor::Command::SpawnExecution),
/// kept as data.
///
/// `advance()`'s command carries no index, and it could not: it is emitted
/// once, in `values` order, by `flowable::loop_::open`. The index is derived
/// here from the loop's own resolved values — see [`open_batch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Requested {
    pub task: TaskIx,
    pub value: String,
    pub parent_run: RunIx,
}

impl Requested {
    /// The [`Command`] arm this is, or `None` for every other command.
    pub(super) fn of(cmd: &Command) -> Option<Self> {
        match cmd {
            Command::SpawnExecution {
                task,
                value,
                parent_run,
            } => Some(Requested {
                task: *task,
                value: value.clone(),
                parent_run: *parent_run,
            }),
            _ => None,
        }
    }
}

/// One loop run's batch of the spawns one round of `advance()` asked for:
/// every row created, and the jobs that settle them -- what the settle's
/// `Loops` phase runs, once per loop run (W9c Task 9 fix round 3: it returns
/// the iterations' first settles as [`claim::Job`]s for the walk to run, in
/// order, each to the end of its tail, where they used to run inline).
///
/// # The index is derived from the values, not from the emission order
///
/// Kestra picks the value **by index** — `either.getLeft().get(nextIndex)` in
/// `LoopExecutionEventMessageHandler`, where `nextIndex` is the number of
/// iterations started so far — so the index and the value are two views of one
/// position in the loop's resolved value list. Deriving the index from that
/// list rather than from the order the commands happen to arrive in means the
/// number is a fact about the flow, not about this process: it survives a
/// restart, and it is the same number [`on_execution_terminal`] computes for
/// the iterations `advance()` never emits.
///
/// **And the derivation is a guard.** The k-th spawn requested for a loop run
/// must be `values[already_spawned + k]`. If it is not, either `advance()` has
/// emitted something other than a prefix of the remaining values or the stored
/// `rendered` has changed under a running loop, and both are corruption rather
/// than a case to tolerate.
///
pub(super) async fn open_batch(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    state: &ExecutionState,
    parent_run: RunIx,
    batch: &[Requested],
) -> Result<Vec<claim::Job>> {
    let values = values_of(ctx, state, parent_run)?;
    let base = spawned_count(conn, ctx.id, parent_run).await?;
    let corrupt = |e: String| {
        worker_err(WorkerError::Corrupt(
            ctx.id,
            format!("loop run {parent_run}: {e}"),
        ))
    };
    // `(index, value)` of each spawn of the batch.
    let picked: Vec<(usize, String)> = match &values {
        LoopValues::Listed(values) => {
            let asked: Vec<&str> = batch.iter().map(|r| r.value.as_str()).collect();
            let indices = iteration_indices(values, base, &asked).map_err(corrupt)?;
            indices
                .into_iter()
                .zip(batch.iter().map(|r| r.value.clone()))
                .collect()
        }
        // A file's values are not in the state: `advance()` asked for
        // `batch.len()` more iterations (each carrying an empty value),
        // and the values are the next ones of the file, by index.
        LoopValues::FromFile { count } => {
            if base + batch.len() > *count {
                return Err(corrupt(format!(
                    "asked to spawn iterations {base}..{} but the file holds {count}",
                    base + batch.len()
                )));
            }
            file_values(conn, ctx.id, parent_run, base, batch.len())
                .await?
                .into_iter()
                .enumerate()
                .map(|(k, v)| (base + k, v))
                .collect()
        }
    };
    // **Every row of the batch first, then each one's first settle**
    // (W9c Task 9 fix round 2). An iteration that ENDS inside its first
    // settle tells its loop at once ([`on_execution_terminal`]), and the
    // loop counts the iterations started from the rows. Created one
    // settle at a time, the batch's later iterations had no row yet, so
    // the loop read "one left to start", started iteration `base + 1`
    // itself -- and this loop then created it again: `duplicate key ...
    // workflow_executions_loop_run_idx`, the whole transaction lost, for
    // a `concurrencyLimit: 0` loop whose first iteration took an empty
    // branch. With every row in place the loop counts the whole batch as
    // started, as Kestra's does (it emits the batch before any
    // iteration runs), and waits for it.
    if base == 0 {
        init_counters(conn, ctx.id, parent_run, values.len(), batch.len()).await?;
    }
    let mut jobs = Vec::with_capacity(batch.len() + 1);
    for (r, (index, value)) in batch.iter().zip(picked) {
        match insert_iteration(conn, ctx, parent_run, r.task, index, &value).await? {
            Ok(child) => jobs.push(claim::Job::Settle(Box::new(claim::Frame::first(child)))),
            // The loop run is failed (a copied FILE input no longer readable):
            // no further iteration of it is started. **Unless an iteration of
            // this batch has already ended the loop run** (re-review m1): its
            // terminal decides before this job runs, and completing a
            // terminal run again would append a second FAILED.
            Err(why) => {
                jobs.push(claim::Job::Complete(super::subflow::Completion {
                    execution_id: ctx.id,
                    run: parent_run,
                    states: vec![RunState::Failed],
                    outputs: None,
                    secret_outputs: None,
                    keep_outputs: true,
                    outputs_failed: None,
                    message: Some(format!("loop iteration {index}: {why}")),
                    stack: claim::Stack::Nested(ctx.caller()),
                    unless_terminal: true,
                }));
                break;
            }
        }
    }
    Ok(jobs)
}

/// Which iteration index each requested spawn is.
///
/// `base` is how many iterations of this loop run have already been started,
/// so the batch is `values[base..]`'s prefix and the k-th spawn is iteration
/// `base + k`.
///
/// **The derivation is also the guard.** Deriving the index from the values
/// list makes it a fact about the flow rather than about this process — it
/// survives a restart, and it is the same number [`on_execution_terminal`]
/// computes for the iterations `advance()` never emits — but it is only sound
/// while the requested values ARE that prefix. If they are not, either
/// `advance()` has emitted something other than a prefix of the remaining
/// values or the stored `rendered` changed under a running loop, and the index
/// and the value would then disagree with a sub-execution's row saying they
/// agreed. That is silent corruption of the one field
/// `oracle-exec/normalise.py` orders `spawned` by, so it is an error rather
/// than a case.
fn iteration_indices(
    values: &[String],
    base: usize,
    asked: &[&str],
) -> std::result::Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(asked.len());
    for (k, value) in asked.iter().enumerate() {
        let index = base + k;
        match values.get(index) {
            None => {
                return Err(format!(
                    "asked to spawn iteration {index} but its values hold only {}",
                    values.len()
                ))
            }
            Some(expected) if expected != value => {
                return Err(format!(
                    "spawn {k} carries value {value:?}, but iteration {index} of its \
                     values is {expected:?}; a spawn's index is its position in the \
                     loop's values and the two must agree"
                ))
            }
            Some(_) => out.push(index),
        }
    }
    Ok(out)
}

/// What one terminated iteration means for the loop run that spawned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// The loop run is over, in this state.
    Terminate(RunState),
    /// Start this iteration index.
    Start(usize),
    /// Iterations are still in flight; do nothing.
    Wait,
}

/// `LoopExecutionEventMessageHandler.handleTerminated`, as a decision.
///
/// Pure, and separate from the writing, because every branch of it is a claim
/// about Kestra that a corpus entry measures — see [`on_execution_terminal`]
/// for which entry pins which — and a claim nothing can drive is a claim
/// nothing checks.
///
/// `started` is how many iterations have been created, `terminated` how many of
/// those have finished (including the one being reported), `total` the number
/// of values.
fn decide(
    transmit_failed: bool,
    child: RunState,
    started: usize,
    terminated: usize,
    total: usize,
) -> Next {
    if transmit_failed && terminated_in_error(child) {
        // `terminateLoop`: FAILED becomes the task's failure state, anything
        // else becomes SUCCESS.
        Next::Terminate(if child == RunState::Failed {
            RunState::Failed
        } else {
            RunState::Success
        })
    } else if started < total {
        Next::Start(started)
    } else if terminated == total {
        Next::Terminate(RunState::Success)
    } else {
        Next::Wait
    }
}

/// Create one sub-execution and return its first settle as a job -- or,
/// when its FILE inputs are refused, the completion that fails the loop run.
/// What [`on_execution_terminal`]'s `Start` runs (W9c Task 9 fix rounds 2-3):
/// the walk runs the job, so the next iteration never nests inside the last.
///
/// The whole of it — the row, its task runs, and anything the sub-execution's
/// own first round asks for — is on the caller's connection and therefore in
/// the caller's transaction. A sub-execution that existed while its parent's
/// loop run did not know it had been spawned is the window the plan's Global
/// Constraint forbids.
async fn spawn_one(
    conn: &mut PgConnection,
    parent: &claim::ExecutionCtx,
    parent_run: RunIx,
    task: TaskIx,
    index: usize,
    value: &str,
) -> Result<claim::Job> {
    Ok(
        match insert_iteration(conn, parent, parent_run, task, index, value).await? {
            Ok(child) => claim::Job::Settle(Box::new(claim::Frame::first(child))),
            Err(why) => claim::Job::Complete(super::subflow::Completion {
                execution_id: parent.id,
                run: parent_run,
                states: vec![RunState::Failed],
                outputs: None,
                secret_outputs: None,
                keep_outputs: true,
                outputs_failed: None,
                message: Some(format!("loop iteration {index}: {why}")),
                stack: claim::Stack::Nested(parent.caller()),
                unless_terminal: false,
            }),
        },
    )
}

/// Insert one iteration's row and check its FILE inputs, in a savepoint:
/// its context, or why the inputs are refused (the row rolled back).
async fn insert_iteration(
    conn: &mut PgConnection,
    parent: &claim::ExecutionCtx,
    parent_run: RunIx,
    task: TaskIx,
    index: usize,
    value: &str,
) -> Result<std::result::Result<claim::ExecutionCtx, String>> {
    let loop_id = parent
        .plan
        .tasks
        .get(task)
        .map(|t| t.id.as_str())
        .ok_or_else(|| {
            worker_err(WorkerError::Corrupt(
                parent.id,
                format!("a spawn names task {task}, which the plan does not hold"),
            ))
        })?;
    let source = sub_flow_source(&parent.source, loop_id, parent.id)?;
    let flow = crate::spec::parse(&source)
        .map_err(|e| worker_err(WorkerError::Unplannable(parent.id, e.message)))?;
    // The parent's own stored source, cut down: planned by the rules the
    // parent started under (`executor::plan_stored`; user ruling 2026-10-12).
    let plan = plan_stored(&source)
        .map_err(|e| worker_err(WorkerError::Unplannable(parent.id, e.to_string())))?;

    // **`inputs` is the PARENT's, and it has to be copied here.**
    //
    // `render::Surface::load` reads `inputs` for the current execution and only
    // that one — unlike `outputs`, which walks the `parent_execution_id` chain
    // — so a sub-execution created without them renders `{{ inputs.x }}`
    // against 0136's `DEFAULT '{}'` and takes the other branch. Corpus-
    // reachable and not hypothetical: `valids/finally-loop` declares
    // `failed: BOOL` defaulting `false`, `corpus.json` posts `true`, and the
    // loop BODY branches on `{{ inputs.failed == false }}` — so egret ran the
    // `then` where Kestra ran the `else`. Kestra passes the parent's execution
    // context down; a `Loop` iteration is not a new execution of a different
    // flow.
    //
    // Read out of the parent row in the same statement rather than carried on
    // `ExecutionCtx`: the row is the one authority on what this execution was
    // started with, and a field copied onto the context is a second copy that
    // can disagree with it. A chain of nested loops therefore inherits
    // transitively, one level at a time, because each parent already holds what
    // it inherited.
    //
    // **`execution_depth` is the PARENT's too, not one more** (W13a final
    // review I1). `Execution.loopExecution` on the pinned artifact (javap)
    // builds the iteration with the parent's own `metadata` field, so its
    // `executionDepth` is the parent's, and with `ExecutionKind.LOOP` -- the
    // kind `flow_trigger` reads off `loop_index` to refuse an iteration as a
    // Flow-trigger source. Read from the row for the same reason as `inputs`.
    // **`variables` is the parent's too** (group C2 final review I2, ruling
    // D10): `Execution.loopExecution` builds the iteration with
    // `this.variables`, so a `SetVariables` before the loop is visible in its
    // body. A copy at spawn time, as Kestra's is: an update inside the
    // iteration changes only the iteration. The parent's settle writes its
    // render side (`claim::write_render_side`) before `open_batch` spawns, in
    // the same transaction, so this reads the value that settle computed.
    //
    // In a savepoint, for `subflow::spawn_one`'s reason: the FILE-input check
    // below needs the row and may refuse it.
    let mut sp = sqlx::Connection::begin(&mut *conn).await.map_err(db_err)?;
    let (id, inputs): (Uuid, serde_json::Value) = sqlx::query_as(concat!(
        "INSERT INTO workflow.executions \
               (workspace_id, flow_id, namespace, flow_key, source, state, history, \
                parent_execution_id, parent_run_ix, loop_index, loop_value, inputs, \
                execution_depth, labels, variables, secret_inputs, ",
        super::stamps::stamp_columns!(),
        ") VALUES ($1,$2,$3,$4,$5,'CREATED','[\"CREATED\"]'::jsonb,$6,$7,$8,$9, \
                 COALESCE((SELECT inputs FROM workflow.executions WHERE id = $6), '{}'::jsonb), \
                 (SELECT execution_depth FROM workflow.executions WHERE id = $6), \
                 (SELECT labels FROM workflow.executions WHERE id = $6), \
                 (SELECT variables FROM workflow.executions WHERE id = $6), \
                 (SELECT secret_inputs FROM workflow.executions WHERE id = $6), ",
        super::stamps::stamp_values!("'CREATED'", "'[\"CREATED\"]'"),
        ") RETURNING id, inputs",
    ))
    .bind(parent.workspace_id)
    .bind(parent.flow_id)
    .bind(&flow.namespace)
    .bind(&flow.id)
    .bind(&source)
    .bind(parent.id)
    .bind(parent_run as i32)
    .bind(index as i32)
    .bind(value)
    .fetch_one(&mut *sp)
    .await
    .map_err(db_err)?;

    // W9a spec §7.1: every execution-creating INSERT checks its FILE inputs.
    // An iteration's are its parent's, already checked when the parent was
    // created, and the parent is in the iteration's loop family -- so this
    // passes unless a copied value is no longer readable (its index row
    // pruned by retention, spec §10). That **fails the loop run naming the
    // input** (fix round 1, M4) -- the iteration's row rolled back -- rather
    // than erring the worker, which would re-claim the run for ever.
    let checked = crate::storage::inputs::check_file_inputs(
        &mut sp,
        parent.workspace_id,
        id,
        &flow,
        &inputs,
        &[],
    )
    .await?;
    if let Err(why) = checked {
        sp.rollback().await.map_err(db_err)?;
        return Ok(Err(why));
    }
    sp.commit().await.map_err(db_err)?;

    Ok(Ok(claim::ExecutionCtx {
        id,
        workspace_id: parent.workspace_id,
        flow_id: parent.flow_id,
        source,
        plan,
        // A sub-execution's logical clock starts at zero, like its `Tick`: the
        // happens-before relation is compared PER EXECUTION (`normalise.py`
        // builds one pair table per node), so a shared counter would be a
        // number two traces read differently.
        seq: 0,
        // **One deeper than its caller**, because the `settle` below runs
        // inside this one. A `Loop` is never refused on it -- see
        // `ExecutionCtx::depth` -- but its frames are real and are counted, so
        // a `Subflow` nested inside loops carries the loops it sits in toward
        // `subflow::MAX_SPAWN_CHAIN`. **That is not a bound on the loop
        // nesting itself**, which nothing refuses: see that constant's
        // "What neither bound closes".
        depth: parent.depth + 1,
        // The same cache as its caller: this settle runs in the caller's
        // transaction (W9c Task 9, `claim::Stack`).
        gate: parent.gate.clone(),
    }))
}

/// One sub-execution has reached a terminal state: tell the loop that spawned
/// it.
///
/// This is the half of `Loop` that `advance()` cannot own. It is pure over
/// `(plan, state, now)` and [`ExecutionState`] holds nothing about a
/// sub-execution — no id, no state, no count — so "spawn the next iteration
/// when one terminates" and "the loop is done" both have to live in a caller
/// that can see the sub-executions. That caller is this.
///
/// # The rule, and where each branch is measured
///
/// `LoopExecutionEventMessageHandler.handleTerminated` in Kestra, with the
/// three branches pinned by three corpus entries rather than by the class:
///
/// - **`transmitFailed` and a terminal-in-error** → the loop ends now, with no
///   further iteration. `valids/loop-failed` is a limit-1 loop over three
///   values whose body always fails: the artifact ran **one** sub-execution and
///   the loop run FAILED.
/// - **an iteration is left to start** → start it, at index "the number
///   started so far". `valids/loop-parallel-less` is limit 2 over four values
///   and the artifact ran **four** sub-executions; `advance()` emitted two.
/// - **all iterations terminated** → the loop SUCCEEDS, *whatever* they
///   terminated as. `valids/loop-failed-no-transmit` is the same failing body
///   with `transmitFailed: false`: three FAILED sub-executions and a
///   SUCCESSFUL loop run.
///
/// # Two readings taken from the class and NOT measurable here
///
/// Named rather than left implicit, because the checkout is not the artifact:
///
/// - `isTerminatedInError()` is `FAILED || KILLED || CANCELLED`
///   (`State.java:296`). Only `FAILED` is reachable from anything the corpus
///   runs, and `RunState` has no `CANCELLED`.
/// - `terminateLoop` computes `state == FAILED ? stateFailure(task) : SUCCESS`,
///   so a **KILLED** iteration ends a `transmitFailed` loop with **SUCCESS**.
///   That reads like a defect in Kestra and no corpus entry reaches it; it is
///   reproduced because this is a differential oracle and a deliberate
///   divergence needs a reason better than "it looks wrong".
///
/// # `outputs` (package #6 Task A3, spec D1)
///
/// The iteration's typed `outputs:` map, as [`iteration_outputs`] evaluated
/// it in the iteration's own settle -- `None` when the `Loop` declares no
/// `outputs:` (or the report is not an iteration's, or the evaluation failed
/// and `state` is therefore `FAILED`). Task A4 appends it to the `Loop` run's
/// list under the lock taken below, beside the counters (spec D3, D4): see
/// [`LoopRunBook`]. SECRET values are stored as the mask and sealed into
/// `task_runs.secret_outputs` (spec D9, brought forward from Task B4).
pub(super) async fn on_execution_terminal(
    conn: &mut PgConnection,
    execution_id: Uuid,
    state: RunState,
    outputs: Option<crate::outputs::Typed>,
    caller: claim::Caller,
) -> Result<Option<claim::Job>> {
    // Async recursion: a loop body may itself contain a `Loop`
    // (`valids/loop-nested`), so terminating the innermost sub-execution walks
    // up the chain, and each level is a nested `settle` inside the caller's
    // transaction. The chain cannot **cycle** — `parent_execution_id` always
    // names a row that existed before this one was inserted — but it can be
    // arbitrarily **deep**, and nothing here refuses it.
    //
    // **The comment that used to sit here cited `flowable::descend`'s reason
    // for having no depth guard, and it does not transfer.** `descend`'s
    // recursion is bounded by `state.runs.len()` for any plan; this one is
    // bounded only by how deeply an author nested `Loop`s, which `build_plan`
    // caps at `plan::MAX_TASK_DEPTH` = 48. Measured before W9c Task 9 fix
    // round 3: 12 nested one-value `Loop`s ran at `ctx.depth = 12` with no
    // `Subflow` anywhere, and **47 aborted the process with a stack
    // overflow** -- an absent guard on a reachable recursion. **Measured at
    // round 3 (re-review 3, 2026-10-09): 46 nested `Loop`s run to SUCCESS
    // (47 executions) and 47 is refused by the planner's 48-level cap**, so
    // static nesting no longer reaches an abort; see the round 2-3 paragraph
    // below for why.
    // It predates W6 (this recursion is W3b `ad93ef2d`, the cap is W3a
    // `6f046e57`) and it is recorded with its owner and with what the artifact
    // does instead on `subflow::MAX_SPAWN_CHAIN`.
    //
    // **W9c Task 9 fix rounds 2-3: neither the ascent nor the descent is
    // recursion any more.** The settle this starts -- the next iteration's
    // first, or the loop run's completion -- is returned as a `claim::Job`,
    // and so is every settle a head starts (a batch's first settles, a
    // `Subflow` child's, a refused `Subflow`'s completion). Only
    // `claim::walk` runs a settle, from an explicit stack, so the Rust stack
    // is the same at any iteration count and any nesting (re-review N1: 66
    // iterations of an `If` with no `else`; C2: 10 iterations of a refused
    // `Subflow`; C3: 50 iterations of a sequential `Loop` in a sequential
    // `Loop` -- each aborted the process). `ExecutionCtx::depth` still counts
    // every link, so `subflow::MAX_SPAWN_CHAIN` refuses what it refused.
    Box::pin(on_execution_terminal_inner(
        conn,
        execution_id,
        state,
        outputs,
        caller,
    ))
    .await
}

async fn on_execution_terminal_inner(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
    // Task A4 (spec D3, D4) appends it, under the parent lock `claim::load`
    // takes below.
    outputs: Option<crate::outputs::Typed>,
    caller: claim::Caller,
) -> Result<Option<claim::Job>> {
    let Some((parent_id, parent_run)) = parent_of(conn, execution_id).await? else {
        return Ok(None);
    };
    // **`Nested`, and the fix round 2 Critical is that this used to be `0`.**
    // This is called from `claim::settle` step 4, so the `settle` at the bottom
    // of this function nests inside the one that called it -- a `Loop` that
    // starts its next iteration from here is one more frame, not a fresh
    // stack. See `claim::Stack`.
    let (ctx, mut state, now) = claim::load(conn, parent_id, claim::Stack::Nested(caller)).await?;
    let before = state.clone();

    let run = usize::try_from(parent_run).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!("negative parent_run_ix {parent_run}"),
        ))
    })?;
    let task = state.runs.get(run).map(|r| r.task).ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!("a sub-execution names run {run}, which the parent does not hold"),
        ))
    })?;
    if !matches!(ctx.plan.tasks[task].kind, TaskKind::Loop { .. }) {
        return Err(worker_err(WorkerError::Corrupt(
            parent_id,
            format!(
                "run {run} spawned a sub-execution but its task {:?} is not a Loop",
                ctx.plan.tasks[task].id
            ),
        )));
    }
    // Already decided. A `transmitFailed` loop ends on the FIRST failure and
    // its siblings terminate afterwards; without this they would each try to
    // re-terminate a finished run and the last one would win.
    if state.runs[run]
        .history
        .last()
        .is_some_and(|s| s.is_terminal())
    {
        return Ok(None);
    }

    let values = values_of(&ctx, &state, run)?;
    let started = spawned_count(conn, parent_id, run).await?;
    let terminated = terminated_count(conn, parent_id, run).await?;
    let transmit = transmit_failed(&ctx, task)?;
    let mut child = as_the_loop_reads_it(child_state);

    // **The list, the counters and the total cap (package #6 Task A4, spec
    // D3, D4; `LoopExecutionEventMessageHandler.handleTerminated`).** Under
    // the parent row lock `claim::load` took above, so two iterations ending
    // at once queue here and neither loses the other's entry.
    //
    // A terminal-in-error under `transmitFailed` ends the loop without
    // touching either (Kestra's first branch: `terminateLoop` alone, so the
    // counters stay as the last iteration that went the other way left them,
    // and the failing iteration's outputs are not appended).
    if !(transmit && terminated_in_error(child)) {
        let mut book = LoopRunBook::load(conn, parent_id, run).await?;
        let entry = match outputs.filter(|t| !t.values.is_empty()) {
            Some(typed) => Some(iteration_entry(conn, execution_id, &state, run, typed).await?),
            None => None,
        };
        // The 16 MiB total, decided here because only the parent sees the
        // whole list: an over-cap iteration is a FAILED iteration, its
        // entry not appended and the message logged on the `Loop` run (the
        // sub-execution's own history is not rewritten after it ended).
        let mut entry = entry.map(|e| (e, 0));
        if let Some((e, total)) = &mut entry {
            let cap = crate::outputs::loop_list_cap(ctx.workspace_id);
            match book.would_fit(e, cap) {
                Ok(fits) => *total = fits,
                Err(message) => {
                    log_line(
                        conn,
                        ctx.workspace_id,
                        parent_id,
                        Some(book.run_id),
                        LogLevel::Error,
                        &message,
                        &[],
                    )
                    .await?;
                    entry = None;
                    child = RunState::Failed;
                }
            }
        }
        if !(transmit && terminated_in_error(child)) {
            let next = decide(transmit, child, started, terminated, values.len());
            let started_after = started + usize::from(matches!(next, Next::Start(_)));
            book.count(
                values.len(),
                started_after.saturating_sub(terminated),
                child,
            );
            if let Some((e, total)) = entry {
                book.append(e, total)?;
            }
            book.store(conn).await?;
        }
    }

    let terminal = match decide(transmit, child, started, terminated, values.len()) {
        Next::Terminate(s) => s,
        Next::Start(index) => {
            let value = match &values {
                LoopValues::Listed(values) => values[index].clone(),
                LoopValues::FromFile { .. } => {
                    file_values(conn, parent_id, run, index, 1).await?.remove(0)
                }
            };
            return spawn_one(conn, &ctx, run, task, index, &value)
                .await
                .map(Some);
        }
        Next::Wait => return Ok(None),
    };
    apply(
        &mut state,
        &Command::SetRunState {
            run,
            state: terminal,
        },
    );
    Ok(Some(claim::Job::Settle(Box::new(claim::Frame::loaded(
        ctx, before, state, now,
    )))))
}

/// One iteration's entry for its `Loop` run's list (spec D3): `{item: {value,
/// iteration, key?}, outputs}`, with each SECRET value replaced by
/// [`MASK`](crate::logs::MASK) and kept aside for sealing (spec D9).
struct Entry {
    entry: serde_json::Value,
    secrets: serde_json::Map<String, serde_json::Value>,
}

/// Build `typed`'s entry for the iteration `child` of loop run `run`.
///
/// `item.value` is the iteration's value as a string (its row's
/// `loop_value`), `iteration` its 0-based index, and `key` present only for a
/// MAP `values:` -- the nth key of the loop run's own `rendered` object, as
/// `worker::render`'s `item.key` reads it (Kestra's `loopRun.key()`).
async fn iteration_entry(
    conn: &mut PgConnection,
    child: Uuid,
    state: &ExecutionState,
    run: RunIx,
    typed: crate::outputs::Typed,
) -> Result<Entry> {
    let (index, value): (i32, Option<String>) = sqlx::query_as(
        "SELECT loop_index, loop_value FROM workflow.executions \
          WHERE id = $1 AND loop_index IS NOT NULL",
    )
    .bind(child)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut item = serde_json::Map::new();
    item.insert(
        "value".into(),
        serde_json::Value::from(value.unwrap_or_default()),
    );
    item.insert("iteration".into(), serde_json::Value::from(index));
    if let Some(key) = state.runs[run]
        .rendered
        .as_deref()
        .and_then(|r| map_key(r, index))
    {
        item.insert("key".into(), serde_json::Value::String(key));
    }
    let mut values = typed.values;
    let mut secrets = serde_json::Map::new();
    for id in &typed.secret_ids {
        if let Some(v) = values.get_mut(id) {
            secrets.insert(
                id.clone(),
                std::mem::replace(v, serde_json::Value::from(crate::logs::MASK)),
            );
        }
    }
    Ok(Entry {
        entry: serde_json::json!({"item": item, "outputs": values}),
        secrets,
    })
}

/// The `index`th key of a MAP `values:` rendered as `rendered`, or `None`
/// for any other shape (a list, a file).
fn map_key(rendered: &str, index: i32) -> Option<String> {
    if !rendered.trim_start().starts_with('{') {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(rendered).ok()? {
        serde_json::Value::Object(m) => m.keys().nth(usize::try_from(index).ok()?).cloned(),
        _ => None,
    }
}

/// A `Loop` run's `task_runs.outputs`, `secret_outputs` and `outputs_bytes`,
/// read and written under the parent lock (spec D3, D4):
/// `iterationCount`, `runningIterations`, `terminatedIterations` (state name
/// -> count) and, once an entry was appended, `outputs` (the list, in
/// termination order).
///
/// **Work per iteration end is the entry's, not the list's** (package #6 fix
/// wave 6, final review I1). [`LoopRunBook::load`] reads the counters (the
/// object without its list, `outputs - 'outputs'`), the list's length and
/// the running byte total -- never the list; [`LoopRunBook::would_fit`]
/// checks the cap from that total and this entry alone; and
/// [`LoopRunBook::store`] appends the entry in SQL (`outputs -> 'outputs' ||
/// entry`) and its SECRET values as one more sealed frame
/// (`secret_outputs || frame`, [`seal_loop_entry`](crate::secret_inputs::seal_loop_entry)),
/// never re-sealing the earlier ones. What the worker sends and receives,
/// and what it parses, serialises and seals, is therefore the counters plus
/// one entry at any list length; `the_loop_book_moves_one_entry_per_
/// iteration_end_at_any_list_length` (`tests/worker.rs`) counts it.
/// Postgres itself still writes the whole `jsonb` value (and its TOAST and
/// WAL) on each `UPDATE`: a `jsonb` column is not updated in place. That
/// residual is bounded by the 16 MiB cap and recorded in the spec (§7).
struct LoopRunBook {
    execution: Uuid,
    run: RunIx,
    run_id: Uuid,
    /// `outputs` without its list: the three counters.
    counters: serde_json::Map<String, serde_json::Value>,
    /// The list's length, read in SQL: the next entry's position.
    len: usize,
    /// `task_runs.outputs_bytes`: the list's serialised bytes plus the clear
    /// size of every SECRET value in it (spec D4, final review I3). `None`
    /// until the first append.
    bytes: Option<usize>,
    /// This iteration end's entry, its sealed frame and the new total, set
    /// by [`LoopRunBook::append`].
    appended: Option<(serde_json::Value, Option<Vec<u8>>, usize)>,
    /// The bytes this book sent to and received from the database, for the
    /// I1 benchmark (`test-util`).
    io: usize,
}

/// The length of `v` serialised, `0` for the impossible error (a positive
/// cap then reads it as "no bytes").
fn json_len(v: &serde_json::Value) -> usize {
    serde_json::to_vec(v).map_or(0, |v| v.len())
}

impl Entry {
    /// **The entry's size for the 16 MiB total, its SECRET values in clear**
    /// (final review I3; as `flow_outputs::cap` counts a flow's): the
    /// masked entry as it is stored, with each `"******"` replaced by the
    /// size of the value it hides. So a SECRET-heavy `Loop` cannot grow its
    /// sealed column past the cap while its masked list stays small.
    fn size(&self) -> usize {
        let mask = json_len(&serde_json::Value::from(crate::logs::MASK));
        self.secrets.values().fold(json_len(&self.entry), |n, v| {
            n.saturating_add(json_len(v)).saturating_sub(mask)
        })
    }
}

impl LoopRunBook {
    async fn load(conn: &mut PgConnection, execution: Uuid, run: RunIx) -> Result<Self> {
        let (run_id, counters, len, bytes): (Uuid, Option<serde_json::Value>, i32, Option<i64>) =
            sqlx::query_as(
                "SELECT id, outputs - 'outputs', \
                        COALESCE(jsonb_array_length(outputs -> 'outputs'), 0), outputs_bytes \
                   FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
            )
            .bind(execution)
            .bind(run as i32)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
        let io = 16 + 4 + 8 + counters.as_ref().map_or(0, json_len);
        let counters = match counters {
            Some(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        Ok(Self {
            execution,
            run,
            run_id,
            counters,
            len: usize::try_from(len).unwrap_or(0),
            bytes: bytes.and_then(|b| usize::try_from(b).ok()),
            appended: None,
            io,
        })
    }

    /// `Ok(total)` -- the running total once `e` is appended -- or
    /// `Err(message)` when that takes the list past `cap` bytes (spec D4's
    /// 16 MiB variant: it names the total). `[]` counts 2 bytes, each entry
    /// its [`Entry::size`], and each entry after the first one comma, so the
    /// total is the list's serialised size with every SECRET value in clear.
    fn would_fit(&self, e: &Entry, cap: usize) -> std::result::Result<usize, String> {
        let total = self
            .bytes
            .unwrap_or(2)
            .saturating_add(e.size())
            .saturating_add(usize::from(self.len > 0));
        if total <= cap {
            return Ok(total);
        }
        let iteration = e.entry["item"]["iteration"].as_i64().unwrap_or(-1);
        Err(total_cap_message(iteration, total, cap))
    }

    /// The three counters after one more terminated iteration in `child`.
    fn count(&mut self, iteration_count: usize, running: usize, child: RunState) {
        let mut terminated = match self.counters.remove("terminatedIterations") {
            Some(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        let n = terminated
            .get(child.wire())
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        terminated.insert(child.wire().to_string(), serde_json::Value::from(n + 1));
        self.counters
            .insert("iterationCount".into(), iteration_count.into());
        self.counters
            .insert("runningIterations".into(), running.into());
        self.counters.insert(
            "terminatedIterations".into(),
            serde_json::Value::Object(terminated),
        );
    }

    /// Append `e` at position [`LoopRunBook::len`], `total` being what
    /// [`LoopRunBook::would_fit`] answered: its SECRET values are sealed
    /// here, as this position's frame, and nothing else is.
    fn append(&mut self, e: Entry, total: usize) -> Result<()> {
        let frame = if e.secrets.is_empty() {
            None
        } else {
            Some(crate::secret_inputs::seal_loop_entry(self.len, &e.secrets)?)
        };
        self.appended = Some((e.entry, frame, total));
        Ok(())
    }

    /// Write the counters and, when [`LoopRunBook::append`] ran, the entry,
    /// its frame and the new total -- each appended in SQL.
    async fn store(mut self, conn: &mut PgConnection) -> Result<usize> {
        let (entry, frame, total) = match self.appended.take() {
            Some((entry, frame, total)) => (Some(entry), frame, i64::try_from(total).ok()),
            None => (None, None, None),
        };
        let counters = serde_json::Value::Object(std::mem::take(&mut self.counters));
        self.io += 16
            + 4
            + json_len(&counters)
            + entry.as_ref().map_or(0, json_len)
            + frame.as_ref().map_or(0, Vec::len)
            + 8;
        sqlx::query(
            "UPDATE workflow.task_runs \
                SET outputs = $3::jsonb || CASE \
                      WHEN $4::jsonb IS NOT NULL THEN jsonb_build_object('outputs', \
                           COALESCE(outputs -> 'outputs', '[]'::jsonb) || jsonb_build_array($4::jsonb)) \
                      WHEN outputs ? 'outputs' THEN jsonb_build_object('outputs', outputs -> 'outputs') \
                      ELSE '{}'::jsonb END, \
                    outputs_bytes = COALESCE($5, outputs_bytes), \
                    secret_outputs = CASE WHEN $6::bytea IS NULL THEN secret_outputs \
                      ELSE COALESCE(secret_outputs, ''::bytea) || $6::bytea END \
              WHERE execution_id = $1 AND run_ix = $2",
        )
        .bind(self.execution)
        .bind(self.run as i32)
        .bind(counters)
        .bind(entry)
        .bind(total)
        .bind(frame)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(self.io)
    }
}

/// **Test-only (`test-util`): one iteration end's book-keeping on a `Loop`
/// run, as `on_execution_terminal_inner` does it** -- load, the cap, the
/// counters, the append, the store -- for the I1 benchmark of package #6
/// fix wave 6. `entry` is the masked entry, `secrets` the SECRET values it
/// masks. Answers the bytes the book moved, or the cap's message.
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub async fn append_loop_entry_for_tests(
    conn: &mut PgConnection,
    execution: Uuid,
    run: RunIx,
    entry: serde_json::Value,
    secrets: serde_json::Map<String, serde_json::Value>,
    cap: usize,
) -> Result<std::result::Result<usize, String>> {
    let mut book = LoopRunBook::load(conn, execution, run).await?;
    let e = Entry { entry, secrets };
    let total = match book.would_fit(&e, cap) {
        Ok(total) => total,
        Err(message) => return Ok(Err(message)),
    };
    let n = book.len + 1;
    book.count(n, 0, RunState::Success);
    book.append(e, total)?;
    Ok(Ok(book.store(conn).await?))
}

/// Spec D4's message for the 16 MiB total, naming it.
fn total_cap_message(iteration: i64, total: usize, cap: usize) -> String {
    use crate::runners::env::MAX_LOOP_ITERATION_OUTPUT_BYTES;
    format!(
        "Loop outputs of iteration {iteration} would bring the Loop's outputs to {total} bytes; \
         egret keeps at most {} per iteration and {} per Loop in the execution. Use fetchType: \
         STORE to keep them in internal storage.",
        bytes_text(MAX_LOOP_ITERATION_OUTPUT_BYTES),
        bytes_text(cap),
    )
}

/// `n` as `<k> MiB` when it is a whole number of MiB, else `<n> bytes`.
pub(crate) fn bytes_text(n: usize) -> String {
    if n > 0 && n.is_multiple_of(1 << 20) {
        format!("{} MiB", n >> 20)
    } else {
        format!("{n} bytes")
    }
}

/// **The counters a `Loop` run carries from its first batch** (spec D4):
/// Kestra's `ExecutorService` saves `iterationCount` (the number of values),
/// `runningIterations` (the first batch) and an empty `terminatedIterations`
/// when it starts the loop, and `handleTerminated` updates them. Written
/// once, by the first batch (`base == 0`), and only onto a run with no
/// outputs yet.
async fn init_counters(
    conn: &mut PgConnection,
    execution: Uuid,
    run: RunIx,
    iteration_count: usize,
    running: usize,
) -> Result<()> {
    sqlx::query(
        "UPDATE workflow.task_runs SET outputs = $3 \
          WHERE execution_id = $1 AND run_ix = $2 AND outputs IS NULL",
    )
    .bind(execution)
    .bind(run as i32)
    .bind(serde_json::json!({
        "iterationCount": iteration_count,
        "runningIterations": running,
        "terminatedIterations": {},
    }))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// **A `Loop` with no values carries the counters too** (spec D4): Kestra
/// saves `{iterationCount: 0, runningIterations: 0, terminatedIterations:
/// {}}` and ends the run `SUCCESS` (`ExecutorService`, `init.totalCount() ==
/// 0`). Called after a settle's `persist`: every `Loop` run this settle
/// moved to `SUCCESS` whose resolved values are empty gets them.
pub(super) async fn empty_loop_counters(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    before: &ExecutionState,
    state: &ExecutionState,
) -> Result<()> {
    for (ix, r) in state.runs.iter().enumerate() {
        if !matches!(ctx.plan.tasks[r.task].kind, TaskKind::Loop { .. })
            || r.history.last() != Some(&RunState::Success)
            || before
                .runs
                .get(ix)
                .and_then(|b| b.history.last())
                .is_some_and(|s| s.is_terminal())
        {
            continue;
        }
        let empty = r
            .rendered
            .as_deref()
            .and_then(|x| resolve_loop_values(x).ok())
            .is_some_and(|v| v.is_empty());
        if empty {
            init_counters(conn, ctx.id, ix, 0, 0).await?;
        }
    }
    Ok(())
}

/// What the end of one LOOP iteration's sub-execution evaluates: its `Loop`'s
/// `outputs:` (package #6 Task A3, spec D1).
#[derive(Debug)]
pub(super) enum IterationEnd {
    /// Not a `Loop` iteration, or its `Loop` declares no `outputs:`: nothing
    /// was evaluated and the report carries no map.
    NoOutputs,
    /// The typed map, for [`on_execution_terminal`] to append (Task A4).
    /// SECRET values are in clear here, in memory only: the append masks and
    /// seals them (spec D9).
    Typed(crate::outputs::Typed),
    /// The evaluation failed (a required output's render, a coercion, or the
    /// per-iteration cap). The error is already logged on the sub-execution;
    /// the caller appends `FAILED` after the terminal and reports `FAILED`.
    Failed,
}

/// **Evaluate a LOOP iteration's `outputs:` at its terminal** (package #6
/// Task A3, spec D1; Kestra `DefaultExecutor`'s terminal block,
/// `loop.computeIterationOutput(runContext, execution)`).
///
/// Called from `claim`'s outbox phase in the settle where the sub-execution
/// becomes terminated, so after its `errors:`/`finally:` -- they run inside
/// the sub-execution -- and on that settle's own render surface (its task
/// outputs, `item`, the outputs it inherits from its loop ancestors). Kestra
/// evaluates on **any** terminal (its block is not gated on the state), and
/// so does this.
///
/// The `Loop` is read from the **parent's** source: the iteration's own flow
/// ([`sub_flow_source`]) is the `Loop`'s body and does not hold it. One
/// indexed read, done only for a row with a `loop_index` (a replay or a
/// `Subflow` child has none, so neither evaluates anything here).
///
/// Every line it writes goes to the sub-execution's log, masked with the
/// render context's redactions (the SECRET inputs it renders against):
/// Kestra's `Failed to render output values: <cause>` at ERROR, and one WARN
/// per optional output that failed to render (`Typed::warnings`).
///
/// **SECRET and FILE (spec D9, D10; Task A4 brought both forward from Task
/// B4, so no task of the package stores either unchecked).** A SECRET output
/// with no master key installed fails the iteration naming the output and
/// `secrets.master_key`; its value is masked in every line written after the
/// evaluation. A FILE output whose value is not a readable file of this
/// execution's loop family fails it with `egret refuses output <id>: <uri> is
/// not a file of this execution` (`storage::inputs::check_file_outputs`).
///
/// **The cap (spec D4, Q5).** A map that serialises to more than
/// [`MAX_LOOP_ITERATION_OUTPUT_BYTES`](crate::runners::env::MAX_LOOP_ITERATION_OUTPUT_BYTES)
/// fails the iteration, naming `fetchType: STORE`. **Corrected 2026-10-29
/// (Task A4):** A3 exempted an explicit `fetchType: STORE`, whose map Task A5
/// will write to storage; A4 stores every map in the `Loop` run's list until
/// A5 lands, so the cap now applies to every `fetchType`, and A5 puts its
/// STORE branch before it. **Task A5 (2026-10-29):** done -- the cap applies
/// to FETCH only (`fetchType: FETCH`, or `AUTO` resolved to it).
///
/// **STORE and AUTO (Task A5, spec D5).** [`fetch_mode`] resolves `AUTO`
/// from the `Loop` run's rendered `values:`; under STORE a SECRET output is
/// refused before anything renders, and the typed map goes to the
/// iteration's Ion file ([`store_iteration_outputs`]), the entry holding
/// `{uri}`.
pub(super) async fn iteration_outputs(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    surface: &super::render::Surface,
    state: &ExecutionState,
) -> Result<IterationEnd> {
    let Some(row) = sqlx::query(
        "SELECT p.source, tr.task_id, tr.rendered, c.loop_index \
           FROM workflow.executions c \
           JOIN workflow.executions p ON p.id = c.parent_execution_id \
           JOIN workflow.task_runs tr \
             ON tr.execution_id = p.id AND tr.run_ix = c.parent_run_ix \
          WHERE c.id = $1 AND c.loop_index IS NOT NULL",
    )
    .bind(ctx.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    else {
        return Ok(IterationEnd::NoOutputs);
    };
    let parent_source: String = row.get("source");
    let task_id: String = row.get("task_id");
    let index: i32 = row.get("loop_index");
    let loop_rendered: Option<String> = row.get("rendered");
    let flow = crate::spec::parse(&parent_source)
        .map_err(|e| worker_err(WorkerError::Unplannable(ctx.id, e.message)))?;
    let the_loop = match find_task(&flow, &task_id).map(|t| &t.kind) {
        Some(ModelTaskKind::Loop(l)) => l,
        _ => {
            return Err(worker_err(WorkerError::Corrupt(
                ctx.id,
                format!("the iteration's run names task {task_id:?}, which is not a Loop of its parent's source"),
            )))
        }
    };
    if the_loop.outputs.is_empty() {
        return Ok(IterationEnd::NoOutputs);
    }
    let store = fetch_mode(the_loop.fetch_type(), loop_rendered.as_deref()) == FetchMode::Store;
    // **Spec D9 at evaluation (package #6 Task A5, spec §2.2 item 8):** a
    // SECRET output under STORE is refused at save, and a version stored
    // before #6 (or an `AUTO` whose `values:` renders to a file only at run)
    // is refused here, before anything is rendered: the file would hold the
    // value in clear. The message names the outputs, never a value.
    if store {
        let secret: Vec<&str> = the_loop
            .outputs
            .iter()
            .filter(|o| o.r#type == Some(crate::models::InputType::Secret))
            .map(|o| o.id.as_str())
            .collect();
        if !secret.is_empty() {
            let message = secret_under_store_message(&secret, the_loop.fetch_type());
            log_on_execution(conn, ctx, LogLevel::Error, &message, &[]).await?;
            return Ok(IterationEnd::Failed);
        }
    }

    let cx = surface.context(&ctx.plan, state, None);
    let redactions: Vec<String> = cx.redactions().to_vec();
    let evaluated = crate::outputs::evaluate(&cx, &the_loop.outputs);
    let typed = match evaluated {
        Ok(typed) => typed,
        Err(e) => {
            let message = format!("Failed to render output values: {e}");
            log_on_execution(conn, ctx, LogLevel::Error, &message, &redactions).await?;
            return Ok(IterationEnd::Failed);
        }
    };
    // The SECRET outputs' own values are masked too in every line written
    // from here on (spec D9).
    let mut redactions = redactions;
    redactions.extend(typed.secret_ids.iter().filter_map(|id| {
        typed.values.get(id).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    }));
    for warning in &typed.warnings {
        log_on_execution(conn, ctx, LogLevel::Warn, warning, &redactions).await?;
    }
    // **Spec D9, brought forward from Task B4 (Task A4 is the first writer of
    // the map):** a SECRET value is stored only sealed, so a deployment with
    // no master key refuses the output, naming it and the setting, as a
    // SECRET input start is refused.
    if !typed.secret_ids.is_empty() && !crate::secret_inputs::can_seal() {
        let message = crate::secret_inputs::no_key_for_secret_outputs(&typed.secret_ids);
        log_on_execution(conn, ctx, LogLevel::Error, &message, &redactions).await?;
        return Ok(IterationEnd::Failed);
    }
    // **Spec D10 (Q4), brought forward from Task B4:** a FILE output names a
    // file of this execution's loop family, or the iteration fails.
    if let Err(message) = crate::storage::inputs::check_file_outputs(
        conn,
        ctx.workspace_id,
        ctx.id,
        &the_loop.outputs,
        &typed.values,
    )
    .await?
    {
        log_on_execution(conn, ctx, LogLevel::Error, &message, &redactions).await?;
        return Ok(IterationEnd::Failed);
    }
    // **STORE (package #6 Task A5, spec D5), before the per-iteration
    // cap:** the map goes to one Ion file under the iteration's own prefix
    // and the entry holds `{uri}`, so the 1 MiB cap, which guards the
    // `Loop` run's list, does not apply to it (the 16 MiB total still does,
    // at the append, to the `{uri}` entries).
    if store {
        return store_iteration_outputs(conn, ctx, &flow, typed, &redactions).await;
    }
    // FETCH: the map itself is the entry, held to the per-iteration cap.
    if let Err(message) = iteration_cap(index, &typed) {
        log_on_execution(conn, ctx, LogLevel::Error, &message, &redactions).await?;
        return Ok(IterationEnd::Failed);
    }
    Ok(IterationEnd::Typed(typed))
}

/// How an iteration's outputs reach the `Loop` run, `fetchType` resolved
/// (spec D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchMode {
    Fetch,
    Store,
}

/// **`fetchType`, `AUTO` resolved** (spec D5; Kestra's `guessOutputMode`):
/// `AUTO` is STORE when `values:` is a string that rendered to a URI
/// (`kestra://`, `file://`, `nsfile://`, Kestra's `URIFetcher.supports`),
/// FETCH otherwise. Decided from the `Loop` run's own `rendered` -- the
/// values rendered once, when the run started -- so once per `Loop` run,
/// not per iteration. A list `values:`, or a string that renders to a list,
/// never starts with a scheme (`rendered` holds the list's JSON); a string
/// that rendered to a URI is the only `rendered` that does, because the
/// worker stores it as [`rendered_from_file`](crate::executor::rendered_from_file)'s
/// `"<uri>\n<count>"`.
fn fetch_mode(fetch_type: crate::models::LoopFetchType, loop_rendered: Option<&str>) -> FetchMode {
    use crate::models::LoopFetchType as F;
    match fetch_type {
        F::Fetch => FetchMode::Fetch,
        F::Store => FetchMode::Store,
        F::Auto if loop_rendered.is_some_and(crate::executor::values_name_a_file) => {
            FetchMode::Store
        }
        F::Auto => FetchMode::Fetch,
    }
}

/// The refusal of a SECRET output under STORE at evaluation (spec D9, §2.2
/// item 8), naming the outputs and how STORE was reached, never a value.
fn secret_under_store_message(ids: &[&str], fetch_type: crate::models::LoopFetchType) -> String {
    let why = match fetch_type {
        crate::models::LoopFetchType::Auto => {
            "fetchType is AUTO and the Loop's values: rendered to a file, so its outputs are stored"
        }
        _ => "fetchType is STORE",
    };
    format!(
        "egret refuses output {}: a SECRET output cannot be written to internal storage ({why}): \
         the iteration's outputs file would hold the value in clear. Use fetchType: FETCH",
        ids.join(", ")
    )
}

/// **The STORE branch** (spec D5): write `typed` as the iteration's Ion file
/// ([`storage::loop_outputs_file`](crate::storage::loop_outputs_file)) and
/// answer the entry's map `{uri}`.
///
/// **The write is never inside the transaction** (the prefetch rule,
/// `storage::prefetch`, applied to a write): the attempt records it with
/// the owner's cache ([`Prefetch::upload`](crate::storage::prefetch::Prefetch)),
/// the owner rolls back, writes the object with nothing held, and runs the
/// attempt again, which indexes the file in its own transaction -- so the
/// index row and the `{uri}` entry commit together, and a later task's
/// `read()` resolves it as readable (spec D5, `resolve_readable_many`) and
/// the pruner keeps it with its loop family.
///
/// An author's problem (no internal storage, a namespace or flow id no URI
/// can carry, a file over `workflow.storage.max_file_bytes`) fails the
/// iteration with its message; a store outage answers `Err`, so nothing
/// commits and the settle runs again, as an outage on a read does.
async fn store_iteration_outputs(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    flow: &Flow,
    typed: crate::outputs::Typed,
    redactions: &[String],
) -> Result<IterationEnd> {
    use crate::storage::prefetch::Upload;
    let Some(fs) = crate::storage::current() else {
        let message = "Loop outputs with fetchType: STORE cannot be written: egret has no \
                       internal storage configured (the `[storage]` section of its \
                       configuration). Use fetchType: FETCH";
        log_on_execution(conn, ctx, LogLevel::Error, message, redactions).await?;
        return Ok(IterationEnd::Failed);
    };
    let (uri, bytes) = match crate::storage::loop_outputs_file(
        &flow.namespace,
        &flow.id,
        ctx.id,
        &typed.values,
        fs.caps.max_file_bytes,
    ) {
        Ok(file) => file,
        Err(reason) => {
            let message = format!("Loop outputs with fetchType: STORE cannot be written: {reason}");
            log_on_execution(conn, ctx, LogLevel::Error, &message, redactions).await?;
            return Ok(IterationEnd::Failed);
        }
    };
    let object = fs
        .object_uri(ctx.workspace_id, &uri)
        .map_err(|e| Error::Internal(anyhow::anyhow!("egret built {e}")))?;
    let gate = ctx.gate.as_ref().ok_or_else(|| {
        Error::Internal(anyhow::anyhow!(
            "execution {}: a reader's context reached a Loop iteration's STORE write",
            ctx.id
        ))
    })?;
    let attempted = bytes.len() as u64;
    let size = match gate.prefetch().upload(ctx.workspace_id, &object, bytes) {
        // The size of what was WRITTEN, the first attempt's bytes (final
        // review m7): a later attempt may compute other bytes (`now()`).
        Upload::Done(written) => written,
        // `Pending`: this attempt is rolled back whatever it answers; it
        // carries on as if written (the gather rule), and its index row and
        // entry go with the rollback.
        Upload::Pending => attempted,
        Upload::Outage(outage) => {
            return Err(Error::Internal(anyhow::anyhow!(
                "execution {}: {outage}",
                ctx.id
            )))
        }
        Upload::Final(message) => {
            log_on_execution(conn, ctx, LogLevel::Error, &message, redactions).await?;
            return Ok(IterationEnd::Failed);
        }
    };
    crate::storage::index_execution_files(
        conn,
        ctx.workspace_id,
        ctx.id,
        &[crate::storage::StoredFile {
            uri: uri.clone(),
            size,
        }],
    )
    .await?;
    let mut values = serde_json::Map::new();
    values.insert("uri".into(), serde_json::Value::String(uri));
    Ok(IterationEnd::Typed(crate::outputs::Typed {
        values,
        secret_ids: Vec::new(),
        warnings: Vec::new(),
    }))
}

/// The per-iteration cap of spec D4: `Err` with the message when `typed`'s
/// map serialises to more than
/// [`MAX_LOOP_ITERATION_OUTPUT_BYTES`](crate::runners::env::MAX_LOOP_ITERATION_OUTPUT_BYTES).
fn iteration_cap(index: i32, typed: &crate::outputs::Typed) -> std::result::Result<(), String> {
    use crate::runners::env::{MAX_LOOP_ITERATION_OUTPUT_BYTES, MAX_LOOP_OUTPUTS_BYTES};
    // `Map` serialises infallibly to a `Vec`; a length of 0 cannot pass a
    // cap that is positive, so an impossible error reads as "no bytes".
    let bytes = serde_json::to_vec(&typed.values).map_or(0, |v| v.len());
    if bytes <= MAX_LOOP_ITERATION_OUTPUT_BYTES {
        return Ok(());
    }
    Err(format!(
        "Loop outputs of iteration {index} are {bytes} bytes; egret keeps at most {} MiB per \
         iteration and {} MiB per Loop in the execution. Use fetchType: STORE to keep them in \
         internal storage.",
        MAX_LOOP_ITERATION_OUTPUT_BYTES >> 20,
        MAX_LOOP_OUTPUTS_BYTES >> 20,
    ))
}

/// One line about `ctx`'s execution itself (no task run), written to
/// `workflow.task_logs` on `conn` -- inside the settle's transaction, so a
/// settle that rolls back leaves no line -- masked with `redactions`
/// (`logs::mask`) and split into [`LOG_PIECE_CHARS`](crate::logs::LOG_PIECE_CHARS)
/// pieces as every stored line is. Also traced, as a handler's line is not:
/// this one has no `RunLog`.
pub(super) async fn log_on_execution(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    level: LogLevel,
    message: &str,
    redactions: &[String],
) -> Result<()> {
    log_line(
        conn,
        ctx.workspace_id,
        ctx.id,
        None,
        level,
        message,
        redactions,
    )
    .await
}

/// [`log_on_execution`], on one task run of `execution` when `task_run` is
/// set (the `Loop` run's own line, Task A4).
pub(super) async fn log_line(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
    task_run: Option<Uuid>,
    level: LogLevel,
    message: &str,
    redactions: &[String],
) -> Result<()> {
    let masked = crate::logs::mask(message, redactions);
    tracing::info!(execution = %execution, level = level.as_str(), message = %masked, "loop iteration outputs");
    let chars: Vec<char> = masked.chars().collect();
    let pieces: Vec<String> = chars
        .chunks(crate::logs::LOG_PIECE_CHARS)
        .map(|c| c.iter().collect())
        .collect();
    sqlx::query(
        "INSERT INTO workflow.task_logs \
                (workspace_id, execution_id, task_run_id, level, ts, message) \
         SELECT $1, $2, $6, $3, $5, m \
           FROM unnest($4::text[]) WITH ORDINALITY AS t(m, n) ORDER BY t.n",
    )
    .bind(workspace)
    .bind(execution)
    .bind(level.as_i16())
    .bind(&pieces)
    .bind(chrono::Utc::now())
    .bind(task_run)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// The loop's resolved iteration values, as
/// [`iteration_values`](crate::executor::iteration_values) produces them.
///
/// Read from the loop run's `rendered` — the caller's answer, which is where
/// `advance()` reads it too — so this list and the one `flowable::loop_::open`
/// took the first batch from are the same list.
fn values_of(ctx: &claim::ExecutionCtx, state: &ExecutionState, run: RunIx) -> Result<LoopValues> {
    let rendered = state.runs[run].rendered.as_deref().ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            ctx.id,
            format!("loop run {run} has spawned an iteration but has no rendered values"),
        ))
    })?;
    resolve_loop_values(rendered)
        .map_err(|e| worker_err(WorkerError::Corrupt(ctx.id, format!("loop run {run}: {e}"))))
}

/// Values `from..from + n` of a file `Loop` run, in index order, read from
/// `workflow.loop_file_values` (migration 0167) by their index: one indexed
/// range per spawn, never the whole file (W9a Task 11, fix round 1). Fewer
/// rows than asked for is corruption -- the run's `rendered` counted them.
async fn file_values(
    conn: &mut PgConnection,
    execution: Uuid,
    run: RunIx,
    from: usize,
    n: usize,
) -> Result<Vec<String>> {
    let values: Vec<String> = sqlx::query_scalar(
        "SELECT value FROM workflow.loop_file_values \
          WHERE execution_id = $1 AND run_ix = $2 AND idx >= $3 AND idx < $4 \
          ORDER BY idx",
    )
    .bind(execution)
    .bind(run as i32)
    .bind(from as i32)
    .bind((from + n) as i32)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    if values.len() != n {
        return Err(worker_err(WorkerError::Corrupt(
            execution,
            format!(
                "loop run {run}: values {from}..{} of its file hold {} rows",
                from + n,
                values.len()
            ),
        )));
    }
    Ok(values)
}

/// `Loop.transmitFailed`, from the execution's frozen source.
///
/// **Not from the `Plan`**, which does not carry it: `executor::TaskKind::Loop`
/// holds `values`, `tasks` and `concurrency_limit`, because those three are all
/// `advance()` needs and a plan that carried a field only a caller reads would
/// be a second copy of the source. W1's `models::Loop` has it
/// (`transmit_failed`, defaulting to `true`, `Loop.java:240`).
fn transmit_failed(ctx: &claim::ExecutionCtx, task: TaskIx) -> Result<bool> {
    let flow = crate::spec::parse(&ctx.source)
        .map_err(|e| worker_err(WorkerError::Unplannable(ctx.id, e.message)))?;
    let id = &ctx.plan.tasks[task].id;
    match find_task(&flow, id).map(|t| &t.kind) {
        Some(ModelTaskKind::Loop(l)) => Ok(l.transmit_failed),
        _ => Err(worker_err(WorkerError::Corrupt(
            ctx.id,
            format!("task {id:?} is not a Loop of the execution's own source"),
        ))),
    }
}

/// `State.Type.isTerminatedInError()` — `FAILED || KILLED || CANCELLED`
/// (`State.java:296`), **all three of which [`RunState`] has since W8 Task 6**.
///
/// It is spelled as Kestra's literal list rather than as
/// `is_terminal() && !is_terminated_no_fail()` so that a reader can diff it
/// against the Java line by line.
///
/// # The guard this doc claimed, which did not exist
///
/// This paragraph said `state::tests::no_fail_is_a_subset_of_terminal` *"asserts
/// the two spellings agree, which is what makes keeping both safe."* **It does
/// not, and never called this function.** It compares
/// `is_terminal() && !is_terminated_no_fail()` against a **literal set written
/// in its own body**, so it pins one spelling against a third copy and says
/// nothing about this one. Measured: **MX1** — drop `RunState::Cancelled` here,
/// leaving `Failed | Killed` — **survived 3 of 3 runs**, 1036 passed, a result
/// line byte-identical to the control.
///
/// **A wrong assertion is worse than none**, because it converts an unguarded
/// predicate into a defended one, and this package has already paid for that
/// class twice. [`tests::the_two_spellings_of_terminated_in_error_agree`] is
/// the guard now: it calls **this function** for every member of
/// `RunState::ALL` and compares it to the derived predicate, so a variant added
/// to one spelling and not the other fails rather than diverging quietly.
///
/// **Not shipped today**, and the bound is worth stating rather than leaving a
/// reader to infer: W8 Task 6's `sub_flow_source` clears `sla:` from a
/// synthesised loop body, so no iteration of this corpus can end `CANCELLED`
/// and `transmitFailed` has never been asked about one. The scenario is one
/// package away — give a `Loop` child a `CANCELLED` terminal and the unguarded
/// version reports **SUCCESS** for a cancelled iteration.
fn terminated_in_error(state: RunState) -> bool {
    matches!(
        state,
        RunState::Failed | RunState::Killed | RunState::Cancelled
    )
}

/// **The state the loop run reads for an iteration that ended `RETRIED`, and
/// the reason it is not `RETRIED`** — W6 Task 4, fix round 1.
///
/// A `CREATE_NEW_EXECUTION` retry inside a `Loop` body ends the *iteration*
/// execution `RETRIED` on egret's side. Kestra never reports that state to a
/// parent, and the measurement of what it does instead is on the pinned digest
/// (`rev4.probe/replay-in-loop`, a one-value `Loop` whose body's only task is a
/// `Fail` with `behavior: CREATE_NEW_EXECUTION`):
///
/// ```text
/// exec 4hsgGQXsbKwevjszb3RKKN  parentId=None   state=FAILED  run l     = FAILED
/// exec 1EM1lBJxgIfaFxh0dSBAVR  parentId=4hsg…  state=FAILED  run inner = FAILED
/// WARN … Flow failed from executor … 'Can't find taskrun with taskrunId
///   '3mXFFNkYdF9Xo5nc0VvOdy' on execution '1EM1lBJxgIfaFxh0dSBAVR''
/// ```
///
/// The id it cannot find is the **`Loop` run's**, which lives in the parent:
/// `markWithTaskRunAs(…, RETRIED, markParents = true)` walks the failing run's
/// parent chain and that chain leaves the sub-execution. The throw aborts the
/// retry marking, `handleFailedExecutionFromExecutor` fails **that execution**,
/// and everything after it is the ordinary loop path with a `FAILED` child —
/// which is why the iteration ends `FAILED` with its body run `FAILED`, and why
/// the root ends `FAILED` through nothing more exotic than `transmitFailed`.
///
/// So this is not a special case bolted onto [`decide`]: it is the one
/// substitution that puts the parent back on the path the artifact takes. It is
/// deliberately **not** `terminated_in_error(Retried)`, which would have sent
/// `decide` down `terminateLoop`'s `state == FAILED ? stateFailure : SUCCESS`
/// ternary and ended the loop `SUCCESS` again — the same wrong answer by a
/// different route.
///
/// # What it does NOT fix, and why that half is not one predicate
///
/// The iteration's own terminal stays `RETRIED` where Kestra's is `FAILED`, and
/// its body run stays `RETRIED` where Kestra's is `FAILED`. That half is
/// `advance()`'s, which is pure over `(plan, state, now)` and cannot see which
/// link kind carried the execution it is driving — it would need a third
/// metadata field, for a case no corpus flow reaches. This half needs nothing
/// new: the caller is [`on_execution_terminal`], which is holding the link.
///
/// The inversion this repairs was **SUCCESS where Kestra says FAILED** — a
/// silent over-acceptance the oracle cannot catch, because no corpus flow puts
/// a `CREATE_NEW_EXECUTION` retry inside a `Loop` (measured over all 351).
fn as_the_loop_reads_it(child: RunState) -> RunState {
    match child {
        RunState::Retried => RunState::Failed,
        other => other,
    }
}

/// The flow a sub-execution runs: the parent flow with the `Loop`'s body in
/// place of its own `tasks:`.
///
/// The sub-execution reports the **same** `flowId` and `namespace` as its
/// parent — measured, all 710 descendants in `traces.json` — so this keeps the
/// flow's identity, its `inputs:` and its `variables:` (the body renders
/// against them: `valids/finally-loop`'s body reads `{{ inputs.failed }}`) and
/// replaces only the task lists.
///
/// The `Loop`'s own `errors:` and `finally:` **replace** the parent flow's
/// (end hooks A6, D5), and that is measured rather than tidy:
/// `valids/all-flowable` ran its flow-level `errors:` in the PARENT when the
/// loop failed, and its sub-execution ran the `Loop`'s own `errors:` instead.
/// Only `afterExecution:` is **cleared** (a LOOP iteration has none), together
/// with `triggers:` and `sla:` below.
fn sub_flow_source(source: &str, loop_task_id: &str, owner: Uuid) -> Result<String> {
    let mut flow = crate::spec::parse(source)
        .map_err(|e| worker_err(WorkerError::Unplannable(owner, e.message)))?;
    let (body, errors, finally) = match find_task(&flow, loop_task_id).map(|t| &t.kind) {
        Some(ModelTaskKind::Loop(l)) => (l.tasks.clone(), l.errors.clone(), l.finally.clone()),
        _ => {
            return Err(worker_err(WorkerError::Corrupt(
                owner,
                format!("task {loop_task_id:?} is not a Loop of the execution's own source"),
            )))
        }
    };
    // Package #6 Task A6 (spec D7): the `retry:` the iteration's tasks
    // inherit, found BEFORE the tree is cut down to the body.
    flow.inherited_retry = retry_above_body(&flow, loop_task_id);
    flow.tasks = body;
    // End hooks A6 (D5): the iteration's `errors:`/`finally:` are the `Loop`'s
    // own, not the parent flow's (Kestra `ES:1037-1043`). `afterExecution`
    // stays cleared below: a LOOP iteration has none (`ExS:1331`).
    flow.errors = errors;
    flow.finally_ = finally;
    flow.after_execution = Vec::new();
    // Package #6 Task B1 (spec D6): the parent flow's `outputs:` are the
    // PARENT execution's, evaluated once at its own terminal
    // (`worker::flow_outputs`). Kestra's `onEnd` evaluates them in every
    // iteration too, against a context where the parent's later tasks do not
    // exist -- treated as a Kestra bug and recorded (spec §6 item 1). Cleared
    // rather than guarded, for the reason `triggers:` and `sla:` are below;
    // `flow_outputs` also skips an iteration, for an iteration stored before
    // this line.
    flow.outputs = Vec::new();
    // A sub-execution is spawned, never triggered. Carrying the parent's
    // `triggers:` into it would put a schedule on a row that is one iteration
    // of one run.
    flow.triggers = Vec::new();
    // **And the flow-level `sla:` is the parent execution's, not an
    // iteration's** -- W8 Task 6 fix round 1, and it closes a real
    // over-acceptance rather than tidying.
    //
    // Kestra runs an iteration as a sub-execution of the SAME flow and guards
    // the evaluation site instead: `handleExecutionChangedSLA` returns early on
    // `getKind() == ExecutionKind.LOOP`.
    //
    // **W8 Task 6 fix round 2 corrects the reason given here.** It read *"egret
    // has no `ExecutionKind`"*, and that is **false**:
    // [`crate::models::ExecutionKind`] exists, has a `Loop` variant, and
    // `trigger::flow` reads it for `ExecutionKind.isNormal`. The true statement
    // is narrower -- **`workflow.executions` has no `kind` column**, so an
    // execution egret creates does not carry one and `claim::settle` cannot ask
    // Kestra's question in Kestra's words.
    //
    // **And the narrower truth does not leave the reader without an
    // alternative, which the false one implied.** A loop iteration IS
    // identifiable there: its row has a non-NULL `loop_index`, which
    // `render::Surface::load` already SELECTs. So both sites were available and
    // this is a choice between them, not the only option. It is made here
    // because a field that is not carried cannot be read by the next consumer
    // either, which is the rule `triggers:` above is cleared under -- the same
    // reason `state::Resume` is a field rather than two `TaskKind` lists.
    //
    // Without this line a flow with an `EXECUTION_ASSERTION` and a `Loop` had
    // its SLA evaluated once per iteration by `claim::sla_terminal`, cancelling
    // iterations Kestra runs.
    //
    // **No corpus flow declares both**, so the ratchet is silent in both
    // directions and `no_flow_level_sla_is_inherited_by_a_loop_sub_execution`
    // is the whole instrument. It is a clear rather than a guard for the reason
    // `triggers:` above is: a field that is not there cannot be read by the
    // next consumer either.
    flow.sla = Vec::new();
    serde_norway::to_string(&flow).map_err(|e| {
        worker_err(WorkerError::Corrupt(
            owner,
            format!("the loop body of {loop_task_id:?} does not serialise: {e}"),
        ))
    })
}

/// The `retry:` a `Loop` iteration's tasks inherit (package #6 Task A6,
/// spec D7): the `Loop`'s own, else its nearest ancestor's in `flow`, else
/// what `flow` itself inherited -- `flow` is then an iteration of an outer
/// `Loop`, and Kestra's walk would go on through that `Loop` too.
///
/// Kestra's `searchForParentTaskWithRetry` (`ExecutorService.java:841-856`)
/// climbs `Flow.findParentTasksByTaskId`, the flowable whose DIRECT children
/// (`tasks`, `errors`, `finally`) hold the task, so the first `retry:` met on
/// the way up is the nearest one. A `CREATE_NEW_EXECUTION` policy found here is
/// carried like any other and refused at run by `worker::replay`, as Kestra
/// throws there (spec D7; A1 warns at save).
fn retry_above_body(flow: &Flow, loop_task_id: &str) -> Option<crate::models::Retry> {
    /// `Some(nearest)` when `want` is `task` or below it, with the nearest
    /// `retry:` on the way down to it (`want`'s own included).
    fn walk<'a>(
        task: &'a Task,
        want: &str,
        above: Option<&'a crate::models::Retry>,
    ) -> Option<Option<&'a crate::models::Retry>> {
        let nearest = task.retry.as_ref().or(above);
        if task.id == want {
            return Some(nearest);
        }
        task.children()
            .into_iter()
            .chain(task.error_children())
            .find_map(|c| walk(c, want, nearest))
    }
    let found = crate::spec::path::root_lists(flow)
        .into_iter()
        .flat_map(|(_, list)| list.iter())
        .find_map(|t| walk(t, loop_task_id, None))
        .flatten();
    found.cloned().or_else(|| flow.inherited_retry.clone())
}

/// One task by id, from anywhere in the flow.
///
/// Task ids are unique across a whole flow (`spec::validate`'s
/// `duplicate_task_id` is a structural check over the nested walk), so the id
/// is a key and not a guess — the same rule `claim::leaf_spec` relies on.
fn find_task<'a>(flow: &'a Flow, want: &str) -> Option<&'a Task> {
    // End hooks A4 fix round (2026-10-27): the shared walk. This one walked
    // `children()` only, so a `Loop` inside any `errors:` list below the root
    // was not found and the tick aborted ("is not a Loop of the execution's
    // own source").
    crate::spec::path::find_task(flow, want)
}

/// The parent execution and the **loop run** inside it, or `None` when there
/// is no loop run to report back to.
///
/// **The `zip` is doing real work since migration 0138, and this doc used to
/// say the opposite.** Under 0135's
/// `workflow_executions_loop_run_all_or_nothing` the four loop columns were all
/// set or all null, so a row naming a parent always named a run and an index
/// too and the pair could never be half-present. 0138 keeps that for a loop
/// iteration -- `workflow_executions_loop_run_whole_or_absent` still forbids a
/// parent *and* a run *without* an index -- but deliberately admits one more
/// shape: **a parent with no loop columns at all**, which is Kestra's
/// `CREATE_NEW_EXECUTION` replay (W6 Task 4). For that row `parent_run_ix` is
/// NULL and this function returns `None`, which is the answer it should give:
/// a replay is a new execution of the same flow, not an iteration a `Loop` run
/// is waiting on, and [`on_execution_terminal`] has nothing to report to.
///
/// A `Subflow` child never reaches here at all -- it is linked by
/// `trigger_execution_id`, and `workflow_executions_one_link_kind` makes a row
/// carrying both links unrepresentable.
async fn parent_of(conn: &mut PgConnection, execution_id: Uuid) -> Result<Option<(Uuid, i32)>> {
    let row = sqlx::query(
        "SELECT parent_execution_id, parent_run_ix FROM workflow.executions WHERE id = $1",
    )
    .bind(execution_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("execution {execution_id}")))?;
    Ok(row
        .get::<Option<Uuid>, _>("parent_execution_id")
        .zip(row.get::<Option<i32>, _>("parent_run_ix")))
}

/// How many iterations of this loop run have been started.
async fn spawned_count(conn: &mut PgConnection, parent: Uuid, run: RunIx) -> Result<usize> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix = $2",
    )
    .bind(parent)
    .bind(run as i32)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(n as usize)
}

/// How many of them have terminated.
///
/// `state` is the execution's own last history entry, written by
/// `claim::persist` in the same transaction that terminated it, so this count
/// already includes the sub-execution whose terminal is being reported.
async fn terminated_count(conn: &mut PgConnection, parent: Uuid, run: RunIx) -> Result<usize> {
    let terminal: Vec<&str> = RunState::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| s.wire())
        .collect();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix = $2 AND state = ANY($3)",
    )
    .bind(parent)
    .bind(run as i32)
    .bind(&terminal)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(n as usize)
}

/// Keeps the `sqlx::Error` as the source, as `claim`'s does, so a caller can
/// read its SQLSTATE (`worker::kill` retries a deadlock).
fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

fn worker_err(e: WorkerError) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed_with(value: serde_json::Value) -> crate::outputs::Typed {
        let mut typed = crate::outputs::Typed::default();
        typed.values.insert("big".into(), value);
        typed
    }

    /// Package #6 A3 (spec D4, Q5): a map over 1 MiB fails its iteration with
    /// the message that names the iteration, its size and `fetchType: STORE`;
    /// a map exactly at the cap passes.
    /// **`fetchType`, `AUTO` resolved (Task A5, spec D5; Kestra's
    /// `guessOutputMode`)**: from the `Loop` run's `rendered`, which starts
    /// with a scheme only when a string `values:` rendered to a URI.
    #[test]
    fn auto_is_store_only_when_the_values_rendered_to_a_uri() {
        use crate::models::LoopFetchType as F;
        let file = crate::executor::rendered_from_file("kestra:///a/b/executions/x/c.ion", 3);
        for (fetch_type, rendered, want) in [
            (F::Auto, Some(file.as_str()), FetchMode::Store),
            (F::Auto, Some("nsfile:///a.txt\n1"), FetchMode::Store),
            (F::Auto, Some(r#"["a","b"]"#), FetchMode::Fetch),
            (F::Auto, Some(r#"{"k":"v"}"#), FetchMode::Fetch),
            (F::Auto, None, FetchMode::Fetch),
            (F::Fetch, Some(file.as_str()), FetchMode::Fetch),
            (F::Store, Some(r#"["a"]"#), FetchMode::Store),
        ] {
            assert_eq!(
                fetch_mode(fetch_type, rendered),
                want,
                "{fetch_type:?} {rendered:?}"
            );
        }
    }

    #[test]
    fn an_iteration_map_over_one_mib_fails_naming_fetch_type_store() {
        use crate::runners::env::MAX_LOOP_ITERATION_OUTPUT_BYTES as CAP;
        // `{"big":"<s>"}` is `s.len() + 10` bytes.
        let at_cap = typed_with(serde_json::Value::String("x".repeat(CAP - 10)));
        assert_eq!(
            serde_json::to_vec(&at_cap.values).unwrap().len(),
            CAP,
            "fixture"
        );
        assert_eq!(iteration_cap(2, &at_cap), Ok(()));

        let over = typed_with(serde_json::Value::String("x".repeat(CAP - 9)));
        assert_eq!(
            iteration_cap(2, &over),
            Err(format!(
                "Loop outputs of iteration 2 are {} bytes; egret keeps at most 1 MiB per \
                 iteration and 16 MiB per Loop in the execution. Use fetchType: STORE to keep \
                 them in internal storage.",
                CAP + 1
            ))
        );
    }

    /// Package #6 Task A6 (spec D7): the `retry:` an iteration inherits is
    /// the `Loop`'s own, else its nearest ancestor's in the parent flow, else
    /// what an enclosing iteration inherited; the flow's own `retry:` stays
    /// where it was and is not copied into the inherited one.
    #[test]
    fn an_iteration_inherits_the_nearest_retry_above_its_body() {
        let retry = |n: u32| format!("retry: {{type: constant, interval: PT1S, maxAttempts: {n}}}");
        let max = |r: &Option<crate::models::Retry>| match r {
            Some(crate::models::Retry::Constant { common, .. }) => common.max_attempts,
            Some(_) => panic!("not a constant retry: {r:?}"),
            None => None,
        };
        let body = "tasks: [{id: c, type: core.log, message: m}]";
        let iteration = |src: &str, id: &str| {
            crate::spec::parse(&sub_flow_source(src, id, uuid::Uuid::nil()).unwrap()).unwrap()
        };

        // The `Loop`'s own, over an ancestor's and the flow's.
        let src = format!(
            "id: f\nnamespace: a.b\n{}\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    {}\n    tasks:\n      - id: l\n        type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        {}\n        {body}\n",
            retry(5),
            retry(4),
            retry(3)
        );
        let it = iteration(&src, "l");
        assert_eq!(max(&it.inherited_retry), Some(3));
        assert_eq!(max(&it.retry), Some(5), "the flow's own stays the flow's");

        // No `retry:` on the `Loop`: the `Sequential`'s, over the flow's.
        let src = format!(
            "id: f\nnamespace: a.b\n{}\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    {}\n    tasks:\n      - id: l\n        type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        {body}\n",
            retry(5),
            retry(4)
        );
        assert_eq!(max(&iteration(&src, "l").inherited_retry), Some(4));

        // In an `errors:` list too: the flowable that holds it is the parent.
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    {}\n    tasks: [{{id: x, type: core.log, message: m}}]\n    errors:\n      - id: l\n        type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        {body}\n",
            retry(2)
        );
        assert_eq!(max(&iteration(&src, "l").inherited_retry), Some(2));

        // Nothing above the body: nothing inherited, the flow's still there.
        let src = format!(
            "id: f\nnamespace: a.b\n{}\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    {body}\n",
            retry(5)
        );
        let it = iteration(&src, "l");
        assert_eq!(it.inherited_retry, None);
        assert_eq!(max(&it.retry), Some(5));

        // A nested `Loop` with nothing between it and the outer `Loop`
        // inherits what the outer iteration inherited; one with a `retry:`
        // of its own does not.
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    {}\n    tasks:\n      - id: plain\n        type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        {body}\n      - id: own\n        type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        {}\n        {body}\n",
            retry(3),
            retry(2)
        );
        let outer = sub_flow_source(&src, "outer", uuid::Uuid::nil()).unwrap();
        assert_eq!(max(&iteration(&outer, "plain").inherited_retry), Some(3));
        assert_eq!(max(&iteration(&outer, "own").inherited_retry), Some(2));
    }

    /// Package #6 Task A6: the inherited `retry:` is not a key a flow may
    /// write. No Kestra class has it, so saving a flow that writes it is
    /// refused (`unknown_property`), while the run path, which parses an
    /// iteration's stored source with no save check, reads it.
    #[test]
    fn the_inherited_retry_key_is_refused_at_save_and_read_at_run() {
        let src = "id: f\nnamespace: a.b\nx-egret-inherited-retry: {type: constant, interval: PT1S, maxAttempts: 3}\ntasks: [{id: c, type: core.log, message: m}]\n";
        let flow = crate::spec::parse(src).unwrap();
        assert!(flow.inherited_retry.is_some(), "the run path reads it");
        let report = crate::spec::validate_source(src, &flow);
        assert!(
            report
                .errors
                .iter()
                .any(|f| f.code == "unknown_property" && f.path.contains("x-egret-inherited-retry")),
            "{report:?}"
        );
    }

    /// End hooks A7 (carry c): a `Loop` nested in the body, in `errors:` and
    /// in `finally:` of the Loop stays where it is, and the iteration source
    /// takes the lists of the Loop it was asked for.
    #[test]
    fn a_nested_loop_keeps_its_own_lists_in_the_iteration_source() {
        let inner = |id: &str| {
            format!("type: io.kestra.plugin.core.flow.Loop\n        values: [1]\n        tasks: [{{id: {id}t, type: core.log, message: m}}]\n        errors: [{{id: {id}e, type: core.log, message: m}}]\n        finally: [{{id: {id}f, type: core.log, message: m}}]\n")
        };
        let src = format!(
            "id: f\nnamespace: a.b\nerrors: [{{id: fe, type: core.log, message: m}}]\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks:\n      - id: inb\n        {}    errors:\n      - id: ine\n        {}    finally:\n      - id: inf\n        {}",
            inner("b"),
            inner("e"),
            inner("g"),
        );
        let ids = |l: &[crate::models::Task]| l.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        let outer = crate::spec::parse(&sub_flow_source(&src, "outer", uuid::Uuid::nil()).unwrap())
            .unwrap();
        assert_eq!(ids(&outer.tasks), ["inb"]);
        assert_eq!(ids(&outer.errors), ["ine"]);
        assert_eq!(ids(&outer.finally_), ["inf"]);
        let inb =
            crate::spec::parse(&sub_flow_source(&src, "inb", uuid::Uuid::nil()).unwrap()).unwrap();
        assert_eq!(
            (ids(&inb.tasks), ids(&inb.errors), ids(&inb.finally_)),
            (vec!["bt".into()], vec!["be".into()], vec!["bf".into()])
        );
        let ine =
            crate::spec::parse(&sub_flow_source(&src, "ine", uuid::Uuid::nil()).unwrap()).unwrap();
        assert_eq!(
            (ids(&ine.tasks), ids(&ine.errors), ids(&ine.finally_)),
            (vec!["et".into()], vec!["ee".into()], vec!["ef".into()])
        );
        let inf =
            crate::spec::parse(&sub_flow_source(&src, "inf", uuid::Uuid::nil()).unwrap()).unwrap();
        assert_eq!(
            (ids(&inf.tasks), ids(&inf.errors), ids(&inf.finally_)),
            (vec!["gt".into()], vec!["ge".into()], vec!["gf".into()])
        );
    }

    /// End hooks A6 (D5): an iteration runs the `Loop`'s own `errors:` and
    /// `finally:`, never the parent flow's, and has no `afterExecution`.
    #[test]
    fn an_iteration_runs_the_loops_errors_and_finally_as_its_own() {
        let src = "id: f\nnamespace: a.b\nerrors: [{id: fe, type: core.log, message: m}]\nfinally: [{id: ff, type: core.log, message: m}]\nafterExecution: [{id: fa, type: core.log, message: m}]\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{id: a, type: core.log, message: m}]\n    errors: [{id: e, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n";
        let sub =
            crate::spec::parse(&sub_flow_source(src, "each", uuid::Uuid::nil()).unwrap()).unwrap();
        let ids = |l: &[crate::models::Task]| l.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&sub.tasks), ["a"]);
        assert_eq!(
            ids(&sub.errors),
            ["e"],
            "the Loop's errors, not the flow's (ES:1037-1043)"
        );
        assert_eq!(ids(&sub.finally_), ["z"]);
        assert!(
            sub.after_execution.is_empty(),
            "a LOOP iteration has no afterExecution (ExS:1331)"
        );
    }

    /// **The guard [`terminated_in_error`]'s doc claimed and did not have.**
    ///
    /// Two spellings of one Java predicate is one chance for them to disagree,
    /// and the assertion that was cited for it —
    /// `state::tests::no_fail_is_a_subset_of_terminal` — **never calls this
    /// function**. It compares `is_terminal() && !is_terminated_no_fail()`
    /// against a literal set written in its own body, which pins one spelling
    /// to a third copy of the list and leaves this one free. **MX1** (drop
    /// `RunState::Cancelled`, leaving `Failed | Killed`) survived that suite
    /// **3 of 3 runs** with a result line byte-identical to the control.
    ///
    /// This calls `terminated_in_error` itself, over **every** member of
    /// `RunState::ALL` rather than over the three that happen to be true today,
    /// so the failure it reports is *which variant* disagrees and in *which
    /// direction*. A variant added to one spelling and not the other fails
    /// here; so does MX1.
    ///
    /// **This test postdates the defect it proves and cannot be evidence that
    /// the defect was ever caught** — it is written because MX1 showed nothing
    /// caught it. The honest claim is the narrow one: MX1 died 3/3 *after* this
    /// existed and survived 3/3 before.
    #[test]
    fn the_two_spellings_of_terminated_in_error_agree() {
        let mut disagreeing: Vec<String> = Vec::new();
        for s in RunState::ALL {
            let literal = terminated_in_error(*s);
            let derived = s.is_terminal() && !s.is_terminated_no_fail();
            if literal != derived {
                disagreeing.push(format!(
                    "{}: terminated_in_error={literal}, \
                     is_terminal() && !is_terminated_no_fail()={derived}",
                    s.wire()
                ));
            }
        }
        assert!(
            disagreeing.is_empty(),
            "`State.Type.isTerminatedInError()` is `FAILED || KILLED || \
             CANCELLED` (`State.java:296`) and this crate holds it twice -- \
             literally in `worker::spawn::terminated_in_error`, derivably in \
             `RunState`. They disagree on:\n  {}",
            disagreeing.join("\n  ")
        );

        // **The positive control.** Every row above is an equality, and an
        // implementation answering `false` to both sides would satisfy all
        // thirteen. Naming the three that must be TRUE is what stops that, and
        // it is the same list `no_fail_is_a_subset_of_terminal` checks from the
        // other side -- the difference being that this one reached it through
        // the function.
        let in_error: std::collections::BTreeSet<&str> = RunState::ALL
            .iter()
            .filter(|s| terminated_in_error(**s))
            .map(|s| s.wire())
            .collect();
        assert_eq!(
            in_error,
            std::collections::BTreeSet::from(["CANCELLED", "FAILED", "KILLED"]),
            "and the three, by name, read OUT of the function rather than \
             compared to a literal beside it"
        );
    }

    fn vals(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The index of a spawn is its position in the loop's values, counting from
    /// the iterations already started.
    #[test]
    fn a_spawns_index_is_its_position_in_the_values() {
        let values = vals(&["a", "b", "c"]);
        assert_eq!(
            iteration_indices(&values, 0, &["a", "b"]),
            Ok(vec![0, 1]),
            "the first batch is indexed from 0"
        );
        assert_eq!(
            iteration_indices(&values, 2, &["c"]),
            Ok(vec![2]),
            "an iteration started after two others is index 2, not index 0 -- \
             `loopRun.index` is the flow's numbering, not the batch's"
        );
        assert_eq!(
            iteration_indices(&values, 3, &[]),
            Ok(vec![]),
            "nothing left to start and nothing asked for"
        );
    }

    /// The guard: a requested value that is not the value at the index it
    /// would be given is corruption, not a case.
    #[test]
    fn a_spawn_whose_value_is_not_the_value_at_its_index_is_refused() {
        let values = vals(&["a", "b", "c"]);
        assert_eq!(
            iteration_indices(&values, 0, &["b"]),
            Err(
                "spawn 0 carries value \"b\", but iteration 0 of its values is \"a\"; \
                 a spawn's index is its position in the loop's values and the two \
                 must agree"
                    .to_string()
            ),
            "the batch must be a PREFIX of the values not yet started"
        );
        assert_eq!(
            iteration_indices(&values, 3, &["a"]),
            Err("asked to spawn iteration 3 but its values hold only 3".to_string()),
            "and it must not run off the end"
        );
    }

    /// Every branch of the continuation, with the corpus entry that measures
    /// it on the artifact named beside it.
    #[test]
    fn the_continuation_matches_the_three_shapes_the_corpus_measures() {
        // `valids/loop-failed`: limit 1, three values, a body that always
        // fails -- the artifact ran ONE sub-execution and the loop FAILED.
        assert_eq!(
            decide(true, RunState::Failed, 1, 1, 3),
            Next::Terminate(RunState::Failed),
            "`transmitFailed` ends the loop on the first failure, with two \
             values never started"
        );
        // `valids/loop-failed-no-transmit`: the same flow with the flag off --
        // three FAILED sub-executions under a SUCCESSFUL loop run.
        assert_eq!(
            decide(false, RunState::Failed, 1, 1, 3),
            Next::Start(1),
            "with the flag off a failure starts the next iteration like any other"
        );
        assert_eq!(
            decide(false, RunState::Failed, 3, 3, 3),
            Next::Terminate(RunState::Success),
            "and the loop SUCCEEDS once every iteration has terminated, whatever \
             they terminated as"
        );
        // `valids/loop-parallel-less`: limit 2 over four values. `advance()`
        // emitted two; the other two are started as the first two finish.
        assert_eq!(decide(true, RunState::Success, 2, 1, 4), Next::Start(2));
        assert_eq!(decide(true, RunState::Success, 3, 2, 4), Next::Start(3));
        assert_eq!(
            decide(true, RunState::Success, 4, 3, 4),
            Next::Wait,
            "all four started, three terminated: the loop waits for the fourth"
        );
        assert_eq!(
            decide(true, RunState::Success, 4, 4, 4),
            Next::Terminate(RunState::Success)
        );
    }

    /// `isTerminatedInError()` is `FAILED || KILLED || CANCELLED`
    /// (`State.java:296`) and `RunState` has the first two. **Neither of these
    /// two rows is measured on the artifact** -- no corpus flow kills a loop
    /// iteration -- and the KILLED row reproduces what reads like a Kestra
    /// defect (`terminateLoop`'s `state == FAILED ? … : SUCCESS`), which is
    /// asserted here so that it is a decision on the record rather than an
    /// accident.
    #[test]
    fn a_killed_iteration_ends_a_transmit_failed_loop_with_success() {
        assert_eq!(
            decide(true, RunState::Killed, 1, 1, 3),
            Next::Terminate(RunState::Success)
        );
        assert!(!terminated_in_error(RunState::Success));
        assert!(!terminated_in_error(RunState::Skipped));
        assert!(terminated_in_error(RunState::Failed));
        assert!(terminated_in_error(RunState::Killed));
    }

    /// **A `RETRIED` iteration fails its loop, and every other terminal is
    /// passed through untouched** — W6 Task 4, fix round 1.
    ///
    /// Two-sided by construction: the `RETRIED` row is the substitution and the
    /// loop over [`RunState::ALL`] is the control, so a
    /// `as_the_loop_reads_it` that answered `FAILED` for everything fails here
    /// rather than passing on the one row that matters.
    ///
    /// The third assertion is the one that says WHY the obvious one-predicate
    /// fix is not this one: adding `RETRIED` to [`terminated_in_error`] sends
    /// `decide` into `terminateLoop`'s ternary, whose non-`FAILED` arm is
    /// `SUCCESS` — the same inversion by a different route.
    #[test]
    fn a_retried_iteration_is_read_by_its_loop_as_a_failure() {
        assert_eq!(
            as_the_loop_reads_it(RunState::Retried),
            RunState::Failed,
            "Kestra's executor fails the iteration execution outright rather \
             than reporting RETRIED to the loop -- measured on the pinned \
             digest, see this function's doc"
        );
        for state in RunState::ALL.iter().copied() {
            if state == RunState::Retried {
                continue;
            }
            assert_eq!(
                as_the_loop_reads_it(state),
                state,
                "{state:?} must reach the loop unchanged; only RETRIED is a \
                 state the artifact never reports to a parent"
            );
        }
        assert_eq!(
            decide(true, as_the_loop_reads_it(RunState::Retried), 1, 1, 1),
            Next::Terminate(RunState::Failed),
            "so the one-value loop whose body was retried into a new execution \
             ends FAILED, and its root with it -- egret reported SUCCESS here \
             until this substitution"
        );
        assert_eq!(
            decide(true, RunState::Retried, 1, 1, 1),
            Next::Terminate(RunState::Success),
            "the inversion itself, at the fixture's own shape: a raw RETRIED \
             falls through to `terminated == total` and the loop SUCCEEDS. \
             **This line does NOT discriminate** -- at (1,1,1) the rejected \
             alternative answers SUCCESS too, by the ternary instead of by the \
             fall-through -- and it used to claim it did. The line below is the \
             one that chooses"
        );
        // **The control, at the input where the three implementations differ.**
        // (1,1,1) cannot tell them apart because two of the three answer
        // SUCCESS by different routes; a three-value loop can, because the
        // fall-through goes somewhere else entirely.
        assert_eq!(
            decide(true, RunState::Retried, 1, 1, 3),
            Next::Start(1),
            "as shipped, RETRIED is not `terminated_in_error`, so one \
             terminated iteration of three starts the next one. **This is what \
             excludes putting RETRIED into `terminated_in_error`**: that \
             alternative answers `Terminate(SUCCESS)` here, through \
             `terminateLoop`'s `state == FAILED ? stateFailure : SUCCESS` \
             ternary -- which is the SAME wrong answer as the fall-through and \
             is why the fix is a substitution rather than a predicate. The \
             ternary's own arm is pinned next door by \
             `a_killed_iteration_ends_a_transmit_failed_loop_with_success`, at \
             this same shape"
        );
        assert_eq!(
            decide(true, as_the_loop_reads_it(RunState::Retried), 1, 1, 3),
            Next::Terminate(RunState::Failed),
            "and the substitution answers FAILED at that shape too -- ending a \
             `transmitFailed` loop on the first failure with two values never \
             started, which is `valids/loop-failed`'s measured shape"
        );
    }

    /// **A `Loop` iteration does not inherit the parent's flow-level `sla:`**
    /// — W8 Task 6 fix round 1, and the instrument for a defect the ratchet
    /// cannot see.
    ///
    /// Kestra's `handleExecutionChangedSLA` returns early on
    /// `getKind() == ExecutionKind.LOOP`, because there an iteration IS a
    /// sub-execution of the same flow. [`sub_flow_source`] synthesises a
    /// different flow instead, and it copied everything it did not explicitly
    /// clear — so before this fix the parent's `EXECUTION_ASSERTION` was
    /// evaluated once per iteration by `claim::sla_terminal`, cancelling
    /// iterations the artifact runs.
    ///
    /// **No corpus flow declares both an `sla:` and a `Loop`**, which is why
    /// this is a hand-built fixture rather than a corpus derivation, and why
    /// the ratchet is silent in both directions. The control is `tasks:`: the
    /// body still arrives, so "the field is gone" is not "the synthesis broke".
    #[test]
    fn no_flow_level_sla_is_inherited_by_a_loop_sub_execution() {
        let source = "id: f\nnamespace: a.b\nsla:\n  - id: a\n    \
                      type: EXECUTION_ASSERTION\n    behavior: CANCEL\n    \
                      assert: \"{{ false }}\"\ntasks:\n  - id: lp\n    \
                      type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    \
                      tasks:\n      - id: body\n        \
                      type: io.kestra.plugin.core.log.Log\n        message: x\n";
        let parent = crate::spec::parse(source).expect("the fixture parses");
        assert_eq!(
            parent.sla.len(),
            1,
            "the fixture must carry an `sla:` -- otherwise the assertion below \
             passes on a flow that never had one"
        );

        let sub = sub_flow_source(source, "lp", Uuid::nil()).expect("the body synthesises");
        let reparsed = crate::spec::parse(&sub).expect("the sub-flow re-parses");
        assert!(
            reparsed.sla.is_empty(),
            "a `Loop` iteration is not the execution the flow-level `sla:` is \
             about; carrying it evaluates the SLA once per iteration. Got {:?}",
            reparsed.sla
        );
        assert_eq!(
            reparsed.tasks.len(),
            1,
            "and the body still arrives -- the control that separates \
             `sla:` being cleared from the synthesis being broken"
        );
        assert_eq!(reparsed.tasks[0].id, "body");
    }

    /// Package #6 Task B1 (spec D6): the parent flow's `outputs:` are not
    /// carried into an iteration's flow, so no iteration evaluates them.
    #[test]
    fn no_flow_level_outputs_are_inherited_by_a_loop_sub_execution() {
        let source = "id: f\nnamespace: a.b\noutputs:\n  - id: o\n    type: STRING\n    \
                      value: x\ntasks:\n  - id: lp\n    \
                      type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    \
                      tasks:\n      - id: body\n        \
                      type: io.kestra.plugin.core.log.Log\n        message: x\n";
        let parent = crate::spec::parse(source).expect("the fixture parses");
        assert_eq!(parent.outputs.len(), 1, "the fixture carries `outputs:`");

        let sub = sub_flow_source(source, "lp", Uuid::nil()).expect("the body synthesises");
        let reparsed = crate::spec::parse(&sub).expect("the sub-flow re-parses");
        assert!(reparsed.outputs.is_empty(), "{:?}", reparsed.outputs);
        assert_eq!(reparsed.tasks[0].id, "body", "and the body still arrives");
    }

    /// Every corpus flow that holds a `Loop` — the sub-flow this module
    /// synthesises must re-parse, and re-parse to the same `Flow`.
    ///
    /// A round-trip through `serde_norway` is the one part of this module that
    /// could break silently: a body that serialised to something the parser
    /// reads differently would give the sub-execution a *different flow* from
    /// the loop's body, and the sub-execution would still run.
    #[test]
    fn a_synthesised_sub_flow_reparses_to_the_body_it_was_made_from() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/corpus");
        let mut checked = 0usize;
        for half in ["valids", "invalids"] {
            let Ok(entries) = std::fs::read_dir(format!("{dir}/{half}")) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(flow) = crate::spec::parse(&source) else {
                    continue;
                };
                let loops: Vec<(String, Vec<Task>)> = loop_tasks(&flow);
                for (id, body) in loops {
                    let sub = sub_flow_source(&source, &id, Uuid::nil())
                        .unwrap_or_else(|e| panic!("{}: {id}: {e}", path.display()));
                    let reparsed = crate::spec::parse(&sub)
                        .unwrap_or_else(|e| panic!("{}: {id}: {}", path.display(), e.message));
                    assert_eq!(
                        reparsed.tasks,
                        body,
                        "{}: the sub-flow for {id} does not re-parse to the loop's body",
                        path.display()
                    );
                    assert_eq!(
                        (reparsed.id.as_str(), reparsed.namespace.as_str()),
                        (flow.id.as_str(), flow.namespace.as_str()),
                        "{}: a sub-execution reports its parent's flow id and namespace \
                         -- measured on all 710 descendants in traces.json",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(
            checked, 52,
            "every `Loop` the corpus declares, over 43 of its 351 files. Derived \
             independently by `grep -c 'type: io.kestra.plugin.core.flow.Loop'` \
             over the same directory, which finds the same 52 -- so this walk \
             loses none of them to a nesting the parser reaches and it does not."
        );
    }

    fn loop_tasks(flow: &Flow) -> Vec<(String, Vec<Task>)> {
        fn walk(t: &Task, out: &mut Vec<(String, Vec<Task>)>) {
            if let ModelTaskKind::Loop(l) = &t.kind {
                out.push((t.id.clone(), l.tasks.clone()));
            }
            for c in t.children() {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        for t in flow.tasks.iter().chain(&flow.errors).chain(&flow.finally_) {
            walk(t, &mut out);
        }
        out
    }
}
