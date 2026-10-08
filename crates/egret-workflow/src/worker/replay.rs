//! The replacement execution a `CREATE_NEW_EXECUTION` retry starts — Kestra's
//! `ExecutionService.replay`, and the four `valids/retry-new-execution-*`
//! entries. W6 Task 4.
//!
//! # What a replay is, and which of the three sub-execution kinds it is
//!
//! It is a **whole new execution of the same flow**, with an empty task-run
//! list, its own state starting at `CREATED`, the parent's `inputs`, and a
//! `parentId` naming the execution it replaces. egret has three kinds of child
//! execution and they are told apart by columns, not by convention:
//!
//! | kind | `parent_execution_id` | `parent_run_ix` / `loop_index` | writer |
//! |---|---|---|---|
//! | `Loop` iteration | set | **set** | [`super::spawn`] |
//! | **replay** | set | **NULL** | this module |
//! | `Subflow` child | NULL (`trigger_execution_id` instead) | — | [`super::subflow`] |
//!
//! Migration 0138 is what made the middle row representable and 0139 is what
//! made it *decidable*; `workflow_executions_loop_run_whole_or_absent` and
//! `workflow_executions_metadata_belongs_to_a_replay` are the two CHECKs that
//! keep the three kinds from blurring into each other.
//!
//! # The three kinds are exclusive here and are NOT in the artifact
//!
//! **A replay of a `Subflow` child is BOTH in Kestra**, and that is measured
//! rather than inferred (pinned digest, fix rounds 2-3 and re-reviews 2-3, four
//! compose projects, 38 runs): `Execution.childExecution()` copies
//! `this.trigger` verbatim (`Execution.java:422`), so the replacement carries
//! the *parent's* `executionId` and the parent's `sub` `taskRunId` alongside
//! its own `parentId`. Kestra then stamps that parent run **`FAILED`** when the
//! replacement fails.
//!
//! egret's replacement reports to nobody, so the parent's `sub` run stops at
//! `SUCCESS` — and **the size of that divergence depends on the successor, not
//! on this module**. With a successor that finishes first the parent EXECUTION
//! still agrees (16 of 16 runs); with one that outlives the replay — a
//! `Sleep PT8S`, a legal publishable flow — Kestra's parent EXECUTION ends
//! `FAILED` where egret answers `SUCCESS` (21 of 22 runs), which is a
//! divergence in the flow's own verdict. An earlier version of this doc said
//! *"every EXECUTION-level answer still agrees with the artifact"* and quoted
//! `~1.1 s` as the gap; both were properties of one fixture's trivial
//! successor.
//!
//! **What stops egret is not one thing and the register says which.** 0138's
//! `workflow_executions_one_link_kind` forbids this module's
//! `parent_execution_id` beside a trigger link — but the address is derivable
//! without storing it, one hop up `parent_execution_id` onto the child, whose
//! own trigger columns are the pair Kestra copied; what actually drops the
//! report is `subflow::on_child_terminal`'s already-terminal guard, which no
//! test in this tree has been shown to observe. No corpus flow reaches the
//! shape. The measurement, the costs and the controller's ruling are in
//! `oracle-exec/README.md`'s coverage statement item 16, and the divergence is
//! asserted as one by
//! `a_subflow_child_is_replayed_and_egret_cannot_report_it_to_the_parent`
//! rather than described here and defended there.
//!
//! **A replay DOES belong in the oracle's `spawned`, and a `Subflow` child does
//! not.** `oracle-exec/driver.rs`'s `read_trace_inner` builds that field with
//! `WHERE parent_execution_id = $1`, and all four golden entries hold
//! `{"parent_run": null, "value": null, …}` there — which is precisely a row
//! with a parent and no loop index. Measured over `traces.json`: of the 710
//! descendants the artifact produced across five harvest rounds, **40 carry a
//! NULL `loopIndex` and all 40 belong to these four flows**, ten each.
//!
//! # Where the instant comes from, and why this is not part of `advance()`
//!
//! `advance()` is pure over `(plan, state, now)` and cannot create an
//! execution. What it can do — and what [`retry::scan`](crate::executor) now
//! does — is end this execution `RETRIED` and write the artifact's own
//! `ExecutionDelay` on the run that failed
//! (`ExecutorService.java:603-608`: both retry behaviours build the same delay,
//! `.taskRunId(taskRun.getId()).date(nextRetryDate)`, and differ only in
//! `delayType`). This module is the other half of `DefaultExecutor`'s
//! **execution-delay loop**, which is a different loop from the executor cycle
//! in the artifact as well: `RESTART_FAILED_FLOW` → `executionService.replay`.
//!
//! # Three refusals, each measured rather than chosen
//!
//! 1. **Not before the deadline.** The delay is the whole of what `maxDuration`
//!    is measured against; a replay that fired at once would make every
//!    interval zero and every chain infinite.
//! 2. **Not inside a `Loop` sub-execution** — see [`spawn_due`]. Measured on the
//!    pinned artifact: Kestra cannot do it either, and fails loudly.
//! 3. **Not from a nested `settle`.** [`ExecutionCtx::depth`](super::claim) is
//!    `0` only where a fresh call stack begins, which is where every real
//!    replay arrives from (`worker::wake` → `resume`, or the driver's
//!    `pass_time` → `resume`). It matters for one shape: an `interval: PT0S`
//!    policy on a flow that can reach `FAILED` **inside** one settle — an
//!    unresolvable `Subflow` target is the reachable example — would otherwise
//!    recurse `settle → spawn → settle` in a single transaction with no bound
//!    at all. `subflow::MAX_SPAWN_CHAIN` exists because that recursion has been
//!    measured to abort the process. The artifact answers this shape the same
//!    way for a structural reason rather than a numeric one: its replay comes
//!    off a *queue*, one execution per delivery.

