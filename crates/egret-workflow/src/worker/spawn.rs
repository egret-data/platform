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
    apply, build_plan, resolve_loop_values, Command, ExecutionState, LoopValues, RunIx, RunState,
    TaskIx, TaskKind, Tick,
};
use crate::models::{Flow, Task, TaskKind as ModelTaskKind};

/// One [`Command::SpawnExecution`](crate::executor::Command::SpawnExecution),
/// kept as data.
///
/// `advance()`'s command carries no index, and it could not: it is emitted
/// once, in `values` order, by `flowable::loop_::open`. The index is derived
/// here from the loop's own resolved values — see [`spawn_all`].
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

/// Execute every spawn one round of `advance()` asked for.
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
/// # A sync fn returning a boxed future
///
/// `settle` awaits this, and `settle` re-enters itself through a `Subflow`
/// chain: awaited inline, this future's size is part of every level's poll
/// frame, and W9a Task 11 fix round 1's file-values arm grew it past what the
/// worker suite's `a_chain_of_subflows_deeper_than_the_bound…` fits in its
/// stack (measured: it overflowed). Boxed here, `settle` holds a pointer.
pub(super) fn spawn_all<'a>(
    conn: &'a mut PgConnection,
    ctx: &'a claim::ExecutionCtx,
    state: &'a ExecutionState,
    requested: &'a [Requested],
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Uuid>>> + Send + 'a>> {
    Box::pin(spawn_all_now(conn, ctx, state, requested))
}

async fn spawn_all_now(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    state: &ExecutionState,
    requested: &[Requested],
) -> Result<Vec<Uuid>> {
    let mut out = Vec::with_capacity(requested.len());
    // Grouped by the loop run, because the index is per loop run and two
    // `Loop`s can be in flight in one execution.
    let mut runs: Vec<RunIx> = Vec::new();
    for r in requested {
        if !runs.contains(&r.parent_run) {
            runs.push(r.parent_run);
        }
    }
    for parent_run in runs {
        let batch: Vec<&Requested> = requested
            .iter()
            .filter(|r| r.parent_run == parent_run)
            .collect();
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
        for (r, (index, value)) in batch.iter().zip(picked) {
            match spawn_one(conn, ctx, parent_run, r.task, index, &value).await? {
                Some(id) => out.push(id),
                // The loop run was failed (a copied FILE input no longer
                // readable): no further iteration of it is started.
                None => break,
            }
        }
    }
    Ok(out)
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

/// Create one sub-execution and drive it until it is waiting on a worker.
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
) -> Result<Option<Uuid>> {
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
    let plan = build_plan(&source)
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
    // In a savepoint, for `subflow::spawn_one`'s reason: the FILE-input check
    // below needs the row and may refuse it.
    let mut sp = sqlx::Connection::begin(&mut *conn).await.map_err(db_err)?;
    let (id, inputs): (Uuid, serde_json::Value) = sqlx::query_as(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            parent_execution_id, parent_run_ix, loop_index, loop_value, inputs, \
            execution_depth) \
         VALUES ($1,$2,$3,$4,$5,'CREATED','[\"CREATED\"]'::jsonb,$6,$7,$8,$9, \
                 COALESCE((SELECT inputs FROM workflow.executions WHERE id = $6), '{}'::jsonb), \
                 (SELECT execution_depth FROM workflow.executions WHERE id = $6)) \
         RETURNING id, inputs",
    )
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
        Box::pin(super::subflow::complete(
            conn,
            parent.id,
            parent_run,
            &[RunState::Failed],
            None,
            Some(&format!("loop iteration {index}: {why}")),
            claim::Stack::Nested(parent.caller()),
        ))
        .await?;
        return Ok(None);
    }
    sp.commit().await.map_err(db_err)?;

    let ctx = claim::ExecutionCtx {
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
        prefetch: parent.prefetch.clone(),
    };
    let empty = ExecutionState::new(&ctx.plan);
    let mut state = empty.clone();
    // `Tick(0)`: a `Tick` is milliseconds since the EXECUTION was created and
    // this is that instant, for the sub-execution's own clock. A sub-execution
    // does not inherit its parent's elapsed time — `executions.created_at` is
    // the origin and this row's is `now()`.
    // Boxed: a loop body may itself hold a `Loop` (`valids/loop-nested`), so
    // `settle` -> `spawn_all` -> here -> `settle` is a real cycle in the type
    // graph. It is not a cycle in the data — a sub-flow is strictly smaller
    // than the flow it came from, since it is one `Loop`'s `tasks:` list.
    Box::pin(claim::settle(conn, &ctx, &empty, &mut state, Tick(0), None)).await?;
    Ok(Some(id))
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
pub(super) async fn on_execution_terminal(
    conn: &mut PgConnection,
    execution_id: Uuid,
    state: RunState,
    caller: claim::Caller,
) -> Result<()> {
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
    // caps at `plan::MAX_TASK_DEPTH` = 48. Measured: 12 nested one-value
    // `Loop`s run at `ctx.depth = 12` with no `Subflow` anywhere, and **47
    // abort the process with a stack overflow** — so this is not a guard
    // nobody could see fire, it is an absent guard on a reachable recursion.
    // It predates W6 (this recursion is W3b `ad93ef2d`, the cap is W3a
    // `6f046e57`) and it is recorded with its owner and with what the artifact
    // does instead on `subflow::MAX_SPAWN_CHAIN`.
    Box::pin(on_execution_terminal_inner(
        conn,
        execution_id,
        state,
        caller,
    ))
    .await
}

