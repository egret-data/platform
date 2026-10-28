//! [`Command::SpawnSubflow`] — the child execution a
//! `io.kestra.plugin.core.flow.Subflow` starts, and the report back when it
//! terminates. W6 Task 3.
//!
//! # What a subflow child is, and what it is NOT
//!
//! It is a **full execution of a different flow**, resolved by
//! `(namespace, flowId)`, with its own inputs, its own `taskRunList` and its own
//! row in the executions list. It is **not** a part of its parent's record, and
//! the difference is a column: it is linked by `trigger_execution_id` /
//! `trigger_run_ix` (migration 0138) and **never** by `parent_execution_id`,
//! which is [`super::spawn`]'s.
//!
//! That is not a tidying preference. `oracle-exec/driver.rs`'s
//! `read_trace_inner` builds the oracle's `spawned` field with
//! `WHERE parent_execution_id = $1`, and **all 41 golden `spawned`-carrying
//! entries have zero `Subflow` children** — so a child linked the other way
//! would turn every entry this module exists to bank into a `spawned`
//! mismatch. `workflow_executions_one_link_kind` makes the wrong combination
//! unrepresentable rather than merely discouraged.
//!
//! # The five facts this module is built on, measured on the ARTIFACT
//!
//! Taken from `kestra/kestra@sha256:28f59bf1…` — the pinned digest — by running
//! flows against it, not from the sibling checkout, which W3b Task 3 established
//! is a **different build**. The class citations below say what to look at; the
//! numbers say what happened.
//!
//! 1. **The child carries no `parentId`.** `io.kestra.tests/subflow-parent`,
//!    executed on the artifact: the child's `parentId` is `null` and its
//!    `trigger` is
//!    `{"id": "subflow", "type": "io.kestra.plugin.core.flow.Subflow",
//!      "variables": {"executionId": …, "taskRunId": …, "taskId": "subflow",
//!      "namespace": "io.kestra.tests", "flowId": "subflow-parent",
//!      "flowRevision": 1}}`. `SELECT id FROM executions WHERE parent_id = <parent>`
//!    returns **zero rows**, which is also why `traces.json` records
//!    `"descendants": []` for that flow in all five harvest rounds.
//! 2. **The parent run goes `CREATED RUNNING <terminal>` with no `SUBMITTED`.**
//!    No worker ever sees a `Subflow`. That is the whole reason
//!    `executor::TaskKind::Executable` exists — see its doc.
//! 3. **`wait: false` does not wait, and the parent sees the child's state at
//!    SPAWN time.** `egret.probe/probe-nowait-parent` printed
//!    `raw={"state":"CREATED","executionId":"61J0wKdkpWSS8MUTSfOPd8"}` — no
//!    `outputs` key at all — while the same flow with `wait: true`
//!    (`egret.probe/probe-outputs-parent`) printed
//!    `eid=… state=SUCCESS o1=hello`.
//! 4. **`transmitFailed` is applied ON TOP of the child's own state, not
//!    instead of it.** `egret.probe/probe-notransmit-parent`
//!    (`transmitFailed: false` over a child that FAILS) gives a parent run AND
//!    attempt reading `["CREATED","RUNNING","FAILED","SUCCESS"]`. The golden
//!    holds the same two-step for `valids/task-allow-failure-executable-flow`
//!    (`… FAILED, WARNING`). See [`guess_state`].
//! 5. **An unresolvable `(namespace, flowId)` fails the RUN, not the flow.**
//!    `egret.probe/probe-unresolvable` published with **200** — Kestra does not
//!    refuse the flow — and executing it left the `Subflow` run
//!    `["CREATED","FAILED"]` (one attempt: `["FAILED"]`), the successor never
//!    started, the execution FAILED, and **no child execution was created**.
//!    Inside a `Parallel` (`egret.probe/probe-unresolvable-nested`) only that
//!    run failed and its sibling still ran. See [`spawn_one`].
//!
//! # Where this is knowingly narrower than Kestra, stated rather than found
//!
//! * **The flow-level `outputs:` block is not evaluated**, here or anywhere in
//!   egret (`render::Surface` publishes `execution.outputs` as `{}` for the
//!   same reason), so `outputs.<task>.outputs` is always the empty map. The one
//!   corpus entry that reads it, `valids/subflow-secret-output-parent`, is
//!   baselined and stays so — and it would not match anyway: on the artifact
//!   that flow FAILS inside `ExecutableUtils.subflowExecutionResultFromChildExecution`'s
//!   catch block (`ExecutableUtils.java:305-309`), which is what produces its
//!   golden attempt of `["CREATED","FAILED"]` beside a run of
//!   `["CREATED","RUNNING","FAILED"]`.
//! * ~~**`allowFailure`/`allowWarning` are unrepresentable**~~ — **W8 Task 5.**
//!   `models::Task` carries both and `RunState` has `Warning`, so
//!   [`guess_state`] implements `ExecutableUtils.guessState` whole rather than
//!   its `transmitFailed` half. `valids/task-allow-failure-executable-flow` and
//!   `valids/task-allow-warning-executable-flow` are the two entries it used to
//!   cost and **both bank**. `transmitFailed: false` is still unreachable from
//!   the corpus and still implemented.
//! * **`revision`, `labels`, `inheritLabels`, `scheduleDate` and
//!   `restartBehavior` are not on `models::Subflow`** (W1's curation), so
//!   nothing here can honour them. `valids/task-flow-inherited-labels` and
//!   `valids/subflow-inherited-labels-parent` are the entries that names.
//!
//! # A gap that is the ORACLE's and not this worker's
//!
//! `oracle-exec/driver.rs`'s `pass_time` walks `parent_execution_id` only, so
//! the driver's clock never reaches a retry deadline held by a subflow child.
//! **That is a coverage limit of the harness, not a stall in the product**, and
//! fix round 1's review is what made the distinction: `claim::due_retries` is a
//! **global** scan, so a shipping worker wakes such a retry like any other. No
//! `Subflow` target in the corpus declares a `retry:` in any case, so nothing
//! reaches even the harness limit.

use egret_core::{Error, Result};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::render::Surface;
use super::{claim, WorkerError};
use crate::executor::{
    apply, build_plan, Command, ExecutionState, RunIx, RunState, TaskIx, TaskKind,
};
use crate::models::{Flow, Subflow, TaskKind as ModelTaskKind};
use crate::runners::{render_str, RenderContext};
use crate::trigger::flow::DEFAULT_MAX_EXECUTION_DEPTH;

