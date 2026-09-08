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
//! * **`allowFailure`/`allowWarning` are unrepresentable** — `models::Task`
//!   carries neither and `RunState` has no `Warning` — so [`guess_state`]
//!   implements only the `transmitFailed` half of
//!   `ExecutableUtils.guessState`. `valids/task-allow-failure-executable-flow`
//!   and `valids/task-allow-warning-executable-flow` are the two entries that
//!   costs, and both stay baselined.
//! * **`revision`, `labels`, `inheritLabels`, `scheduleDate` and
//!   `restartBehavior` are not on `models::Subflow`** (W1's curation), so
//!   nothing here can honour them. `valids/task-flow-inherited-labels` and
//!   `valids/subflow-inherited-labels-parent` are the entries that names.

use egret_core::{Error, Result};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::render::Surface;
use super::{claim, WorkerError};
use crate::executor::{
    apply, build_plan, Command, ExecutionState, RunIx, RunState, TaskIx, TaskKind, Tick,
};
use crate::models::{Flow, Subflow, Task, TaskKind as ModelTaskKind};
use crate::runners::{render_str, RenderContext};

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

/// Execute every subflow spawn one round of `advance()` asked for.
///
/// The whole of it — the child row, its task runs, and whatever the child's own
/// first round asks for — is on the caller's connection and therefore in the
/// caller's transaction, for [`super::spawn::spawn_all`]'s reason: a child that
/// existed while its parent's run did not know it had been started is the
/// window the plan's Global Constraint forbids.
pub(super) async fn spawn_all(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    surface: &Surface,
    state: &ExecutionState,
    requested: &[Requested],
) -> Result<()> {
    for r in requested {
        Box::pin(spawn_one(conn, ctx, surface, state, r)).await?;
    }
    Ok(())
}