use serde_json::Value;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use egret_core::{Error, Result};

use super::claim;
use super::WorkerError;
use crate::executor::{ExecutionState, RunIx, RunState, Tick};

/// Start the replacement execution this one's `CREATE_NEW_EXECUTION` retry
/// scheduled, if it is due and does not exist yet.
///
/// `Ok(())` and nothing else for every execution that is not a `RETRIED` one
/// with a due delay, which is all but four flows' worth of them.
///
/// # The `Loop` refusal is the artifact's behaviour, not a simplification
///
/// **Measured on the pinned digest** (`kestra/kestra@sha256:28f59bf1…`,
/// `egret.probe4/replay-in-loop2`: a one-value `Loop` whose body's only task is
/// a `Fail` carrying `behavior: CREATE_NEW_EXECUTION, maxAttempts: 2,
/// interval: PT1S`). The loop sub-execution ends **`FAILED`**, with one attempt,
/// no `RETRIED`, and **no child execution at all**; the server log says why:
///
/// ```text
/// Flow failed from executor … with exception 'Can't find taskrun with
/// taskrunId '2xzDjVNVp77Y88mGqnc45j' on execution '67EsI18j9aAz2j6a8Ewc2a'
/// ```
///
/// The id it cannot find is the **`Loop` run's**, which lives in the parent
/// execution: `markWithTaskRunAs(…, RETRIED, markParents = true)` walks the
/// failing run's `parentTaskRunId` chain, and in a loop sub-execution that
/// chain leaves the execution. So a replay of a loop iteration is a shape
/// Kestra never produces.
///
/// egret's own `advance()` still marks such a run `RETRIED`, because its loop
/// body is a *synthesised flow* whose runs have no parent outside the
/// sub-execution — a divergence that predates this module and is not one this
/// module can repair, since `advance()` cannot see which link kind carried the
/// execution it is driving.
///
/// **The half of that divergence the PARENT could see is repaired**, and it was
/// worse than the half that remains: with the iteration `RETRIED` and no child,
/// `spawn::decide` counted it as an iteration that terminated without error and
/// the whole flow reported `SUCCESS` where Kestra reports `FAILED` on both the
/// loop run and the root. See
/// [`spawn::as_the_loop_reads_it`](super::spawn) — the throw quoted above
/// aborts Kestra's retry marking and fails the *iteration execution*, so
/// everything after it there is the ordinary `transmitFailed` path with a
/// `FAILED` child. What this module must not do is add the execution the
/// artifact does not create: that is over-acceptance, it would be invisible to
/// the oracle (no corpus flow puts a `CREATE_NEW_EXECUTION` retry inside a
/// `Loop` — measured over all 345), and the child would be an orphan anyway,
/// since [`spawn::on_execution_terminal`](super::spawn) reports a terminal only
/// through a `parent_run_ix` that a replay does not have.
///
/// **It also keeps `render::Surface::load`'s `levels` walk out of a state
/// nothing has measured.** That walk climbs `parent_execution_id` collecting
/// loop `item`s and, unlike the sibling `inheriting` flag, it does not stop at
/// a non-loop link. The only row that could make it cross one is a replay whose
/// parent is a loop iteration — exactly the row refused here, so the walk
/// cannot reach it. A replay of a top-level execution contributes no level at
/// all (its own `loop_index` is NULL), which is why `item` stays `None` all the
/// way up a replay chain.
pub(super) async fn spawn_due(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    state: &ExecutionState,
    now: Tick,
) -> Result<()> {
    // The run supplies the INSTANT and nothing else: `DefaultExecutor`'s
    // delay loop calls `executionService.replay(execution, flow, null, null,
    // empty)` -- a null `taskRunId` -- so the replay is of the whole flow and
    // the delay's own `taskRunId` is never read on this path.
    let Some(run) = due_run(state, now) else {
        return Ok(());
    };
    // A nested `settle` is not the artifact's delay loop; see the module doc's
    // third refusal. The deadline stays on the run and the next `resume` --
    // `worker::wake`'s, or the oracle driver's -- takes it.
    if ctx.depth > 0 {
        return Ok(());
    }

    let row = sqlx::query(
        "SELECT flow_id, namespace, flow_key, inputs, parent_run_ix, \
                attempt_number, origin_offset_ms, execution_depth \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(ctx.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("execution {}", ctx.id)))?;

    // A `Loop` iteration is not replayable -- measured, see this function's
    // doc. `parent_run_ix` is what distinguishes one from a replay under
    // migration 0138's `workflow_executions_loop_run_whole_or_absent`.
    if row.get::<Option<i32>, _>("parent_run_ix").is_some() {
        return Ok(());
    }

    // One replay per execution. The unique index makes a second one
    // unrepresentable (`workflow_executions_loop_run_idx`, `NULLS NOT
    // DISTINCT`, migration 0138); this makes "it already exists" an answer
    // rather than a constraint violation, which is what a second `resume` on
    // the same execution would otherwise get -- and a second `resume` is
    // ordinary: nothing clears a `not_before`.
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(ctx.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    if existing.is_some() {
        return Ok(());
    }

    let attempt: i32 = row.get("attempt_number");
    let offset: i64 = row.get("origin_offset_ms");
    // `ExecutionMetadata.nextAttempt()`: the attempt number goes up by one and
    // the original creation date does not move. Here the date is an offset, so
    // "does not move" is "grows by however much of this execution's own clock
    // has run" -- `now`, the instant this settle is deciding at, which is the
    // instant the child is created.
    //
    // **`now` and not the deadline it passed**, for the artifact's own reason:
    // Kestra creates the replacement when the delay loop next runs, not at the
    // instant the delay named, and its `createdDate` is that later instant.
    // Measured on the pinned digest: `retry-new-execution-flow-attempts` has
    // `interval: PT1S` and its first replay was created 1.194 s after the
    // failure, not 1.000 s. Reading the deadline instead would also give up the
    // one guarantee this arithmetic has -- that the offset grows by strictly
    // more than the interval on every hop.
    let child_offset = offset.saturating_add(now.0.max(0));

    // In a savepoint, for `subflow::spawn_one`'s reason: the FILE-input check
    // below needs the row and may refuse it.
    let mut sp = sqlx::Connection::begin(&mut *conn).await.map_err(db_err)?;
    let child: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            parent_execution_id, attempt_number, origin_offset_ms, inputs, execution_depth) \
         VALUES ($1,$2,$3,$4,$5,'CREATED','[\"CREATED\"]'::jsonb,$6,$7,$8,$9,$10) \
         RETURNING id",
    )
    .bind(ctx.workspace_id)
    .bind(row.get::<Option<Uuid>, _>("flow_id"))
    .bind(row.get::<String, _>("namespace"))
    .bind(row.get::<String, _>("flow_key"))
    // The frozen source, not a re-resolution of `(namespace, flowId)`: Kestra
    // replays the flow the execution was started with (`childExecution` copies
    // everything but the id, the task runs and the state), and a replay that
    // picked up an edit made since would be a different flow with the same
    // trace.
    .bind(&ctx.source)
    .bind(ctx.id)
    .bind(attempt.saturating_add(1))
    .bind(child_offset)
    // `this.inputs` in `Execution.childExecution`. Read out of the parent row
    // rather than carried on the context for `spawn::spawn_one`'s reason: the
    // row is the one authority on what this execution was started with.
    .bind(row.get::<Value, _>("inputs"))
    // **The original's depth, unchanged** (W13a final review I1):
    // `ExecutionService.replay` keeps the execution's metadata
    // (`metadata.nextAttempt()`, a `toBuilder` copy with only the attempt
    // bumped), so `executionDepth` carries over. A replay is the same link in
    // a chain, not a new one.
    .bind(row.get::<i32, _>("execution_depth"))
    .fetch_one(&mut *sp)
    .await
    .map_err(db_err)?;

    // W9a spec §7.1: every execution-creating INSERT checks its FILE inputs.
    // A replay's are its source's, already checked, and the source is on the
    // replay's chain (`parent_execution_id` with no `loop_index`) -- so this
    // passes unless a copied value is no longer readable (its index row
    // pruned by retention, spec §10). That **fails the retried run naming
    // the input** (fix round 1, M4) -- no replay is created, and the run is no
    // longer `RETRIED`, so no later settle tries again -- rather than erring
    // the worker, which would re-claim it for ever.
    let flow = crate::spec::parse(&ctx.source)
        .map_err(|e| worker_err(WorkerError::Unplannable(child, e.message)))?;
    let checked = crate::storage::inputs::check_file_inputs(
        &mut sp,
        ctx.workspace_id,
        child,
        &flow,
        &row.get::<Value, _>("inputs"),
        &[],
    )
    .await?;
    if let Err(why) = checked {
        sp.rollback().await.map_err(db_err)?;
        return Box::pin(super::subflow::complete(
            conn,
            ctx.id,
            run,
            &[RunState::Failed],
            None,
            Some(&format!(
                "the CREATE_NEW_EXECUTION retry cannot replay: {why}"
            )),
            claim::Stack::Nested(ctx.caller()),
        ))
        .await;
    }
    sp.commit().await.map_err(db_err)?;

    let child_ctx = claim::ExecutionCtx {
        id: child,
        workspace_id: ctx.workspace_id,
        flow_id: row.get("flow_id"),
        source: ctx.source.clone(),
        // `build_plan` is deterministic over the source and the source is the
        // same string, so this is the parent's plan by construction. It is
        // rebuilt rather than cloned because `Plan` is not `Clone` and a plan
        // that could be moved between two contexts is a plan two executions
        // could disagree about.
        plan: crate::executor::build_plan(&ctx.source)
            .map_err(|e| worker_err(WorkerError::Unplannable(child, e.to_string())))?,
        // Its own logical clock, for `spawn::spawn_one`'s reason: the
        // happens-before relation is compared PER EXECUTION.
        seq: 0,
        // One deeper than its caller: the `settle` below runs inside this one.
        depth: ctx.depth + 1,
        // The caller's cache: this settle runs in its transaction (W9c Task 9).
        prefetch: ctx.prefetch.clone(),
    };
    let empty = ExecutionState::new(&child_ctx.plan);
    let mut child_state = empty.clone();
    // `Tick(0)`: a `Tick` is milliseconds since the EXECUTION was created and
    // this is that instant. The chain's accumulated time is in
    // `origin_offset_ms`, which `claim::load_with` turns back into
    // `ExecutionMeta::origin` -- deliberately NOT folded into `now`, so that
    // every other tick in this execution stays measured from its own
    // `created_at`.
    //
    // Boxed: this settle can reach `spawn_due` again through the chain, so
    // `settle -> here -> settle` is a real cycle in the type graph. It is not a
    // cycle in the data -- each link is a new execution row with a strictly
    // greater `attempt_number`, and the policy that produced it is what stops
    // the chain.
    Box::pin(claim::settle(
        conn,
        &child_ctx,
        &empty,
        &mut child_state,
        Tick(0),
        None,
    ))
    .await?;
    Ok(())
}