/// **How far down a chain of child executions this worker will spawn**, and the
/// second half of the one answer this module gives that is NOT the artifact's.
///
/// It is a **backstop**, not the guard. [`chain_holds`] is the guard: it
/// refuses a spawn whose target is already somewhere up this execution's own
/// causal chain, which is what a cycle is. This bounds the case a cycle check
/// cannot see — a legitimately **acyclic** chain deep enough to exhaust a
/// stack, because [`spawn_one`] drives the child's first settle inside its
/// caller's transaction and each link is a nested `settle`.
///
/// # Fix round 1 corrected the argument that said this could not exist
///
/// The first version of this module shipped a cycle check and **no** bound, on
/// the argument that *"a depth bound large enough to be worth having is not one
/// this recursion can survive"* — measured, a 16-link chain overflowed the
/// stack. **That argument was made against a guard that turned out not to cover
/// the reachable case**, and it generalised a measurement about a bound used as
/// THE guard (where it would have to be large) to a bound used as a backstop
/// (where it does not). As a backstop the number only has to be larger than
/// anything real and smaller than what kills a stack, and there is room between
/// those: the corpus's deepest chain is **2 links**.
///
/// **8 is four times that**, and it fires long before the 16 that was measured
/// to abort. `tests/worker.rs`'s
/// `a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing`
/// is the positive control and asserts both halves — the chain stops AND the run
/// that hit the bound says why — because a backstop nobody has seen fire is a
/// backstop nobody has seen.
///
/// # It counts BOTH link kinds, and it counts FRAMES rather than rows
///
/// A `Loop` sub-execution is one nested `settle` exactly as a subflow child is,
/// so a `Subflow` inside a loop body inside a loop body is three deep whatever
/// column each hop used. It is read off [`claim::ExecutionCtx::depth`], a
/// counter carried down the call stack, and **not** off a query — mutation M9
/// is why: the first version shared its walk with [`chain_holds`], so a
/// mutation that broke the walk broke the bound too and the reproduction
/// aborted the test binary instead of failing an assertion. A backstop derived
/// from the thing it is backstopping is not a backstop.
///
/// # Fix round 2: it counted ONE DESCENT, and a completing chain reset it
///
/// *"Only frames can count frames"* was right and the counter was still not
/// counting them. `claim::load_with` wrote `depth: 0` for every caller, under
/// a comment claiming none of them was inside another `settle` — and the two
/// that a **completing** chain goes through both are. So this bound never
/// fired on the shape a `wait: true` subflow actually has: the reviewer
/// instrumented the counter, watched it cycle `0,1,2,0,1,2,…`, and aborted the
/// worker with `fatal runtime error: stack overflow` from **nine flows filed
/// in one workspace**. `claim::Stack` is the fix — a load must SAY whether it
/// begins a fresh stack or nests inside one — and
/// `a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter` is the
/// control, because a test that exercises only the descent cannot see a defect
/// in the ascent.
///
/// # What 8 means once the ascent is counted, which is not "8 subflow links"
///
/// It is **8 nested `settle` frames**, and a completing chain spends more than
/// one per link. A plain descent still gets 8 links — each spawn is one frame.
/// But a flow that spawns a subflow, *waits for it to finish*, and then spawns
/// another **in the same execution** costs two frames per subflow: the
/// `complete` that re-enters the parent's settle, and the spawn it then makes.
/// So ~3-4 such subflows reach the bound where round 1's counter would have
/// said 8.
///
/// **That is honest rather than a regression**, and the distinction is where
/// the child terminates. A child that runs a real task needs a worker tick, and
/// the tick enters at [`claim::Stack::Fresh`] with nothing of this crate's
/// below it — a genuinely fresh stack, and the counter is right to restart. The
/// frames only pile up when children terminate **inside** the caller's
/// transaction, which is exactly when the stack really is that deep. Nothing in
/// the corpus is near it (deepest chain: 2 links) and the ratchet is unmoved at
/// **127** with the same twelve, which is what says so. *(This read `131` — the
/// value the day W6 Task 3 measured it, and W6 Task 4's four replay banks moved
/// it four lines later. W6 Task 8's sweep re-derived it: the claim is "unmoved",
/// and the number beside it is a moment.)*
///
/// # What neither bound closes, measured on the artifact rather than assumed
///
/// **A nested `Loop` reaches this same recursion with no `Subflow` anywhere,
/// and nothing refuses it.** 12 statically nested one-value `Loop`s run to
/// completion at `ctx.depth = 12`; **47 of them abort the process**, from a
/// single flow file. `spawn::spawn_one` does not consult this constant, and
/// `plan::MAX_TASK_DEPTH` (`8 * MEASURED_MAX_CORPUS_DEPTH` = **48**) is the
/// only limit on the nesting — a number chosen for headroom over the corpus,
/// never against a stack, and it sits on the wrong side of the cliff.
///
/// **What the artifact does, measured on the pinned digest** (`egret.probe2`,
/// one flow per depth, one-value `Loop`s around a `Return`):
///
/// | nested `Loop`s | Kestra | egret |
/// |---|---|---|
/// | 12 | published, executed, `CREATED RUNNING SUCCESS` | runs, depth 12, unrefused |
/// | 47 | published, executed, **SUCCESS** | **SIGABRT — stack overflow** |
/// | 100 | published, executed, **SUCCESS** in 18 s | not reached |
/// | 498 | published | — |
/// | 499 | **422** `Illegal genericflow source: YAML parsing error: Document nesting depth (1001) exceeds the maximum allowed (1000, from StreamReadConstraints.getMaxNestingDepth())` | — |
///
/// *(The egret column is the measurement before W9c Task 9 fix round 3.
/// Re-measured at round 3 (re-review 3, 2026-10-09): every settle now runs
/// from `claim::walk`'s explicit stack, so 46 nested `Loop`s run to SUCCESS
/// and 47 is refused by `plan::MAX_TASK_DEPTH` before anything runs; nothing
/// in this table's range aborts any more.)*
///
/// So **Kestra imposes no semantic bound on `Loop` nesting at all**: its only
/// refusal is Jackson's document-nesting limit in the YAML reader, two levels
/// per `Loop`, and it is a *parse* error rather than a rule about flowables.
/// "Fix it with the artifact's behaviour" is therefore not available to
/// `spawn::spawn_one` — the artifact's behaviour is *accept and run it*, and
/// egret cannot, because [`spawn_one`] settles the child inside its caller's
/// transaction.
///
/// **This is recorded rather than fixed, and it is recorded with an owner.**
/// It predates W6 — `spawn::on_execution_terminal`'s recursion is W3b
/// (`ad93ef2d`) and `MAX_TASK_DEPTH` is W3a (`6f046e57`), and neither is in
/// this task's diff — and every candidate fix is a new number chosen against a
/// stack this code does not own, measured in a test thread and shipped to a
/// tokio worker with a different one. **It belongs to whoever changes
/// `claim::settle` to stop driving a child's first settle inside its caller's
/// transaction**; until that happens, `plan::MAX_TASK_DEPTH` is the number that
/// has to move and it is W3a's constant, not this module's.
pub const MAX_SPAWN_CHAIN: usize = 8;

/// One [`Command::SpawnSubflow`], kept as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Requested {
    pub task: TaskIx,
    pub run: RunIx,
}

impl Requested {
    /// The [`Command`] arm this is, or `None` for every other command.
    pub(super) fn of(cmd: &Command) -> Option<Self> {
        match cmd {
            Command::SpawnSubflow { task, run } => Some(Requested {
                task: *task,
                run: *run,
            }),
            _ => None,
        }
    }
}