/// Resolve one `Subflow`'s target, create its execution, and drive it until it
/// is waiting on a worker.
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
/// re-derives that over all 345 flows rather than remembering it.
async fn spawn_one(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    surface: &Surface,
    state: &ExecutionState,
    r: &Requested,
) -> Result<()> {
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
            return fail_run(conn, ctx, r.run, &format!("subflow target: {e}")).await;
        }
        // `@NotEmpty`/`@NotNull` in Java, `Option` here because bean validation
        // is not parsing (`models::Subflow`). `spec::validate` refuses both, so
        // a flow reaching here without them has been started from a source that
        // never went through publish — which the oracle's driver does.
        _ => {
            return fail_run(
                conn,
                ctx,
                r.run,
                "subflow task has no namespace/flowId to resolve",
            )
            .await;
        }
    };

    let Some((child_flow, source)) = resolve(conn, ctx.workspace_id, &namespace, &flow_id).await?
    else {
        // The artifact's own message, kept close to it:
        // `"Unable to find flow '" + ns + "'.'" + id + "' with revision '0'"`.
        // egret has no revision, so the clause is dropped rather than faked.
        return fail_run(
            conn,
            ctx,
            r.run,
            &format!("unable to find flow '{namespace}'.'{flow_id}'"),
        )
        .await;
    };

    let child = match crate::spec::parse(&source) {
        Ok(f) => f,
        Err(e) => {
            // `Cannot execute an invalid flow` — `ExecutableUtils.java:200`.
            return fail_run(
                conn,
                ctx,
                r.run,
                &format!(
                    "cannot execute an invalid flow '{namespace}'.'{flow_id}': {}",
                    e.message
                ),
            )
            .await;
        }
    };
    let plan = match build_plan(&source) {
        Ok(p) => p,
        Err(e) => {
            return fail_run(
                conn,
                ctx,
                r.run,
                &format!("cannot execute an invalid flow '{namespace}'.'{flow_id}': {e}"),
            )
            .await;
        }
    };

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
            return fail_run(conn, ctx, r.run, &format!("subflow inputs: {e}")).await;
        }
    };

    let child_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, \
            trigger_execution_id, trigger_run_ix, inputs) \
         VALUES ($1,$2,$3,$4,$5,'CREATED','[\"CREATED\"]'::jsonb,$6,$7,$8) \
         RETURNING id",
    )
    .bind(ctx.workspace_id)
    .bind(child_flow)
    .bind(&child.namespace)
    .bind(&child.id)
    .bind(&source)
    .bind(ctx.id)
    .bind(r.run as i32)
    .bind(&inputs)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;

    let child_ctx = claim::ExecutionCtx {
        id: child_id,
        workspace_id: ctx.workspace_id,
        flow_id: Some(child_flow),
        source,
        plan,
        // Its own logical clock, for `spawn::spawn_one`'s reason: the
        // happens-before relation is compared PER EXECUTION.
        seq: 0,
    };
    let empty = ExecutionState::new(&child_ctx.plan);
    let mut child_state = empty.clone();
    // Boxed: a subflow may call a subflow (`valids/subflow-parent` ->
    // `subflow-child` -> `subflow-grand-child`), so `settle` -> here ->
    // `settle` is a real cycle in the type graph. It is not a cycle in the
    // data unless the AUTHOR wrote one — `invalids/recursive-flow` is that
    // flow and `spec::validate`'s `self_dependency` refuses it — and a chain
    // that is deep but finite terminates because each level is a different
    // execution row.
    Box::pin(claim::settle(
        conn,
        &child_ctx,
        &empty,
        &mut child_state,
        Tick(0),
        None,
    ))
    .await?;

    // `wait: false`: the parent does not wait, and what it sees is the child's
    // state at THIS instant rather than its terminal — measured, fact 3. The
    // run is already RUNNING (the `StartRun` beside the spawn), so this is the
    // second half of `CREATED RUNNING SUCCESS`, which is exactly what
    // `valids/subflow-parent-no-wait`'s golden holds.
    //
    // It has to be read from the row rather than from `child_state`, because
    // the settle above may have terminated the child already and Kestra's
    // answer is the state the child had when the parent stopped caring.
    if !sub.wait {
        return complete(
            conn,
            ctx.id,
            r.run,
            &[RunState::Success],
            Some(json!({"executionId": child_id.to_string(), "state": "CREATED"})),
            None,
        )
        .await;
    }
    Ok(())
}

/// One subflow child has reached a terminal state: tell the run that started it.
///
/// The half of `Subflow` that `advance()` cannot own, for [`super::spawn::on_execution_terminal`]'s
/// reason: it is pure over `(plan, state, now)` and [`ExecutionState`] holds
/// nothing about another execution.
///
/// `Ok(())` and nothing else for an execution that is not a subflow child,
/// which is almost all of them.
pub(super) async fn on_child_terminal(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
) -> Result<()> {
    Box::pin(on_child_terminal_inner(conn, execution_id, child_state)).await
}

async fn on_child_terminal_inner(
    conn: &mut PgConnection,
    execution_id: Uuid,
    child_state: RunState,
) -> Result<()> {
    let Some((parent_id, parent_run)) = trigger_of(conn, execution_id).await? else {
        return Ok(());
    };
    let run = usize::try_from(parent_run).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            parent_id,
            format!("negative trigger_run_ix {parent_run}"),
        ))
    })?;
    let (ctx, state, _now) = claim::load(conn, parent_id).await?;
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
    // **Already decided, and both halves of that are load-bearing.** A
    // `Subflow` run whose task carries a `retry:` starts a SECOND child under
    // the same `(execution, run_ix)` — which is why migration 0138's reverse
    // index is deliberately not unique — so the first child's late terminal
    // must not re-terminate a run the second attempt has since moved. And a
    // run this module has already terminated for `wait: false` must not be
    // terminated again by the child finishing later.
    if state.runs[run]
        .history
        .last()
        .is_some_and(|s| s.is_terminal())
    {
        return Ok(());
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
        return Ok(());
    }

    // Fact 4: the child's own state first, then `guessState` if it differs.
    let guessed = guess_state(sub.transmit_failed, child_state);
    let states: Vec<RunState> = if guessed == child_state {
        vec![child_state]
    } else {
        vec![child_state, guessed]
    };
    complete(
        conn,
        parent_id,
        run,
        &states,
        Some(json!({
            "executionId": execution_id.to_string(),
            "state": child_state.wire(),
            // The flow-level `outputs:` block is not evaluated anywhere in
            // egret, so this is the empty map rather than an absent key — the
            // same shape `render::Surface` gives `execution.outputs`, and for
            // the same reason: `{{ outputs.sub.outputs }}` renders and
            // `{{ outputs.sub.outputs.x }}` fails.
            "outputs": {},
        })),
        None,
    )
    .await
}

