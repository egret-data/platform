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
        /// A `LoopUntil`'s `checkFrequency.maxDuration` is the third, and it is
        /// the one that made this sentence load-bearing rather than
        /// explanatory: Kestra measures it from the loop taskrun's own
        /// `CREATED`, which `resetAttempts()` never appends to, and egret reads
        /// the same instant off the single attempt a `LoopUntil` opens and
        /// never reopens.
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
    /// Fail a `CREATED` run that never started, with the one attempt Kestra
    /// records for it -- group C2 Task 3 (spec D6).
    ///
    /// **It is not a [`Command::SetRunState`], for [`Command::SkipRun`]'s
    /// reason**: an update task (`SetVariables`, `UnsetVariables`, `Labels`)
    /// whose change cannot be computed reads run `CREATED FAILED` with
    /// attempts **`[["FAILED"]]`** in the golden
    /// (`valids/set-variables-duplicate`, `valids/labels-update-task-empty`),
    /// while a `SetRunState(FAILED)` on a run with no attempt leaves
    /// `attempts: []` -- the shape a `Loop` failing at `CREATED` has
    /// (`valids/loop-null`), and the wrong one here. Unlike `SkipRun`'s, the
    /// attempt's history has no `CREATED`: Kestra builds it from the failed
    /// state alone.
    ///
    /// It carries `at` for the same reason `SkipRun` does.
    FailUnstarted {
        run: RunIx,
        at: Tick,
    },
    /// The plan's draft spells `not_before` as an `Instant`; it is a [`Tick`]
    /// for the reason [`Tick`] gives.
    ScheduleRetry {
        run: RunIx,
        not_before: Tick,
    },
    /// Put a run into `PAUSED`, and record **who ends the wait**. W8 Task 3;
    /// the `resume` field is W8 Task 4 fix round 2.
    ///
    /// **Not "a [`Pause`](super::TaskKind::Pause) run", which is what this line
    /// said through two constructs and two review rounds.** It has been a
    /// `LoopUntil`'s since W8 Task 4, and since fix round 2 it is any kind's:
    /// the command names its resumer, so nothing about the KIND is what makes a
    /// run resumable. That is the fifth claim of this class the fix-round-2
    /// sweep corrected, and its program-checked twin is now the type system —
    /// this field cannot be omitted.
    ///
    /// # It is not a [`Command::SetRunState`], and the reason is measured
    ///
    /// [`apply`](super::apply)'s rule 1 pushes any state onto the run's open
    /// attempt, and `PAUSED` appears in **0** of the golden's 878 attempt
    /// histories while appearing in 20 of its 840 run histories. The shape is
    /// `valids/pause-delay`'s: run `["CREATED","RUNNING","PAUSED","SUCCESS"]`,
    /// attempt `["CREATED","RUNNING","SUCCESS"]` — the same attempt, still
    /// open, taking the terminal directly from the `RUNNING` it was left at.
    /// `RunState::Paused`'s own doc recorded this hazard at W8 Task 2 and left
    /// the guard out because nothing emitted the variant yet; this is the
    /// producer that makes it reachable, so the separation is a command rather
    /// than a special case inside `apply`.
    ///
    /// # `resume` names who ends the wait, and that is the whole point
    ///
    /// It used to be `until: Option<Tick>` — a deadline and nothing else — and
    /// "which construct ends this wait" lived in two hand-written lists of
    /// `TaskKind`s behind two catch-alls. **Measured: a `PauseRun` applied to a
    /// run of any third kind stalls for ever, silently** — 20 rounds past its
    /// deadline, 0 commands, under `Sequential`, `Parallel` and `Dag` alike.
    /// See [`Resume`](super::Resume) for the whole finding and for why
    /// changing this command's shape was ruled in scope.
    ///
    /// [`Resume::Never`](super::Resume::Never) is the old `until: None`, with a
    /// name: a `Pause` with neither `pauseDuration:` nor `timeout:`, which on
    /// the pinned artifact waits for a human and nothing else. It writes no
    /// [`TaskRun::not_before`](super::TaskRun::not_before) at all, which is
    /// what keeps the driver's clock from ever being asked to move for it —
    /// and **none of the seven corpus flows written that way is in
    /// `golden.json`**, so the oracle cannot see this distinction in either
    /// direction. That is why it is a variant with its own doc rather than an
    /// absent value.
    PauseRun {
        run: RunIx,
        resume: super::Resume,
    },
    /// Start the next iteration of a [`LoopUntil`](super::TaskKind::LoopUntil)
    /// run: put it back into `RUNNING` and throw away every run beneath it.
    /// W8 Task 4.
    ///
    /// # It is Kestra's `ExecutionService.retryWaitFor`, and both halves are
    /// that method's
    ///
    /// ```java
    /// if (taskRun.getId().equals(flowableTaskRunId)) {
    ///     return taskRun.resetAttempts().incrementIteration();
    /// }
    /// if (isDescendantOf(taskRun, flowableTaskRunId, byId)) {
    ///     return null;                       // removed from taskRunList
    /// }
    /// ```
    ///
    /// **The descendants are REMOVED there and RESET here, and the difference
    /// is representational rather than semantic.** A `RunIx` is a position in
    /// `ExecutionState::runs` and a `run_ix` column, so a run cannot be
    /// deleted without renumbering every run after it; resetting it to a fresh
    /// `CREATED` with no attempts produces the same thing the artifact's record
    /// holds, because the next iteration re-creates exactly the same children
    /// in the same order. The golden is what says so: `valids/waitfor-nested`'s
    /// `loop_1` ran **three** iterations and its child `loop_1_return` appears
    /// **once**, reading `CREATED SUBMITTED RUNNING SUCCESS` with one attempt —
    /// the last iteration's taskrun, with no trace of the two before it.
    ///
    /// **The loop's own run is NOT reset**, and that is the same method:
    /// `resetAttempts()` keeps the state histories and only rewinds `current`,
    /// which is why a `LoopUntil` run reads `CREATED RUNNING PAUSED RUNNING
    /// PAUSED RUNNING <terminal>` in the golden — one `CREATED`, never two.
    /// This command pushes the `RUNNING` onto the run's history and
    /// deliberately **not** onto its attempt, for [`Command::PauseRun`]'s
    /// reason: Kestra drops the attempt and builds a new one per iteration, so
    /// the record holds exactly one attempt reading `CREATED RUNNING
    /// <terminal>`, and pushing a second `RUNNING` onto a kept attempt would
    /// invent a history the golden does not have.
    ///
    /// # What a reset clears, and the one thing it cannot
    ///
    /// History (back to `[CREATED]`), attempts, both render slots and
    /// `not_before`. It cannot clear a descendant's stored **outputs**, which
    /// live in a column and not in [`ExecutionState`] — harmless for every run
    /// that is re-created (the next handler result overwrites them) and a
    /// stated gap for one that is not. Nothing in this corpus reaches it: the
    /// shape needed is a branch inside a `LoopUntil` whose taken side changes
    /// between iterations, and no corpus `LoopUntil` contains an `If` or a
    /// `Switch` at all.
    RestartLoop {
        run: RunIx,
    },
    /// Give a run that [`Command::RestartLoop`] threw away its `CREATED` back.
    /// W8 Task 4.
    ///
    /// # Why "thrown away" is a state and not an inference
    ///
    /// Kestra REMOVES a `LoopUntil`'s descendants from `taskRunList` and
    /// `FlowableUtils.resolveWaitForNext` builds them again **one at a time**,
    /// in list order, as the previous one terminates — so each re-created child
    /// carries the `startDate` of the moment the walk reached it, not the
    /// moment the iteration began. That ordering is compared:
    /// `valids/waitfor-nested` records `loop_1_return` **before** `loop_2` and
    /// `loop_3_return` **before** `loop_3_log`, and both endpoints are the last
    /// iteration's.
    ///
    /// A reset that wrote `CREATED` onto every descendant at once would give
    /// every sibling the SAME left endpoint and turn all three of those pairs
    /// into `overlapping` — measured, before this command existed. So
    /// `RestartLoop` empties a descendant's history instead, which is this
    /// executor's spelling of "removed from `taskRunList`", and the walk emits
    /// this command when it reaches one. An empty history is not a state a run
    /// can otherwise be in: [`Command::CreateRun`] pushes `CREATED` in the same
    /// breath as the run.
    ///
    /// It is not a [`Command::SetRunState`] for [`Marks`]'s sake as much as for
    /// clarity: `worker::claim::Marks` stamps a run's left endpoint when its
    /// history goes from empty to non-empty, which is exactly "created", and a
    /// `SetRunState` would have to be told apart from every other one.
    ///
    /// [`Marks`]: crate::worker
    RecreateRun {
        run: RunIx,
    },
    SetRunState {
        run: RunIx,
        state: RunState,
    },
    /// A terminal the RUN and its ATTEMPT do **not** agree on — W8 Task 5.
    ///
    /// [`Command::SetRunState`] writes one state to both, under the rule that
    /// the attempt takes it only while the attempt is still open. Two shapes in
    /// the artifact need a different pair, and neither can be spelled as a
    /// sequence of `SetRunState`s:
    ///
    /// | shape | run gets | the attempt gets | why |
    /// |---|---|---|---|
    /// | a **runnable** whose `allowFailure:`/`allowWarning:` moved its terminal | the softened state | the handler's own | `WorkerTaskProcessor` builds the attempt from the raw result and only then does `taskRun.withState(softened)`. `valids/task-allow-failure-runnable`'s golden is a run ending `WARNING` over three attempts that all end `FAILED` |
    /// | a **`Subflow`** whose `ExecutableUtils.guessState` disagrees with its child | the guessed state | the same state, **after the attempt has already closed** | `ExecutableUtils.subflowExecutionResult` rebuilds the attempt from the parent taskrun's whole `State`, so a `Subflow`'s attempt is a COPY of its run's history. `valids/task-allow-warning-executable-flow`'s run and attempt both read `CREATED RUNNING FAILED SUCCESS` |
    ///
    /// **`attempt` is pushed whether or not the attempt has closed**, which is
    /// the whole difference from `SetRunState` and is why the second row needs
    /// this command rather than a rule change. Widening `SetRunState` instead
    /// would be wrong for every retry: `valids/retry-failed`'s attempts each end
    /// `FAILED` and the `RETRYING` that follows must not join them.
    ///
    /// **`attempt` is a `RunState` and not an `Option`** — fix round 1. It was
    /// `Option`, with `None` documented as "leaves the attempt alone", and no
    /// caller ever wrote it: a variant nothing constructs is a branch in
    /// `apply` nobody has seen taken, which is the shape this package has paid
    /// for twice. A run whose history moves while no attempt is open is
    /// [`Command::SetRunState`]'s job and always was.
    ///
    /// It contains no decision: both halves are values the emitter has already
    /// chosen, exactly as `SkipRun`'s two writes are.
    CompleteRun {
        run: RunIx,
        /// Pushed onto the run's own history.
        run_state: RunState,
        /// Pushed onto the latest attempt, open or closed. A run with no
        /// attempt at all takes the `run_state` and nothing else, which is a
        /// fact about the state rather than a choice this command makes.
        attempt: RunState,
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
    /// Begin a `WorkingDirectory` block's prepare claim: `namespaceFiles`,
    /// `inputFiles` and the cache restore, before its first child (spec §5.1).
    /// The block stays `RUNNING`; the claim is the worker's, not a run.
    BeginWorkingDirectory {
        run: RunIx,
    },
    /// The block's children are done, so its finish claim runs: `outputFiles`
    /// and the cache save. It runs whether a child failed or not (spec §5.3, V2).
    FinishWorkingDirectory {
        run: RunIx,
    },
    /// The worker completed the claim that [`Command::BeginWorkingDirectory`]
    /// or [`Command::FinishWorkingDirectory`] asked for. A claim that FAILED is
    /// not this command: the worker fails the block's run with
    /// [`Command::SetRunState`], as it fails any run (spec §5.3, V3).
    CompleteWorkingDirectoryClaim {
        run: RunIx,
    },
}