/// Resolve one `Subflow`'s target, create its execution, and return its first
/// settle -- and, for `wait: false`, the completion of this run -- as jobs for
/// the walk (W9c Task 9 fix round 3), or the job that fails this run when it is
/// refused. What the settle's `Subflows` phase runs, once per request.
///
/// The whole of it -- the child row, its task runs, and whatever the child's
/// own first round asks for -- is on the caller's connection and therefore in
/// the caller's transaction, for `spawn::open_batch`'s reason: a child that
/// existed while its parent's run did not know it had been started is the
/// window the plan's Global Constraint forbids.
///
/// # The unresolvable target is a decision, and it is the artifact's
///
/// Fact 5 in this module's doc. egret does the same three things: it does **not**
/// refuse the flow (a `build_plan` that refused would turn `valids/task-runif`
/// from `runs` into `refused`, which is a regression on an entry whose
/// `Subflow` is SKIPPED and never resolves anything); it fails **that run**;
/// and it creates no child.
///
/// **One shape diverges and it is unreachable.** Kestra never emits `RUNNING`
/// for such a run, because the lookup happens while it is still building the
/// `SubflowExecution`; egret has already emitted `StartRun` in the same round
/// that emitted the spawn — see [`Command::SpawnSubflow`] for why it cannot be
/// emitted alone — so egret's run reads `["CREATED","RUNNING","FAILED"]` where
/// the artifact's reads `["CREATED","FAILED"]`. **No golden entry reaches it**:
/// the corpus's one unresolvable target belongs to `valids/task-runif`'s
/// `notexecutedflowable`, whose falsy `runIf:` skips it first, and
/// `tests/executor.rs`'s `the_only_unresolvable_subflow_target_in_the_corpus_is_skipped`
/// re-derives that over all 351 flows rather than remembering it.
pub(super) async fn spawn_one(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    surface: &Surface,
    state: &ExecutionState,
    r: &Requested,
) -> Result<Vec<claim::Job>> {
    let task_id = ctx
        .plan
        .tasks
        .get(r.task)
        .map(|t| t.id.clone())
        .ok_or_else(|| {
            worker_err(WorkerError::Corrupt(
                ctx.id,
                format!(
                    "a subflow spawn names task {}, which the plan does not hold",
                    r.task
                ),
            ))
        })?;
    let flow = crate::spec::parse(&ctx.source)
        .map_err(|e| worker_err(WorkerError::Unplannable(ctx.id, e.message)))?;
    let sub = subflow_of(&flow, &task_id).ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            ctx.id,
            format!("task {task_id:?} is planned Executable but is not a Subflow of the execution's own source"),
        ))
    })?;

    let cx = surface.context(&ctx.plan, state, Some(r.run));

    // Rendered, because Kestra renders them:
    // `runContext.render(currentTask.subflowId().namespace())` and the same for
    // `flowId` (`ExecutableUtils.java:173-174`). `valids/task-flow-dynamic` is
    // the corpus flow that writes `{{ inputs.namespace }}` there; it has no
    // golden entry, so nothing here banks on the rendering, but a resolver that
    // compared the raw source would silently look up a `{{ … }}` as a name.
    let (namespace, flow_id) = match (
        render_name(sub.namespace.as_deref(), &cx),
        render_name(sub.flow_id.as_deref(), &cx),
    ) {
        (Ok(Some(ns)), Ok(Some(id))) => (ns, id),
        (Err(e), _) | (_, Err(e)) => {
            return Ok(fail_run(ctx, r.run, &format!("subflow target: {e}")));
        }
        // `@NotEmpty`/`@NotNull` in Java, `Option` here because bean validation
        // is not parsing (`models::Subflow`). `spec::validate` refuses both, so
        // a flow reaching here without them has been started from a source that
        // never went through publish — which the oracle's driver does.
        _ => {
            return Ok(fail_run(
                ctx,
                r.run,
                "subflow task has no namespace/flowId to resolve",
            ));
        }
    };

    // Before anything is written, two refusals -- the one answer here that is
    // not the artifact's, and the reason is a measured stack overflow rather
    // than a preference.
    //
    // **The backstop is asked FIRST and it asks no question of the database.**
    // `ctx.depth` is a counter carried down the call stack
    // (`claim::ExecutionCtx::depth`), so it cannot be wrong in the same way the
    // walk below can be -- which is not a hypothetical: mutation M9 broke the
    // walk and the FIRST version of this bound, which shared it, went down with
    // it and aborted the test binary instead of failing an assertion.
    if ctx.depth + 1 > MAX_SPAWN_CHAIN {
        return Ok(fail_run(
            ctx,
            r.run,
            &Refusal::TooDeep(ctx.depth).message(&namespace, &flow_id),
        ));
    }
    if chain_holds(conn, ctx.id, &namespace, &flow_id).await? {
        return Ok(fail_run(
            ctx,
            r.run,
            &Refusal::Cycle.message(&namespace, &flow_id),
        ));
    }

    let Some((child_flow, source)) = resolve(conn, ctx.workspace_id, &namespace, &flow_id).await?
    else {
        // The artifact's own message, kept close to it:
        // `"Unable to find flow '" + ns + "'.'" + id + "' with revision '0'"`.
        // egret has no revision, so the clause is dropped rather than faked.
        return Ok(fail_run(
            ctx,
            r.run,
            &format!("unable to find flow '{namespace}'.'{flow_id}'"),
        ));
    };

    let child = match crate::spec::parse(&source) {
        Ok(f) => f,
        Err(e) => {
            // `Cannot execute an invalid flow` — `ExecutableUtils.java:200`.
            return Ok(fail_run(
                ctx,
                r.run,
                &format!(
                    "cannot execute an invalid flow '{namespace}'.'{flow_id}': {}",
                    e.message
                ),
            ));
        }
    };
    // **The THIRD door, and it has to be here rather than in [`resolve`].**
    //
    // A flow can be switched off two ways -- the `state` column the management
    // UI writes, and the YAML's own `disabled: true` -- and until W10 Task 5 fix
    // round 2 this path honoured only the first. `resolve`'s SQL tests
    // `deleted_at IS NULL AND state <> 'disabled'`; `disabled:` is not a column,
    // so no predicate can see it and the check cannot live there. It lives here
    // because this is the first line at which the parsed flow exists.
    //
    // **The failure it closes was concrete**: an operator writes
    // `disabled: true`, `POST .../flows/{id}/executions` answers 409, and a
    // sibling flow naming that flow as a `Subflow` target ran it anyway.
    // `routes::startable_flow` closed both doors for the API in fix round 1 and
    // its comment claimed this function agreed -- it did not, and that claim is
    // corrected there.
    //
    // **The message is `resolve`'s miss verbatim, deliberately.** Both other
    // doors already answer "unable to find flow": they are inside `resolve`,
    // which returns `Option` and cannot distinguish. Giving this door its own
    // wording would mean an operator who typed `disabled: true` and an operator
    // who set the column got DIFFERENT answers for the same intent. One switch,
    // one answer. **It is a divergence from the artifact and a deliberate one**
    // -- `ExecutableUtils` raises a distinct error for `isDisabled()` -- but the
    // divergence is pre-existing on the column door and this keeps it at one
    // rather than two.
    if child.disabled {
        return Ok(fail_run(
            ctx,
            r.run,
            &format!("unable to find flow '{namespace}'.'{flow_id}'"),
        ));
    }

    let plan = match build_plan(&source) {
        Ok(p) => p,
        Err(e) => {
            return Ok(fail_run(
                ctx,
                r.run,
                &format!("cannot execute an invalid flow '{namespace}'.'{flow_id}': {e}"),
            ));
        }
    };
    // No-silent-stall §4.3 point 5: a child this deployment cannot run would
    // stall, and the parent with it. The child is inserted below, not through
    // `start_execution_in_on`, so its start check does not reach here.
    let unserved = crate::spec::unrunnable(&child, crate::runtime::served());
    if !unserved.is_empty() {
        return Ok(fail_run(
            ctx,
            r.run,
            &format!(
                "cannot execute an invalid flow '{namespace}'.'{flow_id}': {}",
                crate::spec::describe(&unserved)
            ),
        ));
    }

    // **The child's `metadata.executionDepth`, and the artifact's refusal on
    // it** (W13a final review I1). `ExecutableUtils.subflowExecution` on the
    // pinned artifact (javap, offsets 703-792 and 1092-1100) writes
    // `executionDepthOrZero() + 1` on the child and, when that exceeds
    // `maxDepth` (100), logs and throws "Cannot execute flow '%s'.'%s': the
    // execution chain exceeded the maximum depth of %d" -- after the target is
    // resolved, found enabled and found valid, which is where this sits. The
    // throw fails the `Subflow` task, so here the run FAILS with that message
    // (its config-property sentence dropped: egret has no such property).
    //
    // Until this, the child was inserted at 0, so a chain that crossed a
    // `Subflow` restarted its count and a Flow-trigger cycle through one
    // could only be stopped by `chain_holds`, never by the depth.
    let parent_depth: i32 =
        sqlx::query_scalar("SELECT execution_depth FROM workflow.executions WHERE id = $1")
            .bind(ctx.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
    let child_depth = parent_depth.saturating_add(1);
    if i64::from(child_depth) > i64::from(DEFAULT_MAX_EXECUTION_DEPTH) {
        return Ok(fail_run(
            ctx,
            r.run,
            &format!(
                "cannot execute flow '{namespace}'.'{flow_id}': the execution chain exceeded \
                 the maximum depth of {DEFAULT_MAX_EXECUTION_DEPTH}"
            ),
        ));
    }

    // **The child's inputs are the `Subflow`'s own `inputs:`, rendered — NOT
    // the parent's.** That is the difference from `spawn::spawn_one`, which
    // copies the parent's inputs down because a loop body is the same flow's
    // `tasks:` running in its parent's context. A subflow is a different flow
    // with its own `inputs:` declaration, and `render::Surface::load` applies
    // that flow's own defaults and types to whatever is stored here
    // (`with_declared_defaults`), exactly as it does for a posted top-level
    // input. `valids/task-flow` is the corpus flow this decides: it passes
    // `string: "{{ inputs.string }}"` to `io.kestra.tests/switch`, the harvest
    // posted `THIRD`, and the THIRD case is the one that fails — which is the
    // whole of why that entry's golden is FAILED and not SUCCESS.
    let inputs = match rendered_inputs(sub, &cx) {
        Ok(v) => v,
        Err(e) => {
            return Ok(fail_run(ctx, r.run, &format!("subflow inputs: {e}")));
        }
    };
    // Package #3 (spec D7): the CHILD's declared `inputs:` are checked as a
    // top-level start's are -- the same checker, before `split` so a SECRET
    // `validator` sees the clear value -- and the coerced object is stored.
    // A refusal fails this `Subflow` run, as Kestra's "Subflow task FAILED".
    // The child does not go through `start_execution_in_on`: it has its own
    // INSERT below.
    let inputs = match claim::checked_inputs(&child, &inputs) {
        Ok(v) => v,
        Err(e) => {
            return Ok(fail_run(
                ctx,
                r.run,
                &format!("subflow inputs: {}", e.message),
            ));
        }
    };
    // KI-112: the CHILD's declaration decides which of them are SECRET.
    let (inputs, secret_inputs) = match crate::secret_inputs::split(&child, inputs) {
        Ok(split) => split,
        Err(Error::InvalidInput(e)) => {
            return Ok(fail_run(ctx, r.run, &format!("subflow inputs: {e}")));
        }
        Err(e) => return Err(e),
    };

    // **In a savepoint, because the FILE-input check needs the row and may
    // refuse it** (W9a spec §7.1). The readable set is walked FROM the child
    // -- its `trigger_execution_id` is this execution, which is what makes the
    // parent's files readable to it -- so the check runs after the INSERT; a
    // refusal rolls the row back and fails this run naming the input, the way
    // a target that does not resolve fails it above. The parent's run is the
    // author's to fix, never the worker's.
    let mut sp = sqlx::Connection::begin(&mut *conn).await.map_err(db_err)?;
    let child_id: Uuid = sqlx::query_scalar(concat!(
        "INSERT INTO workflow.executions \
               (workspace_id, flow_id, namespace, flow_key, source, state, history, \
                trigger_execution_id, trigger_run_ix, inputs, execution_depth, secret_inputs, ",
        super::stamps::stamp_columns!(),
        ") VALUES ($1,$2,$3,$4,$5,'CREATED','[\"CREATED\"]'::jsonb,$6,$7,$8,$9,$10, ",
        super::stamps::stamp_values!("'CREATED'", "'[\"CREATED\"]'"),
        ") RETURNING id",
    ))
    .bind(ctx.workspace_id)
    .bind(child_flow)
    .bind(&child.namespace)
    .bind(&child.id)
    .bind(&source)
    .bind(ctx.id)
    .bind(r.run as i32)
    .bind(&inputs)
    .bind(child_depth)
    .bind(secret_inputs)
    .fetch_one(&mut *sp)
    .await
    .map_err(db_err)?;
    let checked = crate::storage::inputs::check_file_inputs(
        &mut sp,
        ctx.workspace_id,
        child_id,
        &child,
        &inputs,
        &[],
    )
    .await?;
    if let Err(why) = checked {
        sp.rollback().await.map_err(db_err)?;
        return Ok(fail_run(
            ctx,
            r.run,
            &format!("subflow '{namespace}'.'{flow_id}': {why}"),
        ));
    }
    sp.commit().await.map_err(db_err)?;

    let child_ctx = claim::ExecutionCtx {
        id: child_id,
        workspace_id: ctx.workspace_id,
        flow_id: Some(child_flow),
        source,
        plan,
        // Its own logical clock, for `spawn::spawn_one`'s reason: the
        // happens-before relation is compared PER EXECUTION.
        seq: 0,
        // One deeper than its caller: the `settle` below runs inside this one.
        // See `ExecutionCtx::depth`, and the guard above that reads it.
        depth: ctx.depth + 1,
        // The caller's cache: this settle runs in its transaction (W9c Task 9).
        gate: ctx.gate.clone(),
    };
    // `wait: false`: the parent does not wait, and what it sees is the child's
    // state at THIS instant rather than its terminal — measured, fact 3. The
    // run is already RUNNING (the `StartRun` beside the spawn), so this is the
    // second half of `CREATED RUNNING SUCCESS`, which is exactly what
    // `valids/subflow-parent-no-wait`'s golden holds.
    //
    // It has to be read from the row rather than from the child's state,
    // because the child's settle may have terminated it already and Kestra's
    // answer is the state the child had when the parent stopped caring.
    //
    // Two jobs, in this order: the child's first settle -- to the end of its
    // tail -- and then this run's completion, a settle of THIS execution that
    // nests inside its own (`claim::Frame`'s re-entrancy). The walk runs them
    // where the inline calls ran.
    let wait = sub.wait;
    let mut jobs = vec![claim::Job::Settle(Box::new(claim::Frame::first(child_ctx)))];
    if !wait {
        jobs.push(claim::Job::Complete(Completion {
            execution_id: ctx.id,
            run: r.run,
            states: vec![RunState::Success],
            outputs: Some(json!({"executionId": child_id.to_string(), "state": "CREATED"})),
            message: None,
            // `Nested`: this is inside this execution's own `settle`, and the
            // one the completion runs nests inside it.
            stack: claim::Stack::Nested(ctx.caller()),
            unless_terminal: false,
        }));
    }
    Ok(jobs)
}

