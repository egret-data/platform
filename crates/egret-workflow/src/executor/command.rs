//! What [`advance`](super::advance) asks the caller to do. The caller does the
//! I/O; nothing here reads a clock.

use super::plan::TaskIx;
use super::state::{RunIx, RunState, Tick};

/// What [`advance`](super::advance) asks the caller to do.
///
/// Two fields diverge from the plan's draft, both deliberately and both
/// recorded in `oracle-exec/README.md` rather than left for a reader to
/// rediscover:
///
/// - `ScheduleRetry::not_before` is a [`Tick`], not a `std::time::Instant` —
///   Task 2's correction, for the reason [`Tick`] gives.
/// - `StartRun` carries `at` — Task 3's, for the reason on the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CreateRun {
        task: TaskIx,
        parent: Option<RunIx>,
        value: Option<String>,
    },
    StartRun {
        run: RunIx,
        /// The `now` [`advance`](super::advance) held when it took this
        /// decision, stored verbatim on the attempt this command opens.
        ///
        /// **Why the command carries the instant.** Every elapsed-time rule in
        /// the artifact is measured from a run's attempt start —
        /// `RETRY_FAILED_TASK`'s `maxDuration` from
        /// `this.attempts.getFirst().getState().minDate()`
        /// (`TaskRun.java:337`), and `WaitFor`/`Pause` from their own start.
        /// Something therefore has to store that instant, and there were three
        /// places to put it: a `now` parameter on
        /// [`apply`](super::apply), a field the caller sets by hand, or here.
        /// Here is the only one where the stored instant cannot disagree with
        /// the one the decision was taken at.
        at: Tick,
    },
    /// A falsy `runIf:`: terminate this `CREATED` run as `SKIPPED`, with the
    /// one attempt Kestra records for it — W3c.
    ///
    /// **It is not a [`Command::SetRunState`], and the attempt is why.** Every
    /// one of Kestra's four skip sites writes the state *and* an attempt in the
    /// same expression —
    /// `taskRun.withState(SKIPPED).addAttempt(TaskRunAttempt.builder().state(new State().withState(SKIPPED)).build())`
    /// — and a `new State()` starts at `CREATED`, which is why the golden's
    /// skipped runs read `["CREATED","SKIPPED"]` with an attempt reading the
    /// same two. A `SetRunState` cannot produce that: [`apply`](super::apply)
    /// appends to the *last* attempt and a skipped run has none, so it would
    /// leave `attempts: []` — which is a different measured shape, the one a
    /// **failed render** of the same expression produces
    /// (`valids/task-runif`'s `willfailedtheflow` is `["CREATED","FAILED"]`
    /// with no attempt at all).
    ///
    /// It carries `at` for the same reason [`Command::StartRun`] does: the
    /// attempt it opens stores an instant, and the only value that instant can
    /// have without a second opinion is the `now` the decision was taken at.
    SkipRun {
        run: RunIx,
        at: Tick,
    },
    /// The plan's draft spells `not_before` as an `Instant`; it is a [`Tick`]
    /// for the reason [`Tick`] gives.
    ScheduleRetry {
        run: RunIx,
        not_before: Tick,
    },
    SetRunState {
        run: RunIx,
        state: RunState,
    },
    SetExecutionState {
        state: RunState,
    },
    /// Run one iteration of a [`Loop`](super::TaskKind::Loop) as a **separate
    /// execution**.
    ///
    /// A `Loop` does not iterate inside its own execution: each value produces
    /// a sub-execution carrying `parentId`, and the body's tasks never appear
    /// in the parent's `taskRunList`. Measured over the whole golden: 41 entries
    /// carry 142 sub-executions between them, and across the 39 comparable
    /// entries holding a `Loop` not one of the 82 tasks inside a loop body has a
    /// run in its parent — on egret's side or on Kestra's.
    ///
    /// # `task` is the LOOP, not the body
    ///
    /// A sub-execution runs the loop's **whole** `tasks:` list, which a single
    /// body index cannot name: `valids/loop-serial-multiple-tasks` has two body
    /// tasks and each of its two sub-executions ran both of them (`runs:
    /// ["first", "second"]`, twice, in the golden). So this names the `Loop`
    /// itself and the caller reads its list, which is also what Kestra's
    /// `loopRun` carries — `{"taskId": "1_each", "taskRunId": …, "index": …,
    /// "value": …}` names the loop task, never a body task.
    ///
    /// # Nothing executes this yet, and that is a hole with a count
    ///
    /// **This section is corrected rather than deleted, in the commit that
    /// opened `src/executor/`** — W6 Task 3, which is the first package that
    /// legitimately did, and the condition under which the pointer was left
    /// here (`oracle-exec/README.md`, *Pointers that no longer resolve*).
    ///
    /// It said *"Nothing executes this yet"*, and W3b Task 4 made that false:
    /// `worker::spawn::spawn_all` executes it, so the oracle's side of
    /// `spawned` is filled and 18 of the 41 entries that carry a sub-execution
    /// match. It also pointed at
    /// `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`, **a
    /// test that no longer exists**: the prefix comparison was all one batch
    /// could offer, and the successor is
    /// `every_sub_execution_egret_runs_is_one_kestra_ran`, a whole-set
    /// comparison. `the_sub_executions_are_compared_and_this_is_what_they_cost`
    /// derives the 41/142 population and what it still costs.
    /// Start the child execution a
    /// [`Subflow`](super::TaskKind::Executable) names, and link it by
    /// `trigger_execution_id`/`trigger_run_ix` -- **never** by
    /// `parent_execution_id`. W6.
    ///
    /// # Why this is not a [`Command::SpawnExecution`]
    ///
    /// Four reasons, each measured, and the third is the decisive one:
    ///
    /// 1. a `SpawnExecution` derives the child's source from the PARENT's own
    ///    YAML and carries a loop `value`; a `Subflow` resolves a **different
    ///    flow** by `(namespace, flowId)`;
    /// 2. migration 0135's CHECK made a child with a parent and no loop index
    ///    unrepresentable (0138 relaxed it, for the replay, not for this);
    /// 3. `oracle-exec/driver.rs` builds the oracle's `spawned` field from
    ///    `parent_execution_id`, and **all 41 golden `spawned`-carrying entries
    ///    have zero `Subflow` children** -- linking one that way would turn the
    ///    entries this command exists to bank into `spawned` mismatches;
    /// 4. Kestra itself makes the distinction: `parentId` for a loop iteration
    ///    and for a `CREATE_NEW_EXECUTION` replay, **NULL** for a subflow
    ///    child. Measured on the pinned artifact, four flows, four rounds:
    ///    `io.kestra.tests/subflow-parent`'s child carries `parentId: null` and
    ///    `trigger: {id: <task id>, type:
    ///    io.kestra.plugin.core.flow.Subflow, variables: {executionId,
    ///    taskRunId, taskId, namespace, flowId, flowRevision}}`.
    ///
    /// # It is emitted BESIDE the `StartRun`, in one round
    ///
    /// `apply` records nothing for it -- [`ExecutionState`](super::ExecutionState)
    /// has nowhere to put a child execution, exactly as for
    /// [`Command::SpawnExecution`] -- so a round that emitted this and left the
    /// run `CREATED` would emit it again on the next round, for ever.
    /// [`super::flowable::start`] therefore emits `[StartRun, SpawnSubflow]`
    /// together, which is also what Kestra does: `ExecutableUtils.subflowExecution`
    /// builds its `SubflowExecution` with
    /// `parentTaskRun(currentTaskRun.withState(State.Type.RUNNING))`.
    ///
    /// **The one shape this costs is recorded rather than hidden.** Measured on
    /// the artifact (`egret.probe/probe-unresolvable`), a `Subflow` whose
    /// `(namespace, flowId)` resolves to nothing gives a run reading
    /// `["CREATED", "FAILED"]` with one attempt reading `["FAILED"]` -- no
    /// `RUNNING` at all, because Kestra never gets as far as building the
    /// `SubflowExecution`. egret has already emitted `StartRun` by the time the
    /// lookup happens, so it produces `["CREATED", "RUNNING", "FAILED"]` with
    /// the matching attempt. **No golden entry reaches this**: the corpus's one
    /// unresolvable target is `valids/task-runif`'s `notexecutedflowable`,
    /// whose falsy `runIf:` skips it before anything resolves. See
    /// `worker::subflow` for the decision this shape is the tail of.
    SpawnSubflow {
        /// The `Subflow` task itself -- the caller reads its
        /// `namespace`/`flowId`/`inputs`/`wait` out of the execution's frozen
        /// source, because a `Plan` carrying a task's properties would be a
        /// second copy of the source.
        task: TaskIx,
        /// The run of that task this child belongs to -- `trigger_run_ix`.
        ///
        /// It is on the command rather than derived by the caller because two
        /// `Subflow` tasks may name the same target
        /// (`valids/subflow-parent-retry`'s `subflow1` and `subflow2` do), and
        /// a retried `Subflow` starts a second child under the SAME
        /// `(execution, run_ix)` -- which is why migration 0138's reverse index
        /// is deliberately not unique.
        run: RunIx,
    },
    SpawnExecution {
        task: TaskIx,
        /// The iteration value, already resolved to the string Kestra puts on
        /// `loopRun.value` — see
        /// [`iteration_values`](super::flowable::loop_::iteration_values).
        value: String,
        /// The run of the `Loop` this iteration belongs to.
        parent_run: RunIx,
    },
}