async fn on_execution_terminal_inner(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
    caller: claim::Caller,
) -> Result<()> {
    let Some((parent_id, parent_run)) = parent_of(conn, execution_id).await? else {
        return Ok(());
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
        return Ok(());
    }

    let values = values_of(&ctx, &state, run)?;
    let started = spawned_count(conn, parent_id, run).await?;
    let terminated = terminated_count(conn, parent_id, run).await?;

    let terminal = match decide(
        transmit_failed(&ctx, task)?,
        as_the_loop_reads_it(child_state),
        started,
        terminated,
        values.len(),
    ) {
        Next::Terminate(s) => s,
        Next::Start(index) => {
            let value = match &values {
                LoopValues::Listed(values) => values[index].clone(),
                LoopValues::FromFile { .. } => {
                    file_values(conn, parent_id, run, index, 1).await?.remove(0)
                }
            };
            spawn_one(conn, &ctx, run, task, index, &value).await?;
            return Ok(());
        }
        Next::Wait => return Ok(()),
    };
    apply(
        &mut state,
        &Command::SetRunState {
            run,
            state: terminal,
        },
    );
    claim::settle(conn, &ctx, &before, &mut state, now, None).await
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
/// a `CREATE_NEW_EXECUTION` retry inside a `Loop` (measured over all 345).
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
/// The parent's flow-level `errors:`, `finally:` and `afterExecution:` are
/// **cleared**, and that is measured rather than tidy: `valids/all-flowable`
/// ran its flow-level `errors:` in the PARENT when the loop failed, and its
/// sub-execution ran the `Loop`'s own `errors:` instead. Egret can do the first
/// and not the second — see this module's header for the W1 gap that costs.
fn sub_flow_source(source: &str, loop_task_id: &str, owner: Uuid) -> Result<String> {
    let mut flow = crate::spec::parse(source)
        .map_err(|e| worker_err(WorkerError::Unplannable(owner, e.message)))?;
    let body = match find_task(&flow, loop_task_id).map(|t| &t.kind) {
        Some(ModelTaskKind::Loop(l)) => l.tasks.clone(),
        _ => {
            return Err(worker_err(WorkerError::Corrupt(
                owner,
                format!("task {loop_task_id:?} is not a Loop of the execution's own source"),
            )))
        }
    };
    flow.tasks = body;
    flow.errors = Vec::new();
    flow.finally_ = Vec::new();
    flow.after_execution = Vec::new();
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

/// One task by id, from anywhere in the flow.
///
/// Task ids are unique across a whole flow (`spec::validate`'s
/// `duplicate_task_id` is a structural check over the nested walk), so the id
/// is a key and not a guess — the same rule `claim::leaf_spec` relies on.
fn find_task<'a>(flow: &'a Flow, want: &str) -> Option<&'a Task> {
    fn walk<'a>(task: &'a Task, want: &str) -> Option<&'a Task> {
        if task.id == want {
            return Some(task);
        }
        task.children().into_iter().find_map(|c| walk(c, want))
    }
    flow.tasks
        .iter()
        .chain(flow.errors.iter())
        .chain(flow.finally_.iter())
        .find_map(|t| walk(t, want))
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

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

fn worker_err(e: WorkerError) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "every `Loop` the corpus declares, over 43 of its 345 files. Derived \
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