/// One subflow child has reached a terminal state: tell the run that started it.
///
/// The half of `Subflow` that `advance()` cannot own, for [`super::spawn::on_execution_terminal`]'s
/// reason: it is pure over `(plan, state, now)` and [`ExecutionState`] holds
/// nothing about another execution.
///
/// `Ok(None)` and nothing else for an execution that is not a subflow child,
/// which is almost all of them. Otherwise it returns the parent run's
/// completion as a job for the walk (W9c Task 9 fix rounds 2-3), so a
/// completing chain does not nest one settle inside the next.
pub(super) async fn on_child_terminal(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
    caller: claim::Caller,
) -> Result<Option<claim::Job>> {
    Box::pin(on_child_terminal_inner(
        conn,
        execution_id,
        child_state,
        caller,
    ))
    .await
}

async fn on_child_terminal_inner(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
    caller: claim::Caller,
) -> Result<Option<claim::Job>> {
    let Some((parent_id, parent_run)) = trigger_of(conn, execution_id).await? else {
        return Ok(None);
    };
    let run = usize::try_from(parent_run).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!("negative trigger_run_ix {parent_run}"),
        ))
    })?;
    // **`Nested`, and this is the ASCENT the fix round 2 Critical lived on.**
    // This runs from `claim::settle` step 4 of the CHILD, which is itself
    // nested inside the parent's `spawn_one`; the settle its completion runs
    // below nests inside both. `Stack::Fresh` here -- which is what
    // `load_with`'s unconditional `depth: 0` amounted to -- reset the counter
    // once per link of a completing chain. See `claim::Stack`.
    let (ctx, state, _now) =
        claim::load(conn, parent_id, claim::Stack::Nested(caller.clone())).await?;
    let task = state.runs.get(run).map(|r| r.task).ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!("a subflow child names run {run}, which the parent does not hold"),
        ))
    })?;
    if !matches!(ctx.plan.tasks[task].kind, TaskKind::Executable { .. }) {
        return Err(worker_err(WorkerError::Corrupt(
            parent_id,
            format!(
                "run {run} started a subflow child but its task {:?} is not Executable",
                ctx.plan.tasks[task].id
            ),
        )));
    }
    // **Already decided — and "load-bearing" is what this comment used to
    // claim, which W6 Task 4 fix round 4 downgraded to what is actually
    // known.** The shape it is written for: a `Subflow` run whose task carries
    // a `retry:` starts a SECOND child under the same `(execution, run_ix)` —
    // which is why migration 0138's reverse index is deliberately not unique —
    // so the first child's late terminal must not re-terminate a run the
    // second attempt has since moved.
    //
    // **Nothing in this tree observes it, and it was never measured.** Mutating
    // the condition so it can never fire leaves 53 worker tests, 45 executor
    // tests and the oracle ratchet's 294 / 127 / 127 / 0 byte-identical, and
    // the two corpus flows named for the shape — `valids/subflow-parent-retry`
    // and `valids/retry-subflow` — are both baselined, so the ratchet is blind
    // to them by construction. It has also never been established that egret
    // can produce the delivery order the shape needs: `on_child_terminal` runs
    // synchronously inside the child's own settle, so a "late" first terminal
    // has no demonstrated path here.
    //
    // **The `wait: false` half of the older claim was redundant.** That comment
    // also said a run terminated for `wait: false` must not be terminated again
    // by the child finishing later — but the `if !sub.wait` return below sits
    // AFTER this guard, so removing this one changes a `wait: false` run's
    // outcome only where `spec::parse` or `subflow_of` fails on the parent's
    // own source, turning a silent `Ok(())` into an error.
    //
    // **It is also what would drop the report if egret ever followed Kestra on
    // a replayed `Subflow` child**, whose parent run is terminal by the time
    // the replacement fails. Kestra has no equivalent:
    // `Execution.hasTaskRunJoinable` permits terminal→terminal when the history
    // grows. That divergence, and this guard as the only cost that survives
    // it, are registered in `oracle-exec/README.md`'s coverage statement,
    // item 16.
    if state.runs[run]
        .history
        .last()
        .is_some_and(|s| s.is_terminal())
    {
        return Ok(None);
    }

    let flow = crate::spec::parse(&ctx.source)
        .map_err(|e| worker_err(WorkerError::Unplannable(ctx.id, e.message)))?;
    let sub = subflow_of(&flow, &ctx.plan.tasks[task].id).ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!(
                "task {:?} is planned Executable but is not a Subflow of the execution's own source",
                ctx.plan.tasks[task].id
            ),
        ))
    })?;
    // **`wait: false` reports nothing**, and this guard is the whole of that:
    // `spawn_one` has already given the run its SUCCESS and its
    // `{"state": "CREATED"}` output, and without this the child's real terminal
    // would arrive later and overwrite both — turning `valids/subflow-parent-no-wait`
    // (SUCCESS on the artifact) into whatever its child happened to do.
    if !sub.wait {
        return Ok(None);
    }

    // Fact 4: the child's own state first, then `guessState` if it differs.
    let parent_task = &ctx.plan.tasks[task];
    let guessed = guess_state(
        sub.transmit_failed,
        child_state,
        parent_task.allow_failure,
        parent_task.allow_warning,
    );
    let states: Vec<RunState> = if guessed == child_state {
        vec![child_state]
    } else {
        vec![child_state, guessed]
    };
    // See `open_completion` for why the SECOND state is a `Command::CompleteRun`.
    Ok(Some(claim::Job::Complete(Completion {
        execution_id: parent_id,
        run,
        states,
        outputs: Some(json!({
            "executionId": execution_id.to_string(),
            "state": child_state.wire(),
            // The flow-level `outputs:` block is not evaluated anywhere in
            // egret, so this is the empty map rather than an absent key — the
            // same shape `render::Surface` gives `execution.outputs`, and for
            // the same reason: `{{ outputs.sub.outputs }}` renders and
            // `{{ outputs.sub.outputs.x }}` fails.
            "outputs": {},
        })),
        message: None,
        stack: claim::Stack::Nested(caller),
        unless_terminal: false,
    })))
}

/// `ExecutableUtils.guessState` — **whole since W8 Task 5**, except for the
/// trigger set, which is narrowed for a reason that is not about the flags.
///
/// Java's is
/// ```text
/// if (transmitFailed && (isFailed || isPaused || KILLED || WARNING)) {
///     finalState = (allowedFailure && isFailed) ? WARNING : current;
///     return finalState == WARNING && allowWarning ? SUCCESS : finalState;
/// } else return SUCCESS;
/// ```
///
/// The two-line body is [`crate::executor::allow_failure::soften`], which is
/// the same expression written once for all four sites that read the flags.
///
/// **The two inputs this paragraph used to call non-existent now exist.** It
/// read: *"`models::Task` carries no `allowFailure` and no `allowWarning`, so
/// the two `allow*` branches are unreachable, and [`RunState`] has no
/// `Warning`"*. W8 Task 5 added all three, and the two golden entries that cost
/// — `valids/task-allow-{failure,warning}-executable-flow` — bank.
///
/// # The trigger set, and a justification that expired inside the task that
/// wrote it
///
/// Java's disjunction is `isFailed() || isPaused() || KILLED || WARNING`.
/// **Three of the four are here; `PAUSED` is not, and that is a property of the
/// CALLER.** [`on_child_terminal`] runs from `claim::settle` step 4 of a child
/// that has **terminated**, and `State.Type.isTerminated()` does not list
/// `PAUSED` — measured off the pinned artifact at W8 Task 2. So a paused child
/// never reaches this function at all; the disjunct is unreachable rather than
/// declined.
///
/// **`WARNING` was excluded for the same-sounding reason and the reason was
/// wrong by the time it was written.** W8 Task 5 fix round 1 restored it. The
/// paragraph it replaces read: *"a child execution ending `WARNING` needs
/// something inside it to produce one, which for a `Subflow`'s child means the
/// same flags one level down and no corpus flow writes that. Neither is
/// representable as a `child_state` this function can be handed."* Both halves
/// were false **in the commit that wrote them**:
///
/// - *"not representable"* was true until `RunState::Warning` existed, and the
///   same task added it. `worker::from_wire` derives from [`RunState::ALL`], so
///   the variant became decodable in the same commit this sentence claimed it
///   was not.
/// - *"needs the same flags one level down"* is not what it needs. A child
///   execution ends `WARNING` whenever its own aggregate does — one softened
///   failure and nothing worse, which is
///   `executor::tests::an_execution_whose_only_failure_is_softened_ends_warning`
///   driven end to end. The flags are the CHILD's own, at any depth, and the
///   parent's `transmitFailed` is what carries the result up.
///
/// **The lesson is the mechanism, not the instance: a justification can expire
/// silently when the task that expires it is the same task that reads it.** The
/// question that finds it is not a re-read — it is *does this sentence name a
/// thing this commit changes?*
///
/// The fixture missed it for the reason a fixture always does. `the_allow_flags
/// _only_move_a_transmitted_failure` supplied `Failed`, `Killed` and `Success`
/// and **never `Warning`** — the element of the class it was not built for.
/// `a_warning_child_is_transmitted_and_only_allow_warning_moves_it` is the row
/// that was missing.
///
/// **What the artifact answers, read off the bytecode** (`javap -p -c`,
/// `ExecutableUtils.guessState`, offsets 44–47 for the trigger and 79–102 for
/// the tail): with `transmitFailed: true` and a `WARNING` child,
/// `(allowedFailure && isFailed())` is false because `isFailed()` is
/// `this == FAILED` alone, so `finalState` is the child's own `WARNING`; the
/// tail then maps it to `SUCCESS` under `allowWarning:` **and under
/// `allowWarning:` alone**. That is exactly
/// [`soften`](crate::executor::allow_failure::soften)'s `(Warning, _, true)`
/// arm, which is why this function needs no arm of its own for it.
///
/// **Two corrections W8 Task 2 owes this paragraph, both from the decompiled
/// artifact** (`/app/kestra` in `kestra/kestra@sha256:28f59bf1…`):
///
/// - It said `KILLED` came from `State.Type.isFailed()` being
///   `FAILED || KILLED`. It is not: `isFailed()` is `this == FAILED` alone, and
///   `KILLED` is a disjunct `guessState` spells out itself —
///   `execution.getState().getCurrent() == State.Type.KILLED`. The pair is the
///   same one `spawn::terminated_in_error` reads for a `Loop`, but not for the
///   reason given.
/// - It said `RunState` has no paused state. **From W8 Task 2 it does, and from
///   W8 Task 3 something emits it.** The reason moves a second time:
///   `flowable::pause` puts a RUN into [`RunState::Paused`], but this function
///   is reached by a child EXECUTION's terminal, and a paused execution has not
///   terminated. So `PAUSED` stays out because a paused child never gets here
///   at all, not because nothing can be paused.
///
/// The cost WAS two golden entries, named in this module's doc; both bank.
///
/// **`transmitFailed: false` is measured and unreachable from the corpus**:
/// `egret.probe/probe-notransmit-parent` on the artifact gives
/// `["CREATED","RUNNING","FAILED","SUCCESS"]`, and no corpus `Subflow` turns it
/// off (`tests/executor.rs`'s `no_corpus_subflow_turns_transmit_failed_off`).
/// It is implemented anyway, because a differential oracle's job is the
/// behaviour and not the subset of it one corpus happens to reach.
fn guess_state(
    transmit_failed: bool,
    child: RunState,
    allow_failure: bool,
    allow_warning: bool,
) -> RunState {
    match transmit_failed
        && matches!(
            child,
            // Java's disjunction, minus the one disjunct that cannot reach
            // this function: `isFailed() || isPaused() || KILLED || WARNING`.
            // See the doc above for why `PAUSED` is absent and why that is a
            // property of the CALLER rather than a narrowing chosen here.
            RunState::Failed | RunState::Killed | RunState::Warning
        ) {
        // **W8 Task 5 supplies the two `allow*` inputs the paragraph above
        // called unreachable.** They are the `Subflow` TASK's own flags, read
        // off the plan rather than off `models::Subflow` -- `allowFailure:` is
        // a `Task.java` property, not a `Subflow.java` one, which is why
        // `subflow_of` cannot hand them over.
        true => crate::executor::allow_failure::soften(child, allow_failure, allow_warning),
        false => RunState::Success,
    }
}