/// `ExecutableUtils.guessState`, restricted to the half egret can represent.
///
/// Java's is
/// ```text
/// if (transmitFailed && (isFailed || isPaused || KILLED || WARNING)) {
///     finalState = (allowedFailure && isFailed) ? WARNING : current;
///     return finalState == WARNING && allowWarning ? SUCCESS : finalState;
/// } else return SUCCESS;
/// ```
///
/// **Three of its inputs do not exist here and that is stated rather than
/// approximated**: `models::Task` carries no `allowFailure` and no
/// `allowWarning`, and [`RunState`] has neither `Warning` nor a paused state.
/// So the two `allow*` branches are unreachable and the trigger set narrows
/// from `{FAILED, PAUSED, KILLED, WARNING}` to `{Failed, Killed}` —
/// `State.Type.isFailed()` is `FAILED || KILLED` in Java, which is where the
/// second of those comes from, and it is the same pair
/// `spawn::terminated_in_error` reads for a `Loop`.
///
/// The cost is two golden entries, named in this module's doc.
///
/// **`transmitFailed: false` is measured and unreachable from the corpus**:
/// `egret.probe/probe-notransmit-parent` on the artifact gives
/// `["CREATED","RUNNING","FAILED","SUCCESS"]`, and no corpus `Subflow` turns it
/// off (`tests/executor.rs`'s `no_corpus_subflow_turns_transmit_failed_off`).
/// It is implemented anyway, because a differential oracle's job is the
/// behaviour and not the subset of it one corpus happens to reach.
fn guess_state(transmit_failed: bool, child: RunState) -> RunState {
    match transmit_failed && matches!(child, RunState::Failed | RunState::Killed) {
        true => child,
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
/// is the execution's own `FOR UPDATE`, which `claim::load` takes here.
async fn complete(
    conn: &mut PgConnection,
    execution_id: Uuid,
    run: RunIx,
    states: &[RunState],
    outputs: Option<Value>,
    message: Option<&str>,
) -> Result<()> {
    let (ctx, mut state, now) = claim::load(conn, execution_id).await?;
    let before = state.clone();
    sqlx::query(
        "UPDATE workflow.task_runs SET outputs = $1, message = $2 \
          WHERE execution_id = $3 AND run_ix = $4",
    )
    .bind(&outputs)
    .bind(message)
    .bind(execution_id)
    .bind(run as i32)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    for s in states {
        apply(&mut state, &Command::SetRunState { run, state: *s });
    }
    claim::settle(conn, &ctx, &before, &mut state, now, None).await
}

/// Fail one `Subflow` run with the reason it could not start.
///
/// Separate from [`complete`] only so that every call site reads as what it is;
/// the message is what `tests/worker.rs` asserts on, because — like
/// `handlers::fail` — nothing in `golden.json` records a failure message.
async fn fail_run(
    conn: &mut PgConnection,
    ctx: &claim::ExecutionCtx,
    run: RunIx,
    message: &str,
) -> Result<()> {
    complete(conn, ctx.id, run, &[RunState::Failed], None, Some(message)).await
}

/// `(flow id, source)` of the flow named `(namespace, flow_id)` **in this
/// execution's own workspace**, or `None`.
///
/// # Two decisions, both forced by measurement
///
/// **1. It resolves against the DRAFT row, `workflow.flows.source`, and not
/// through `flow_versions`.** W6 Task 1 filed the whole corpus through the
/// product's own create-then-publish path and measured the result: only **16 of
/// 345** flows reach a live version, because publish step 4 asks
/// `registry::known_type` about `io.kestra.plugin.core.*` leaves egret has not
/// ported. Of the **20 distinct literal `Subflow` targets**, **2** are live and
/// **17** are draft-only — so a resolver reading `flow_versions` would resolve
/// 2 of 20 and this package would bank nothing. `driver::target_census`
/// asserts those figures on every run, so this paragraph is compared by a
/// running program.
///
/// It is also the consistent choice today rather than merely the possible one:
/// `FlowRepo::find_by_name`, which is what `publish` resolves a
/// `RefKind::Subflow` through, reads the same table and never consults
/// `live_version_id`, and **nothing in `crates/*/src` starts an execution from
/// a version snapshot** — there is no product route that starts one at all yet.
/// Whoever adds one has to decide which source an execution runs *and* revisit
/// this in the same change, so that a parent and its child cannot disagree
/// about which version of the world they are in.
///
/// **2. It is scoped to `workspace_id`, and `FlowRepo::find_by_name` is not.**
/// That method's own doc records why it cannot be: `WorkflowRefPort::workspace_of`
/// carries no caller-workspace parameter, so two workspaces naming a flow
/// `a.b/nightly` resolve nondeterministically by `id`. **This caller has the
/// workspace** — it is on `ExecutionCtx` — so it uses it, and the alternative
/// is a cross-workspace read of another tenant's flow source through a
/// `(namespace, flowId)` pair an author can type. Migration 0138's composite
/// foreign key would refuse the resulting child row, but only after the source
/// had been read, which is the wrong order to find out.
///
/// A trashed or disabled flow does not resolve, which is Kestra's own pair of
/// refusals one step earlier: `if (flow.isDisabled())` and
/// `if (flow instanceof FlowWithException)` (`ExecutableUtils.java:192-205`).
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
/// **What this does NOT close is the publish-time half**, which is not this
/// module's seam: `publish::resolve_task` still cannot see such a `Subflow`, so
/// its `flow_ref` is never filled and a cross-workspace target inside an
/// `errors:` is never refused. Named here so the next reader finds it beside
/// the walk that goes wider, rather than in a report.
fn subflow_of<'a>(flow: &'a Flow, want: &str) -> Option<&'a Subflow> {
    fn walk<'a>(task: &'a Task, want: &str) -> Option<&'a Subflow> {
        if task.id == want {
            return match &task.kind {
                ModelTaskKind::Subflow(s) => Some(s),
                _ => None,
            };
        }
        task.children()
            .into_iter()
            .chain(task.error_children())
            .find_map(|c| walk(c, want))
    }
    flow.tasks
        .iter()
        .chain(flow.errors.iter())
        .chain(flow.finally_.iter())
        .find_map(|t| walk(t, want))
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

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
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
        assert_eq!(guess_state(true, RunState::Failed), RunState::Failed);
        assert_eq!(guess_state(true, RunState::Killed), RunState::Killed);
        assert_eq!(guess_state(true, RunState::Success), RunState::Success);
        assert_eq!(guess_state(false, RunState::Failed), RunState::Success);
        assert_eq!(guess_state(false, RunState::Killed), RunState::Success);
        // Not in Java's trigger set at all, so it is the `else` branch even
        // with `transmitFailed` on.
        assert_eq!(guess_state(true, RunState::Skipped), RunState::Success);
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