/// The `RETRIED` run whose `ExecutionDelay` has come due, if there is one.
///
/// # This is also what says the EXECUTION was retried, and that used to be a
/// second guard
///
/// `spawn_due` opened with `if state.history.last() != Some(&RunState::Retried)
/// { return Ok(()) }`. It was **measured redundant** rather than assumed so:
/// mutated to `if false`, `--lib` (636), `--test worker` (52) and
/// `--test executor` (45, ratchet included and unmoved at 127) were all green,
/// which is the shape this programme rules indistinguishable from a guard that
/// is merely weak — so the difference had to be established, and it is this.
///
/// A run reaches the filter below only if its own history ends `RETRIED` **and**
/// it carries a deadline, and exactly one thing in the crate writes that pair:
/// `retry::scan`'s `CREATE_NEW_EXECUTION` arm, which emits
/// `SetExecutionState { Retried }` in the SAME command list — asserted, on the
/// same fixture, by `executor::tests::create_new_execution_ends_the_execution_retried`
/// (run history, execution history and `not_before` all in one test). The other
/// behaviour leaves the run `RETRYING`, which this filter never selects, and
/// `RETRIED` is terminal, so `advance()` returns nothing for such an execution
/// and no second deadline can appear on it.
///
/// So the condition was folded into the one place it is derivable from, where
/// deleting the fact **does** kill a test, instead of standing as a line no
/// mutation could reach. (`next_retry_at`'s `NonZeroU32` is the precedent.)
///
/// **At most one can exist**, and it is by construction rather than by luck:
/// `retry::scan` returns on the first eligible run, its `CREATE_NEW_EXECUTION`
/// arm ends the execution `RETRIED`, and `advance()` returns nothing at all for
/// a terminal execution — so no second `ScheduleRetry` can ever be written to
/// this execution. The earliest deadline is taken anyway, because "there can be
/// only one" is a property of a different function and a caller that silently
/// picked the first of two would be reading it as an assumption.
///
fn due_run(state: &ExecutionState, now: Tick) -> Option<RunIx> {
    (0..state.runs.len())
        .filter(|&ix| state.runs[ix].history.last() == Some(&RunState::Retried))
        .filter_map(|ix| state.runs[ix].not_before.map(|t| (t, ix)))
        .filter(|(t, _)| *t <= now)
        .min()
        .map(|(_, ix)| ix)
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

fn worker_err(e: WorkerError) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}