/// Terminate one `Subflow` run, write what the parent sees of its child, and
/// settle the parent.
///
/// `states` is applied in order — one entry normally, two when
/// [`guess_state`] disagrees with the child (fact 4).
///
/// **The outputs go down before the settle**, for `claim::apply_outcome`'s
/// reason: a successor's `{{ outputs.<this task> }}` has to see them in the
/// same transaction that finished the task.
///
/// **No lease and no fence.** Nothing claimed this run — `persist` writes no
/// `type_id` for a `TaskKind::Executable`, so `claim`'s `type_id IS NOT NULL`
/// predicate cannot take it and there is no token to check. The serialisation
/// is the execution's own `FOR NO KEY UPDATE`, which `claim::load` takes here.
pub(super) struct Completion {
    pub execution_id: Uuid,
    pub run: RunIx,
    pub states: Vec<RunState>,
    pub outputs: Option<Value>,
    pub message: Option<String>,
    pub stack: claim::Stack,
    /// Skip it when the run is already terminal (re-review m1): only the loop
    /// spawner's FILE-input refusal sets it, where an iteration of the same
    /// batch can have ended the loop run first. Every other caller completes
    /// a run it knows is not (or, for a replay's `RETRIED` run, deliberately
    /// moves one that is).
    pub unless_terminal: bool,
}

/// Open a [`Completion`] when the walk reaches it: load the execution, write
/// the outputs and message, apply the states, and hand back the settle --
/// or `None` when `unless_terminal` and the run is already terminal.
pub(super) async fn open_completion(
    conn: &mut PgConnection,
    c: Completion,
) -> Result<Option<claim::Frame<'static>>> {
    let Completion {
        execution_id,
        run,
        states,
        outputs,
        message,
        stack,
        unless_terminal,
    } = c;
    let (ctx, mut state, now) = claim::load(conn, execution_id, stack).await?;
    if unless_terminal
        && state
            .runs
            .get(run)
            .and_then(|r| r.history.last())
            .is_some_and(|s| s.is_terminal())
    {
        return Ok(None);
    }
    let before = state.clone();
    sqlx::query(
        "UPDATE workflow.task_runs SET outputs = $1, message = $2 \
          WHERE execution_id = $3 AND run_ix = $4",
    )
    .bind(&outputs)
    .bind(message.as_deref())
    .bind(execution_id)
    .bind(run as i32)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    // **The first state is a `SetRunState`; every state after it is a
    // `CompleteRun`** -- W8 Task 5. The first one closes the attempt, and
    // Kestra's `ExecutableUtils.subflowExecutionResult` rebuilds the attempt
    // from the parent taskrun's whole `State` afterwards, so a `Subflow`'s
    // attempt is a COPY of its run's history rather than a window onto part of
    // it. Measured: `valids/task-allow-warning-executable-flow`'s run and
    // attempt both read `CREATED RUNNING FAILED SUCCESS`, and so do
    // `egret.probe/probe-notransmit-parent`'s on the pinned artifact.
    //
    // With one state -- which is every call but the two `allow*` `Subflow`s and
    // any `transmitFailed: false` one -- this is byte-identical to what it was.
    for (i, s) in states.iter().enumerate() {
        let cmd = match i {
            0 => Command::SetRunState { run, state: *s },
            _ => Command::CompleteRun {
                run,
                run_state: *s,
                attempt: *s,
            },
        };
        apply(&mut state, &cmd);
    }
    Ok(Some(claim::Frame::loaded(ctx, before, state, now)))
}

/// Fail one `Subflow` run with the reason it could not start: the job that
/// completes it FAILED.
///
/// Separate from [`Completion`] only so that every call site reads as what it
/// is; the message is what `tests/worker.rs` asserts on, because — like
/// `handlers::fail` — nothing in `golden.json` records a failure message.
fn fail_run(ctx: &claim::ExecutionCtx, run: RunIx, message: &str) -> Vec<claim::Job> {
    vec![claim::Job::Complete(Completion {
        execution_id: ctx.id,
        run,
        states: vec![RunState::Failed],
        outputs: None,
        message: Some(message.to_string()),
        // `Nested`: every caller of this is inside `spawn_one`, which is inside
        // its own execution's `settle`.
        stack: claim::Stack::Nested(ctx.caller()),
        unless_terminal: false,
    })]
}

/// `(flow id, source)` of the flow named `(namespace, flow_id)` **in this
/// execution's own workspace**, or `None`.
///
/// # Two decisions, both forced by measurement
///
/// **1. It resolves against the DRAFT row, `workflow.flows.source`, and not
/// through `flow_versions`.**
///
/// **The measurement this paragraph used to lead with has been overturned, and
/// the conclusion has not.** It read: *"only 16 of 345 flows reach a live
/// version … of the 20 distinct literal `Subflow` targets, 2 are live and 17
/// are draft-only — so a resolver reading `flow_versions` would resolve 2 of 20
/// and this package would bank nothing."* That was true while publish refused
/// almost the whole corpus for an unported task type, and W10 Task 6 fix round
/// 1 is what ended it: `spec::registry::TASK_TYPES` now carries the four core
/// FQCNs the worker implements, so **248 of 345** corpus flows reach a live
/// version (was 21), unported-type refusals fell from **296 to 68**, and of the
/// 20 literal `Subflow` targets **18 are live and 1 is draft-only** (was 2 and
/// 17). A `flow_versions` resolver would now reach almost all of them.
///
/// So this is no longer a reason, and it is kept as a **retired** one rather
/// than deleted, because a paragraph that silently drops its evidence reads as
/// though it never had any. The decision is unchanged and now rests on reasons
/// 1 and 3 below alone — which is exactly as it should be: the binding reason
/// was never the corpus's shape. `driver::target_census` asserts every figure
/// above on every run, so this paragraph is still compared by a running
/// program.
///
/// It is also the consistent choice today rather than merely the possible one:
/// `FlowRepo::find_in_workspace`, which is what `publish` resolves a
/// `Subflow` through (since debt §5.1, 2026-10-19; the unscoped
/// `FlowRepo::find_by_name` before), reads the same table and never consults
/// `live_version_id`.
///
/// # The deferral this paragraph used to end on has been settled
///
/// It used to say "**nothing in `crates/*/src` starts an execution from a
/// version snapshot** — there is no product route that starts one at all yet.
/// Whoever adds one has to decide which source an execution runs *and* revisit
/// this in the same change."
///
/// **W6 Task 7 is that route** (`crate::webhook_routes`), and this is the
/// revisit. The decision, taken there and recorded here so the two cannot
/// drift: **a webhook executes the DRAFT row's `source`, exactly as this
/// resolver does.** Three reasons, in order of weight:
///
/// 1. **A parent and its child must not disagree about which version of the
///    world they are in** — the sentence this paragraph already carried, and
///    now the binding one. A webhook-started flow whose task is a `Subflow`
///    would otherwise run a published parent against a draft child.
/// 2. ~~**A version-snapshot resolver would reach almost nothing.**~~
///    **RETIRED at W10 Task 6 fix round 1** — it was measured at *16 of 345*
///    flows reaching a live version (21 by W8 Task 3) because publish refused
///    every `io.kestra.plugin.core.*` leaf egret had not listed in
///    `spec::registry::TASK_TYPES`; that table now carries the four FQCNs the
///    worker implements and **248 of 345** flows reach a live version. Reasons
///    1 and 3 are what carry the decision, and always were. Kept struck through
///    rather than deleted so the next reader knows this argument was made and
///    what happened to it. `driver::target_census` asserts the new figures.
/// 3. Kestra's own `findByIdForExecution` reads the flow repository, not a
///    version archive; its `revision` parameter is a separate opt-in that
///    egret's model has no field for at all.
///
/// **W10 Task 5 added the THIRD site**, `routes::start_flow_execution` — the
/// management API's start button — and it takes the same decision for reason 1
/// again. Note the ordinal moved with the count: this paragraph said "two
/// files" below and an ordinal shares no letters with the number it names, so
/// both had to be corrected or they would disagree.
///
/// **What it costs, stated rather than discovered**: an author editing a draft
/// changes what a live webhook URL — or a start button — runs, with no publish
/// step in between. That is a real operational hazard and it is the reason this
/// is written down in three files. The fix, when someone wants it, is a
/// `flow_versions`-backed execution path — and it has to arrive HERE, in
/// `webhook_routes` and in `routes::start_flow_execution` together, for
/// reason 1.
///
/// **2. It is scoped to `workspace_id`, and `FlowRepo::find_by_name` was not.**
/// (Removed 2026-10-19, debt §5.1; publish now uses the scoped
/// `FlowRepo::find_in_workspace` too.) That method's own doc recorded why it
/// could not be: `WorkflowRefPort::workspace_of`
/// carries no caller-workspace parameter, so two workspaces naming a flow
/// `a.b/nightly` resolve nondeterministically by `id`. **This caller has the
/// workspace** — it is on `ExecutionCtx` — so it uses it, and the alternative
/// is a cross-workspace read of another tenant's flow source through a
/// `(namespace, flowId)` pair an author can type.
///
/// **`workspace_id = $1` is the ONLY thing that stops that, and nothing in the
/// schema is a second line of defence.** This paragraph used to say migration
/// 0138's composite foreign key would refuse the resulting child row; **it
/// would not, and the branch's own mutation proves it.** The child a
/// cross-workspace resolve produces carries the PARENT's `workspace_id` and the
/// OTHER workspace's `flow_id`, so `workflow_executions_trigger_same_workspace`
/// is satisfied, and the only constraint on `flow_id` is
/// `executions_flow_id_fkey FOREIGN KEY (flow_id) REFERENCES
/// workflow.flows(id)` — **one column, no workspace half** (read off
/// `\d workflow.executions`). Deleting the predicate makes
/// `a_subflow_target_in_another_workspace_does_not_resolve` report
/// `(1, Some("SUCCESS"), None)`: one child execution created and run to
/// SUCCESS, no constraint anywhere.
///
/// It is corrected rather than deleted because the false version is the
/// sentence a reader would find when deciding whether this predicate is
/// load-bearing — and this repository has already shipped a cross-workspace
/// read once, on a column whose guard everyone assumed existed.
///
/// **This function closes TWO of the three doors, and the third is not here.**
/// The SQL tests `deleted_at IS NULL` (the trash) and `state <> 'disabled'` (the
/// column). It does **not** test the YAML's own `disabled: true`, and it cannot:
/// that field is inside `source`, not a column, so no predicate can reach it.
/// [`spawn_one`] parses the source and checks it there, with the same refusal
/// message these two produce.
///
/// The sentence here used to read *"a trashed or disabled flow does not
/// resolve"*, which papered over exactly that gap -- "disabled" names two
/// different switches and this function sees one of them. W10 Task 5 fix round 2
/// found the YAML half honoured nowhere on this path at all.
///
/// Kestra makes the pair of refusals one step earlier:
/// `if (flow.isDisabled())` -- which reads the YAML field, `Flow::disabled` at
/// `models/flow.rs` -- and `if (flow instanceof FlowWithException)`
/// (`ExecutableUtils.java:192-205`).
async fn resolve(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    namespace: &str,
    flow_id: &str,
) -> Result<Option<(Uuid, String)>> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, source FROM workflow.flows \
          WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3 \
            AND deleted_at IS NULL AND state <> 'disabled'",
    )
    .bind(workspace_id)
    .bind(namespace)
    .bind(flow_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(row)
}

/// Why a spawn is refused, or `None`.
///
/// An enum rather than an `Option<String>` so the two messages live beside the
/// two decisions and `tests/worker.rs` can assert each without a copy of the
/// wording drifting from the site that produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The target is already somewhere up this execution's own causal chain.
    Cycle,
    /// This execution is already `.0` links down, and one more would exceed
    /// [`MAX_SPAWN_CHAIN`].
    TooDeep(usize),
}

impl Refusal {
    fn message(self, namespace: &str, flow_id: &str) -> String {
        match self {
            Refusal::Cycle => format!(
                "subflow '{namespace}'.'{flow_id}' is already running higher up \
                 this execution's own chain; starting it would loop"
            ),
            Refusal::TooDeep(links) => format!(
                "this execution is {links} child execution(s) down a chain and \
                 {MAX_SPAWN_CHAIN} is as deep as this worker will build in one \
                 transaction; '{namespace}'.'{flow_id}' is not started"
            ),
        }
    }
}

/// Walk this execution's causal chain to the top: is `(namespace, flow_id)`
/// already on it?
///
/// **The cycle question and nothing else.** The depth backstop used to share
/// this walk and share its answer, and mutation M9 measured what that cost: a
/// mutation that broke the walk took the bound with it, so the SIGABRT
/// reproduction aborted the test binary rather than failing an assertion. The
/// bound is `claim::ExecutionCtx::depth` now -- a counter carried down the call
/// stack, which cannot be wrong in the same way a query can.
///
/// # Why a guard exists here at all, measured rather than reasoned
///
/// `spec::validate`'s `self_dependency` refuses a `Subflow` that names its OWN
/// flow — `invalids/recursive-flow` is that flow and Kestra refuses it too —
/// but **neither side refuses A -> B -> A**, and a resolver that looks its
/// target up by name cannot see a cycle from one flow. Kestra would create
/// executions for ever, slowly, one message at a time.
///
/// egret cannot do that: [`spawn_one`] drives the child's first settle **inside
/// its caller's transaction** (the same arrangement `spawn::spawn_one` has for
/// a loop iteration, where a cycle is impossible), so a cycle is unbounded
/// async recursion. **Measured**: a mutually recursive pair overflowed the
/// test thread's stack and aborted the process with SIGABRT.
///
/// # The walk follows BOTH link columns, and the first version did not
///
/// **This is fix round 1's Critical, and the defect was structural rather than
/// an oversight.** The first walk followed `trigger_execution_id` only. A
/// `Loop` sub-execution is linked by `parent_execution_id` and its
/// `trigger_execution_id` is **NULL** — not by accident but by
/// `workflow_executions_one_link_kind`, the constraint W6 Task 2 added to make
/// carrying both unrepresentable. So the walk **truncated at every loop
/// boundary**, and a cycle whose every hop crossed one was invisible to it:
/// two flows, each a one-value `Loop` whose body is a `Subflow` naming the
/// other, still aborted the worker with SIGABRT **with that guard shipped and
/// enabled**.
///
/// The premise was that a causal chain is walkable by one link kind. 0138
/// guarantees the opposite: **a child is linked by exactly one of two columns,
/// and which one depends on why it exists.** So the walk reads
/// `COALESCE(trigger_execution_id, parent_execution_id)` — "whichever link this
/// row has" — and the `COALESCE` is unambiguous *because* of that same CHECK,
/// which is why it is not a choice between two candidates.
///
/// `tests/worker.rs`'s `a_cycle_whose_every_hop_crosses_a_loop_boundary_stops_too`
/// is the reproduction, kept as the test; `a_cycle_of_subflows_stops_instead_of_crashing`
/// is the same-link case, and it is the control that made the diagnosis certain
/// — it was caught by the first guard, so the link kind is what distinguishes
/// them.
///
/// # What it costs, stated as a restriction and not only as a safety net
///
/// Kestra runs `A -> B -> A` for ever; egret refuses at the first repeat. The
/// same rule also refuses a **terminating** mutually recursive workflow — a
/// fan-out over a tree that revisits a flow at a deeper level — which Kestra
/// would run correctly. That is a functional restriction, not only a guard.
/// Nothing in the corpus reaches it (`tests/executor.rs`'s
/// `the_corpus_subflow_graph_has_no_cycle_and_no_deep_chain`), and refusing is
/// the lesser wrong while `spawn_one` settles the child inside its caller's
/// transaction — but a reader deciding whether to relax it should know it costs
/// behaviour and not only safety.
///
/// # What is left, in the words the class deserves
///
/// Between this and [`MAX_SPAWN_CHAIN`] the two shapes a **`Subflow`** can
/// recurse through are refused. **That is not the same as the class being
/// bounded, and fix round 1's text said it was.** Nested `Loop`s reach the
/// same recursion with no `Subflow` anywhere and nothing refuses them —
/// [`MAX_SPAWN_CHAIN`]'s *"What neither bound closes"* carries the measurement
/// and the owner.
///
/// **The thing none of it closes is the reason a bound is needed at all**:
/// `spawn_one` settles the child inside its caller's transaction, so depth in
/// the data is depth on the stack, and every number here is chosen against a
/// stack size this code does not control. The real fix is to stop settling the
/// child there — a change to how `tick` finds work — and it belongs to whoever
/// needs a deep chain.
///
/// # What it compares, where it starts, and why it terminates
///
/// It reads `flow_key` / `namespace` — the identity pair a `Subflow` names —
/// rather than `flow_id`, the surrogate key, because two workspaces' rows are
/// different `id`s for the same name and the name is what an author wrote.
///
/// **It starts at this execution itself**, so `A -> A` is caught here as well
/// as by `spec::validate`'s `self_dependency` — two guards, and the second is
/// the one that holds for a flow started from a source that never went through
/// publish, which is what the oracle's driver does.
///
/// The walk terminates whatever the data says: a chain of rows can only run
/// backwards in time, because a child of either kind always names a row that
/// existed before it was inserted. That argument is unchanged by widening the
/// walk, because `parent_execution_id` names an older row for the same reason
/// `trigger_execution_id` does.
async fn chain_holds(
    conn: &mut PgConnection,
    execution_id: Uuid,
    namespace: &str,
    flow_id: &str,
) -> Result<bool> {
    let hit: bool = sqlx::query_scalar(
        "WITH RECURSIVE up AS ( \
             SELECT id, COALESCE(trigger_execution_id, parent_execution_id) AS caused_by, \
                    namespace, flow_key \
               FROM workflow.executions WHERE id = $1 \
           UNION ALL \
             SELECT e.id, COALESCE(e.trigger_execution_id, e.parent_execution_id), \
                    e.namespace, e.flow_key \
               FROM workflow.executions e JOIN up ON e.id = up.caused_by \
         ) \
         SELECT EXISTS ( \
             SELECT 1 FROM up WHERE namespace = $2 AND flow_key = $3 \
         )",
    )
    .bind(execution_id)
    .bind(namespace)
    .bind(flow_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(hit)
}

/// The execution that started this one and the run inside it, or `None` when
/// nothing did.
///
/// `trigger_run_ix` is NULL for a `Flow`-trigger-fired execution (W6 Task 6) —
/// which is caused by an execution reaching a state and by no particular
/// taskrun — so the `zip` is what separates the two kinds of trigger link, and
/// a flow-triggered execution correctly reports back to nobody.
async fn trigger_of(conn: &mut PgConnection, execution_id: Uuid) -> Result<Option<(Uuid, i32)>> {
    let row = sqlx::query(
        "SELECT trigger_execution_id, trigger_run_ix FROM workflow.executions WHERE id = $1",
    )
    .bind(execution_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("execution {execution_id}")))?;
    Ok(row
        .get::<Option<Uuid>, _>("trigger_execution_id")
        .zip(row.get::<Option<i32>, _>("trigger_run_ix")))
}

/// The `Subflow` a task id names, from anywhere in the flow.
///
/// # It walks `error_children()` and `spawn::find_task` does not
///
/// **The gap is real and was recorded before this task inherited it.** Neither
/// `spec::validate` (`validate.rs:1283`) nor `publish::resolve_task` recurses
/// through a task's own `errors:` — both walk `children()` only — so a
/// `Subflow` nested inside a flowable's `errors:` never has its reference
/// resolved at publish. `driver::subflow_targets` took the wider walk on
/// purpose and measured that the extra arm is redundant *on today's corpus*.
///
/// **It is NOT redundant here**, and that was measured on the artifact rather
/// than reasoned: `egret.probe/probe-errors-subflow` puts a `Subflow` in a
/// `Sequential`'s `errors:` list, and Kestra ran it — the `handler` run reads
/// `CREATED RUNNING SUCCESS` and a child execution was created. `build_plan`
/// flattens every list's `errors:` too, so the plan reaches such a task and
/// this module is asked to spawn it; a walk that stopped at `children()` would
/// answer `None` and report a corruption for a flow that is merely unusual.
/// `claim::leaf` had exactly this bug and exactly this fix.
///
/// **End hooks A4 fix round (2026-10-27):** all five worker walks, this one
/// included, now go through `spec::path::find_task`, so `spawn::find_task`
/// reaches `error_children()` too and the heading above is history.
///
/// **The publish-time half is closed too (corrected 2026-10-27).** This
/// paragraph used to say `publish::resolve_task` could not see such a
/// `Subflow`; that stopped being true at debt PR-2 (#698), whose resolve walk
/// visits `children_mut()` and then `error_children_mut()`, so a `Subflow` in
/// any `errors:` (and, since #2a, any `finally:`) is resolved by the same
/// `resolve_task` as one in `tasks:` -- its `flow_ref` filled, its target
/// checked the same way. `tests/publish.rs::
/// a_subflow_in_any_new_list_is_resolved_like_one_in_tasks` pins the walk.
fn subflow_of<'a>(flow: &'a Flow, want: &str) -> Option<&'a Subflow> {
    // End hooks A4 fix round (2026-10-27): the shared walk, which also reaches
    // `afterExecution:` (#2b).
    match &crate::spec::path::find_task(flow, want)?.kind {
        ModelTaskKind::Subflow(s) => Some(s),
        _ => None,
    }
}

/// One of `namespace`/`flowId`, rendered.
fn render_name(
    raw: Option<&str>,
    cx: &RenderContext,
) -> std::result::Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(s) => render_str(s, cx).map(Some).map_err(|e| e.to_string()),
    }
}

/// The `Subflow`'s `inputs:` map, with every string rendered.
///
/// Recursive over arrays and objects, because Kestra's
/// `runContext.render(Map<String, Object>)` is: a value nested inside a list an
/// author wrote is rendered too. Non-strings pass through — there is nothing in
/// a number to render.
fn rendered_inputs(sub: &Subflow, cx: &RenderContext) -> std::result::Result<Value, String> {
    fn render_value(v: &Value, cx: &RenderContext) -> std::result::Result<Value, String> {
        Ok(match v {
            Value::String(s) => Value::String(render_str(s, cx).map_err(|e| e.to_string())?),
            Value::Array(a) => Value::Array(
                a.iter()
                    .map(|x| render_value(x, cx))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            ),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| render_value(x, cx).map(|r| (k.clone(), r)))
                    .collect::<std::result::Result<serde_json::Map<_, _>, _>>()?,
            ),
            other => other.clone(),
        })
    }
    let Some(inputs) = &sub.inputs else {
        return Ok(json!({}));
    };
    let mut out = serde_json::Map::new();
    for (k, v) in inputs {
        out.insert(k.clone(), render_value(v, cx)?);
    }
    Ok(Value::Object(out))
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

    /// [`guess_state`]'s whole table, against the two measurements that pin it.
    ///
    /// The `transmitFailed: true` rows are the golden's
    /// (`valids/task-flow` FAILED, `valids/subflow-parent` SUCCESS); the
    /// `false` row is `egret.probe/probe-notransmit-parent` on the pinned
    /// artifact, whose parent run read `CREATED RUNNING FAILED SUCCESS`.
    #[test]
    fn guess_state_is_the_transmit_failed_half_of_the_artifacts_table() {
        // The rows this table has always had, with both flags OFF -- which is
        // what every `Subflow` in the corpus but two writes.
        assert_eq!(
            guess_state(true, RunState::Failed, false, false),
            RunState::Failed
        );
        assert_eq!(
            guess_state(true, RunState::Killed, false, false),
            RunState::Killed
        );
        assert_eq!(
            guess_state(true, RunState::Success, false, false),
            RunState::Success
        );
        assert_eq!(
            guess_state(false, RunState::Failed, false, false),
            RunState::Success
        );
        assert_eq!(
            guess_state(false, RunState::Killed, false, false),
            RunState::Success
        );
        // **Non-trigger TERMINALS under `transmitFailed: true`** -- the rows
        // that make WIDENING the trigger set fail rather than pass. Both are
        // `isTerminatedNoFail()`'s and neither is in Java's disjunction, so
        // both take the `else` branch and answer `SUCCESS`; adding either to
        // the `matches!` would make `soften` return the state itself instead.
        //
        // `Skipped` was here already and `Retried` was **not** -- W8 Task 5 fix
        // round 2 measured the difference: a mutation adding `| RunState::Skipped`
        // to the trigger set dies on the line below, and one adding
        // `| RunState::Retried` survived the whole suite until this row existed.
        // One member of a class is not the class.
        assert_eq!(
            guess_state(true, RunState::Skipped, false, false),
            RunState::Success
        );
        assert_eq!(
            guess_state(true, RunState::Retried, false, false),
            RunState::Success
        );
        // And with the flags ON, because `soften` is what the trigger arm
        // calls: a widened set would return `Retried` here too, and a fixture
        // that only ever passed `false, false` could not tell a widening from a
        // `soften` that had stopped being identity on a non-`FAILED` state.
        assert_eq!(
            guess_state(true, RunState::Retried, true, true),
            RunState::Success
        );
    }

    /// **A `PAUSED` child cannot reach [`guess_state`], and that is carried by
    /// code rather than by prose** — W8 Task 5 fix round 2.
    ///
    /// Java's trigger set is `isFailed() || isPaused() || KILLED || WARNING`
    /// and this function omits `isPaused()`. The omission is sound because
    /// `claim::settle` calls [`on_child_terminal`] behind
    /// `is.filter(|s| s.is_terminal())`, so the only states this function can
    /// be HANDED are `RunState::ALL`'s terminals — and `PAUSED` is not one.
    /// That was true and unpinned: nothing failed if `is_terminal` grew a
    /// `Paused`, and the doc would then have been quietly wrong in the same way
    /// the `WARNING` justification was.
    ///
    /// **Derived from `RunState::ALL` rather than listed**, and with a positive
    /// control: a `!contains` alone passes on an implementation where nothing
    /// is reachable at all, which is the acquitting direction and the only one
    /// that matters.
    #[test]
    fn a_paused_child_cannot_reach_guess_state() {
        // **Through `claim::notifies_parent`, which is the predicate
        // `claim::settle` actually filters on** -- not through `is_terminal`
        // directly. Fix round 3: deriving from `is_terminal` made this test
        // true of the ENUM and silent about the CALL SITE, so the link it
        // exists to pin was still carried by prose alone.
        let reachable: Vec<RunState> = RunState::ALL
            .iter()
            .copied()
            .filter(|s| crate::worker::claim::notifies_parent(*s))
            .collect();
        assert!(
            !reachable.contains(&RunState::Paused),
            "a PAUSED child execution has not terminated, so `claim::settle`'s \
             `is.filter(|s| s.is_terminal())` never hands it here -- and this \
             is what lets `guess_state` omit Java's `isPaused()` disjunct"
        );
        for state in [RunState::Failed, RunState::Killed, RunState::Warning] {
            assert!(
                reachable.contains(&state),
                "{state:?} IS in the trigger set and must be reachable -- \
                 without this the assertion above passes on an `is_terminal` \
                 that answers false for everything"
            );
        }
        // **`Cancelled` is reachable here and is deliberately NOT in
        // `guess_state`'s trigger set** -- W8 Task 6. It is terminal, so
        // `claim::settle` really does hand it to `on_child_terminal`, and
        // `ExecutableUtils.guessState`'s disjunction is `isFailed() ||
        // isPaused() || KILLED || WARNING` (`ExecutableUtils.java:46-47`) with
        // no `CANCELLED` in it -- so a cancelled child gives the parent's run
        // `SUCCESS`. That is a row the corpus cannot supply and
        // `a_cancelled_child_gives_the_parent_run_success` supplies it.
        assert!(
            reachable.contains(&RunState::Cancelled),
            "unlike `Paused`, a `Cancelled` child DOES reach `guess_state`; \
             the omission of `CANCELLED` from the trigger set is Java's, not a \
             property of what can arrive"
        );
        assert_eq!(
            reachable.len(),
            7,
            "seven of the thirteen `RunState::ALL` are terminal; a bare \
             `!contains` says nothing about how many"
        );
    }

    /// **A `CANCELLED` child gives the parent's `Subflow` run `SUCCESS`** —
    /// W8 Task 6, and it is Java's omission rather than egret's.
    ///
    /// `ExecutableUtils.guessState`'s disjunction is `isFailed() || isPaused()
    /// || KILLED || WARNING` (`ExecutableUtils.java:46-47`). `isFailed()` is
    /// `FAILED` alone, and **`CANCELLED` is in none of the four** — so a child
    /// execution that an `sla:` cancelled is not transmitted at all and the
    /// `else` branch answers `SUCCESS`, whatever `transmitFailed:` says and
    /// whatever the `allow*` flags say.
    ///
    /// **This row is reachable in production and not from this corpus**, which
    /// is why it is asserted rather than argued: `RunState::Cancelled` has one
    /// producer (a flow-level `EXECUTION_ASSERTION` with `behavior: CANCEL`)
    /// and the two corpus flows that carry one are named as a `Subflow` target
    /// by nothing. `a_paused_child_cannot_reach_guess_state` is the companion
    /// that pins the OTHER half — that a `CANCELLED` child really does arrive
    /// here, unlike a `PAUSED` one.
    ///
    /// Every combination of the three flags is driven, because the claim is
    /// that none of them moves the answer; a single row could not say that.
    #[test]
    fn a_cancelled_child_gives_the_parent_run_success() {
        for transmit in [true, false] {
            for af in [true, false] {
                for aw in [true, false] {
                    assert_eq!(
                        guess_state(transmit, RunState::Cancelled, af, aw),
                        RunState::Success,
                        "CANCELLED is outside `guessState`'s trigger set, so \
                         transmitFailed={transmit} allowFailure={af} \
                         allowWarning={aw} cannot change the answer"
                    );
                }
            }
        }
        // The control on the same fixture: a state that IS in the trigger set
        // is transmitted, so "SUCCESS for everything" fails here.
        assert_eq!(
            guess_state(true, RunState::Killed, false, false),
            RunState::Killed
        );
    }

    /// The `allow*` columns W8 Task 5 added, and the rows the corpus's two
    /// declaring flows reach.
    ///
    /// **`transmitFailed` is varied against the SAME flag pair**, because the
    /// two are read by different halves of Java's expression and a fixture that
    /// only ever turned one knob could not tell "softened" from "never
    /// triggered": both answer `SUCCESS` for `(Failed, true, true)`.
    #[test]
    fn the_allow_flags_only_move_a_transmitted_failure() {
        // `valids/task-allow-failure-executable-flow` is `(true, Failed, true,
        // false)`. This row read `Failed` at Step 2, where `RunState` had no
        // `Warning`; Step 3 added the variant and this is the artifact's
        // answer.
        assert_eq!(
            guess_state(true, RunState::Failed, true, false),
            RunState::Warning
        );
        // `valids/task-allow-warning-executable-flow`.
        assert_eq!(
            guess_state(true, RunState::Failed, true, true),
            RunState::Success
        );
        // `allowWarning:` alone does nothing to a FAILED child, at this site as
        // at every other.
        assert_eq!(
            guess_state(true, RunState::Failed, false, true),
            RunState::Failed
        );
        // A `KILLED` child is in the trigger set but is not `isFailed()`, so
        // Java's `(allowedFailure && isFailed())` is false and the flags cannot
        // move it. This is the row that separates "soften a failure" from
        // "soften anything the trigger set admits".
        assert_eq!(
            guess_state(true, RunState::Killed, true, true),
            RunState::Killed
        );
        // With `transmitFailed` off the `else` branch answers before the flags
        // are consulted at all -- the same answer by a different route, which
        // is why the row above it exists.
        assert_eq!(
            guess_state(false, RunState::Failed, true, true),
            RunState::Success
        );
    }

    /// **A `WARNING` child is TRANSMITTED, and only `allowWarning:` moves it**
    /// — the row `the_allow_flags_only_move_a_transmitted_failure` did not
    /// contain, and the one a shipped defect lived in until W8 Task 5 fix
    /// round 1.
    ///
    /// That test supplies `Failed`, `Killed` and `Success` and never `Warning`,
    /// so it could not see [`guess_state`] answering `SUCCESS` where the
    /// artifact keeps `WARNING`. The trigger set is
    /// `isFailed() || isPaused() || KILLED || WARNING` (bytecode 44–47) and the
    /// tail is `finalState == WARNING && allowWarning ? SUCCESS : finalState`
    /// (79–102).
    ///
    /// **The oracle cannot score any of this**, so these four assertions are
    /// the whole instrument: no corpus `Subflow` has a child that ends
    /// `WARNING`, because that needs the child's own flags and the two flows
    /// that write them are the parents rather than the children. A mutation
    /// reverting the `RunState::Warning` disjunct leaves the ratchet at 111 and
    /// kills **only this test** — measured, fix round 1's M18.
    ///
    /// **`allow_failure` is varied against a fixed `allow_warning` in both
    /// halves**, because `(allowedFailure && isFailed())` is false for a
    /// `WARNING` child whatever `allowFailure:` says — a mapping that read the
    /// two flags as one would pass a fixture that only moved `allow_warning`.
    #[test]
    fn a_warning_child_is_transmitted_and_only_allow_warning_moves_it() {
        // Transmitted and kept: `allowFailure:` cannot reach it, because
        // `State.Type.isFailed()` is `this == FAILED` alone.
        assert_eq!(
            guess_state(true, RunState::Warning, false, false),
            RunState::Warning
        );
        assert_eq!(
            guess_state(true, RunState::Warning, true, false),
            RunState::Warning
        );
        // `allowWarning:` moves it, with `allowFailure:` on or off.
        assert_eq!(
            guess_state(true, RunState::Warning, false, true),
            RunState::Success
        );
        assert_eq!(
            guess_state(true, RunState::Warning, true, true),
            RunState::Success
        );
        // The control: with `transmitFailed` off the `else` branch answers
        // `SUCCESS` for the same child, so "SUCCESS" above would have been
        // indistinguishable from "never triggered" without this row beside it.
        assert_eq!(
            guess_state(false, RunState::Warning, false, false),
            RunState::Success
        );
    }

    fn flow(src: &str) -> Flow {
        crate::spec::parse(src).expect("the probe flow parses")
    }

    /// The walk reaches a `Subflow` inside a flowable's `errors:`, which is the
    /// shape `spawn::find_task` and `publish::resolve_task` both miss.
    ///
    /// Measured reachable on the artifact — see [`subflow_of`].
    #[test]
    fn the_walk_reaches_a_subflow_nested_in_an_errors_list() {
        let f = flow(
            r#"
id: probe-errors-subflow
namespace: egret.probe
tasks:
  - id: seq
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: boom
        type: io.kestra.plugin.core.execution.Fail
    errors:
      - id: handler
        type: io.kestra.plugin.core.flow.Subflow
        namespace: egret.probe
        flowId: target
"#,
        );
        let sub = subflow_of(&f, "handler").expect("the errors-nested Subflow is reachable");
        assert_eq!(sub.namespace.as_deref(), Some("egret.probe"));
        assert_eq!(sub.flow_id.as_deref(), Some("target"));
        assert!(subflow_of(&f, "boom").is_none(), "a Leaf is not a Subflow");
    }

    /// A `Subflow`'s `inputs:` are rendered, and nested strings with them.
    #[test]
    fn inputs_are_rendered_including_nested_strings() {
        let f = flow(
            r#"
id: p
namespace: n
tasks:
  - id: s
    type: io.kestra.plugin.core.flow.Subflow
    namespace: n
    flowId: t
    inputs:
      plain: "{{ inputs.x }}"
      nested: ["{{ inputs.x }}", 7]
      literal: 3
"#,
        );
        let sub = subflow_of(&f, "s").unwrap();
        let mut cx = RenderContext::new();
        cx.insert("inputs", json!({"x": "hello"}));
        assert_eq!(
            rendered_inputs(sub, &cx).unwrap(),
            json!({"plain": "hello", "nested": ["hello", 7], "literal": 3})
        );
    }

    /// No `inputs:` is the empty object, not a null — `start_execution` stores
    /// `{}` for the same reason and `Surface::load` then applies the child
    /// flow's own declared defaults on top.
    #[test]
    fn a_subflow_with_no_inputs_passes_an_empty_object() {
        let f = flow(
            r#"
id: p
namespace: n
tasks:
  - id: s
    type: io.kestra.plugin.core.flow.Subflow
    namespace: n
    flowId: t
"#,
        );
        let sub = subflow_of(&f, "s").unwrap();
        assert_eq!(
            rendered_inputs(sub, &RenderContext::new()).unwrap(),
            json!({})
        );
    }
}
