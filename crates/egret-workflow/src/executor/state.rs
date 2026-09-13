//! The snapshot [`advance`](super::advance) reads, and the bookkeeping that
//! records a [`Command`] against it.
//!
//! No I/O and no clock: `now` arrives as a [`Tick`] parameter, and the only
//! instants stored here are ones a [`Command`] carried in.

use super::command::Command;
use super::plan::{Plan, TaskIx};

/// An index into [`ExecutionState::runs`].
pub type RunIx = usize;

/// Milliseconds since the execution was created.
///
/// The programme spec writes `now: Instant`. It is spelled `Tick` here on
/// purpose: `std::time::Instant` cannot be constructed at an arbitrary value,
/// so every retry and timeout test would need a real clock — in a module whose
/// whole point is that it has none. The divergence from the spec's illustrative
/// signature is deliberate and is recorded in `oracle-exec/README.md`.
///
/// **The origin is the execution's creation**, which is why nothing here
/// carries an execution-start field: elapsed-since-the-execution-started *is*
/// `now.0`. That is the instant Kestra measures a `CREATE_NEW_EXECUTION`
/// retry's `maxDuration` from —
/// `execution.getMetadata().getOriginalCreatedDate()`,
/// `core/src/main/java/io/kestra/core/models/executions/TaskRun.java:316`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Tick(pub i64);

/// Kestra's `ExecutionMetadata` — the two facts about an execution that are
/// **not** facts about its runs, and that a `CREATE_NEW_EXECUTION` retry is the
/// only reader of.
///
/// `core/src/main/java/io/kestra/core/models/executions/ExecutionMetadata.java`
/// carries exactly `attemptNumber` (defaulting to 1) and `originalCreatedDate`,
/// and `ExecutionMetadata.nextAttempt()` is the whole of how a replay derives
/// the child's: `attemptNumber + 1`, the original date untouched.
///
/// # Why this is on [`ExecutionState`] and not on the caller
///
/// **It is what makes the replay chain terminate**, and until W6 Task 4 nothing
/// in this module could see it. [`retry::next_retry_at`](super::retry) reads
/// `(1, Tick(0))` for `CREATE_NEW_EXECUTION` — "this execution IS the first
/// attempt, and `Tick`'s origin is its own creation" — which is true of the
/// first execution and false of every replay, so `maxAttempts` never bites and
/// `maxDuration` never accumulates. The caller cannot repair that after the
/// fact: the golden's third execution ends `["CREATED","RUNNING","FAILED"]`
/// with **no** `RETRIED`, and whether `RETRIED` is emitted is [`advance`]'s
/// output, not the caller's. So the input widens; `advance` stays pure over
/// `(plan, state, now)`.
///
/// [`advance`]: super::advance
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionMeta {
    /// `execution.getMetadata().getAttemptNumber()` — **1** for an execution
    /// nobody has replayed, `N` for the `N-1`th replay of one.
    ///
    /// It counts EXECUTIONS of a `CREATE_NEW_EXECUTION` chain. The other
    /// attempt count in this module, [`TaskRun::attempts`], counts a
    /// `RETRY_FAILED_TASK`'s attempts inside ONE execution. They are the two
    /// clocks `retry`'s header table separates and they are never the same
    /// number: all four `retry-new-execution-*` entries end each execution with
    /// exactly one attempt on the only run they have.
    pub attempt: u32,
    /// `execution.getMetadata().getOriginalCreatedDate()`, in this execution's
    /// own [`Tick`] space — so **`Tick(0)` for an original and a NEGATIVE tick
    /// for a replay**, the instant its chain began measured backwards from its
    /// own creation.
    ///
    /// A `Tick` rather than a duration because that is what it is compared
    /// against: `nextDate.isAfter(originalCreatedDate.plus(maxDuration))` is
    /// `next.0 > origin.0 + maxDuration` here, and both sides have to be in one
    /// clock. `Tick`'s doc says the origin is *the execution's creation*, which
    /// stays exactly true — this is where the ORIGINAL execution's creation
    /// falls on that scale.
    pub origin: Tick,
}

impl Default for ExecutionMeta {
    /// An execution nobody replayed: attempt 1, and its own creation is the
    /// origin. Kestra's `@Builder.Default attemptNumber = 1` plus an
    /// `originalCreatedDate` that is the execution's own `createdDate`.
    fn default() -> Self {
        Self {
            attempt: 1,
            origin: Tick(0),
        }
    }
}

/// The snapshot [`advance`](super::advance) reads. Owned by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionState {
    /// CREATION order — the oracle compares this order.
    pub runs: Vec<TaskRun>,
    pub history: Vec<RunState>,
    /// Kestra's `Execution.metadata`. Defaulted for every execution that is not
    /// a `CREATE_NEW_EXECUTION` replay, which is all of them until one exists.
    pub metadata: ExecutionMeta,
}

impl ExecutionState {
    /// A freshly created execution: no runs, history `[Created]`.
    pub fn new(_plan: &Plan) -> Self {
        Self {
            runs: Vec::new(),
            history: vec![RunState::Created],
            metadata: ExecutionMeta::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRun {
    pub task: TaskIx,
    pub parent: Option<RunIx>,
    /// A loop iteration's value.
    ///
    /// Measured over the whole corpus, Kestra's own `TaskRun.value` is null on
    /// all 840 runs in `golden.json`: the iteration value lives on the
    /// sub-execution's `loopRun.value`. The field is kept because
    /// `Command::CreateRun` carries it and Task 6 spawns with it.
    pub value: Option<String>,
    /// The caller's render of this task's expression — an `If` condition or a
    /// `Switch` value. `advance()` is pure and cannot render, so it reads the
    /// result here. `None` means "not rendered yet", which is not an error.
    /// Populated from Task 5 onward; the field exists from Task 3 so no later
    /// task changes this struct's shape.
    pub rendered: Option<String>,
    /// The caller's render of this task's `runIf:` — W3c.
    ///
    /// **A second field rather than a second use of [`TaskRun::rendered`]**,
    /// because one run can need both: `runIf:` is on `Task` and so is carried
    /// by every kind, including an `If` that also has a `condition`. They are
    /// also read at different states — `runIf:` at `CREATED`, before the run is
    /// started at all, and a branch expression at `RUNNING` — so a single slot
    /// would have to be cleared between them.
    ///
    /// `None` means "the caller has not answered yet" and the only correct
    /// response to it is to emit nothing and wait. It is **not** falsy: reading
    /// it that way would skip every run whose `runIf:` the caller had not
    /// reached.
    pub rendered_run_if: Option<String>,
    pub history: Vec<RunState>,
    pub attempts: Vec<Attempt>,
    pub not_before: Option<Tick>,
}

impl TaskRun {
    /// The [`Tick`] at which this run's first attempt started, or `None` if it
    /// has not started one.
    ///
    /// **Derived, not stored.** Kestra measures a `RETRY_FAILED_TASK` retry's
    /// `maxDuration` from exactly this instant —
    /// `this.attempts.getFirst().getState().minDate()`,
    /// `core/src/main/java/io/kestra/core/models/executions/TaskRun.java:337` —
    /// so a second `TaskRun::started_at` field would be a copy of
    /// `attempts[0].started_at` that could disagree with it. One writer, one
    /// reader.
    pub fn started_at(&self) -> Option<Tick> {
        self.attempts.first().map(|a| a.started_at)
    }

    /// The [`Tick`] at which the attempt now in flight started, or `None` if
    /// this run has never started one. This is what a `timeout` on a *flowable*
    /// (`Pause`, `WaitFor`) is measured from.
    pub fn attempt_started_at(&self) -> Option<Tick> {
        self.attempts.last().map(|a| a.started_at)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub history: Vec<RunState>,
    /// The `now` [`advance`](super::advance) held when it decided to start this
    /// attempt, carried in on [`Command::StartRun`].
    ///
    /// **This field is the whole of W3a Task 3's answer to the hole
    /// `oracle-exec/README.md` records under *What this oracle does not prove*,
    /// cause 2.** Before it, `ExecutionState` and `TaskRun` carried no instant
    /// except `not_before`, so `advance()` had nothing to subtract `now` from
    /// and could not implement any elapsed-time rule however the environment
    /// behaved. Task 7 (`RetryPolicy::max_duration`) and Task 5 (`WaitFor`'s
    /// `maxDuration`) are the two that need it; see [`TaskRun::started_at`] for
    /// the Kestra line each is measured from.
    pub started_at: Tick,
}

/// The states a run passes through — **and, through
/// [`Command::SetExecutionState`], the states an execution passes through too**.
/// One type for both, so the vocabulary it has to cover is the union of both.
///
/// **That union is 13 states**, measured over `golden.json`; this enum holds
/// **11** of them. Every number in this paragraph is derived on every run by
/// `tests/executor.rs::the_state_vocabulary_this_enum_holds_is_derived_from_the_golden`,
/// which is what stops it becoming a count written beside a derivation — it was
/// one from Task 3 until Task 7 had to move it.
///
/// **That test read a literal list of its own until W3c, so it could not see
/// this enum at all**; it now derives the vocabulary from [`RunState::ALL`].
/// Adding `Skipped` here left it green, which is precisely the failure it
/// existed to prevent.
///
/// Measured over the 266 `runs` entries, by how many are wholly inside the
/// vocabulary: **35** for the six the plan drafted, **229** once Task 3 added
/// `Submitted`, **233** once Task 7 added `Retried`, **237** once W3c added
/// `Skipped`, **253** once W8 Task 2 added `Paused` and `Restarted`.
/// `Submitted` is the addition that made the sequential slice
/// reachable at all; `Retried` buys exactly the four `retry-new-execution-*`
/// entries, none of which can bank, because the replacement execution a
/// `CREATE_NEW_EXECUTION` retry starts is not representable here at all.
///
/// **W8 Task 2's 16 buy nothing either, and that is stated rather than left to
/// a green ratchet to imply.** Being inside the vocabulary is necessary and not
/// sufficient — `SKIPPED`'s four and `RETRIED`'s four are the two earlier
/// measured examples — and nothing in this crate *emits* either new variant
/// yet: the constructs that do are `Pause` and `LoopUntil`, both still planned
/// opaque by [`build_plan`](super::plan::build_plan). The variants are here
/// because those two constructs need the **same** one, so splitting them across
/// packages would block each on the other's state.
///
/// **Both producers exist now** — `Pause` at W8 Task 3 (eight banked entries)
/// and `LoopUntil` at W8 Task 4 (one) — and they leave the state by different
/// doors, which is what `advance`'s `pause_mark` has three arms for: a resumed
/// `Pause` owes the execution `RESTARTED` and a resumed `LoopUntil` owes it
/// `RUNNING`. Measured: of the golden's 18 `PAUSED` execution histories, 12
/// carry a `RESTARTED` and 6 do not, and the 6 are exactly `LoopUntil`'s.
///
/// The two still missing, with the number of `runs` entries whose trace uses
/// each: `WARNING` 10, `CANCELLED` 3.
/// `oracle-exec/README.md` prints the full table and names which later task owns
/// each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Created,
    /// The executor has handed the task to a worker and the worker has not
    /// picked it up yet.
    ///
    /// **Measured, and it is the only thing in Kestra's record that separates a
    /// task the worker ran from a flowable the executor ran**: on
    /// `valids/parallel`, `parent` (the `Parallel` itself) goes `CREATED RUNNING
    /// SUCCESS` while each of `t1`..`t6` goes `CREATED SUBMITTED RUNNING
    /// SUCCESS`. It appears in 580 of the golden's 840 run histories and in no
    /// execution history.
    Submitted,
    Running,
    Success,
    Failed,
    Retrying,
    /// The terminal a `CREATE_NEW_EXECUTION` retry leaves behind.
    ///
    /// Added by Task 7, which is what took this enum from 7 of the golden's 13
    /// states to 8. It is a **terminal** state — `State.java`'s own
    /// `isTerminated()` lists `RETRIED` beside `FAILED` and `SUCCESS`, and the
    /// harvester already reads that list verbatim — and that is the whole of
    /// what makes the four `retry-new-execution-*` entries stop rather than
    /// spin: Kestra replays the flow as a *different* execution, which nothing
    /// in [`Command`] can ask for.
    Retried,
    Killed,
    /// The terminal a falsy `runIf:` leaves behind — W3c, which is what took
    /// this enum from 8 of the golden's 13 states to **9**.
    ///
    /// **It is the golden's whole `SKIPPED` population and it has exactly one
    /// producer.** `State.Type.SKIPPED` is written at four sites in the whole
    /// Kestra checkout's main sources and every one of them is guarded by
    /// `TruthUtils.isTruthy(runContext.render(task.getRunIf()))` —
    /// `executor/…/ExecutorService.java:1302` and `:1382`,
    /// `executor/…/handler/ExecutionEventMessageHandler.java:272`, and
    /// `worker/…/processors/WorkerTaskProcessor.java:141`. Nothing else in the
    /// artifact can put a run into it.
    ///
    /// **Terminal, and terminal in the benign half.** `State.java:288` lists it
    /// beside `SUCCESS` in `isTerminated()`, and `:293` lists it again in
    /// `isTerminatedNoFail()`; `Execution.guessFinalState` has no arm for it, so
    /// a skipped child leaves its parent's aggregate at whatever it was.
    /// `valids/task-runif-executionupdating` is that case measured — two of its
    /// five runs are `SKIPPED` and the execution ends `SUCCESS`.
    Skipped,
    /// **Non-terminal**, and the state `Pause` and `LoopUntil` both stop in —
    /// W8 Task 2, which is what took this enum from 9 of the golden's 13 states
    /// to 10.
    ///
    /// **Non-terminal is measured on the pinned artifact, not inferred from the
    /// name.** `State.Type.isTerminated()` is `FAILED || WARNING || SUCCESS ||
    /// KILLED || CANCELLED || RETRIED || SKIPPED || RESUBMITTED` — eight
    /// members, and `PAUSED` is not one of them; nor is it in
    /// `isTerminatedNoFail()` or `isTerminatedInError()`. Decompiled from
    /// `/app/kestra` in `kestra/kestra@sha256:28f59bf1…` (class-file v69,
    /// Corretto 25), `io/kestra/core/models/flows/State.java`,
    /// `State.Type.isTerminated()`. The same file gives `isPaused()` as
    /// `this == PAUSED` and `canBeRestarted()` as `isFailed() || isPaused()`.
    ///
    /// **The golden agrees from the other side**: over the 408 execution
    /// histories, `PAUSED` is in 18 and **ends none of them**; over the 840 run
    /// histories it is in 20 and ends none.
    ///
    /// **And it appears in 0 of the golden's 878 attempt histories** — derived
    /// on every run by
    /// `tests/executor.rs::the_golden_populations_are_derived_recursed_and_unrecursed`,
    /// which pins 408/840/878 beside the un-recursed 266/570/604 because
    /// **this number was published as 604 for one round**: that is the same
    /// walk with recursion off, and the sentence around it said "recursively".
    /// That is
    /// not a detail: [`apply`]'s rule 1 pushes any non-terminal `SetRunState`
    /// onto the open attempt, so the first producer of this variant will push a
    /// state Kestra never records unless it emits something other than a plain
    /// `SetRunState`. `valids/flow-concurrency-cancel-pause` is the shape —
    /// run `["CREATED","RUNNING","PAUSED","SUCCESS"]`, attempt
    /// `["CREATED","RUNNING","SUCCESS"]`.
    ///
    /// **W8 Task 3 is that producer, and it took the OTHER answer**: rather
    /// than a rule inside [`apply`] excluding this variant from rule 1, the
    /// pause emits a [`Command::PauseRun`](super::Command) — a command that
    /// writes the run's history and deliberately not the attempt's. `apply`
    /// therefore still contains no decision, which is what its own doc
    /// promises, and the exclusion is visible at the call site rather than
    /// buried in a `!= Paused`. The measurement above is what that command's
    /// doc cites.
    ///
    /// [`apply`]: self::apply
    Paused,
    /// **Non-terminal, execution-only, and it is Kestra's word for "created
    /// again"** — W8 Task 2, which is what took this enum from 10 of the
    /// golden's 13 states to **11**.
    ///
    /// `State.Type.isCreated()` is `this == CREATED || this == RESTARTED`, so
    /// in the artifact `RESTARTED` is a member of the *created* class rather
    /// than a terminal, and `isTerminated()` does not list it. It is written by
    /// `ExecutionService.markAs` — `return newState == CANCELLED ?
    /// withState(CANCELLED) : withState(RESTARTED)` — which is the tail of
    /// `ExecutionService.resume`, i.e. **what a resumed pause leaves on the
    /// execution**; `ExecutorService.handleRestart` then maps it straight back
    /// to `RUNNING`. Both decompiled from the pinned digest.
    ///
    /// **Execution-only is measured, not assumed**: `RESTARTED` appears in 12
    /// of the golden's 408 execution histories and in **0** of its 840 run
    /// histories and **0** of its 878 attempt histories — all three derived by
    /// `the_golden_populations_are_derived_recursed_and_unrecursed`, not
    /// written here. Every one of the 12 is
    /// a `Pause` that was resumed, and the shape is always
    /// `CREATED RUNNING PAUSED RESTARTED RUNNING …`. The six remaining `PAUSED`
    /// executions carry no `RESTARTED` at all — those are `LoopUntil`'s, which
    /// re-enters `RUNNING` directly.
    ///
    /// **[`apply`]'s rule 2 is therefore narrower than Kestra's `isCreated()`
    /// and deliberately stays so.** That rule clears a run's renders when the
    /// run re-enters `CREATED`; Kestra would count `RESTARTED` as a re-creation
    /// too. Widening it would be a guard on a path 0 of 840 run histories
    /// takes.
    ///
    /// [`apply`]: self::apply
    Restarted,
}

impl RunState {
    /// Every variant, in wire order — the **one** list the oracle's vocabulary
    /// arm reads.
    ///
    /// **It exists because that arm was a guard that could not fail.**
    /// `the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` carried
    /// its own literal copy of the eight state names, so it measured a *list in
    /// the test file* against the golden and never once looked at this enum:
    /// adding `Skipped` here left it green, which is exactly what it exists to
    /// notice. It now derives `held` from this slice.
    ///
    /// [`RunState::slot`] is exhaustive, so a variant added to this enum forces
    /// an arm there, and `run_state_all_holds_every_variant_in_order`
    /// round-trips the two. What that pair does **not** prove is that a variant
    /// given a fresh `slot` was also appended here — nothing in Rust can
    /// enumerate the variants to check it — so that is stated rather than
    /// claimed away.
    pub const ALL: &'static [RunState] = &[
        RunState::Created,
        RunState::Submitted,
        RunState::Running,
        RunState::Success,
        RunState::Failed,
        RunState::Retrying,
        RunState::Retried,
        RunState::Killed,
        RunState::Skipped,
        RunState::Paused,
        RunState::Restarted,
    ];

    /// This variant's position in [`RunState::ALL`]. Exhaustive on purpose.
    pub const fn slot(self) -> usize {
        match self {
            RunState::Created => 0,
            RunState::Submitted => 1,
            RunState::Running => 2,
            RunState::Success => 3,
            RunState::Failed => 4,
            RunState::Retrying => 5,
            RunState::Retried => 6,
            RunState::Killed => 7,
            RunState::Skipped => 8,
            RunState::Paused => 9,
            RunState::Restarted => 10,
        }
    }

    /// Kestra's spelling, which is what the golden holds.
    pub fn wire(self) -> &'static str {
        match self {
            RunState::Created => "CREATED",
            RunState::Submitted => "SUBMITTED",
            RunState::Running => "RUNNING",
            RunState::Success => "SUCCESS",
            RunState::Failed => "FAILED",
            RunState::Retrying => "RETRYING",
            RunState::Retried => "RETRIED",
            RunState::Killed => "KILLED",
            RunState::Skipped => "SKIPPED",
            RunState::Paused => "PAUSED",
            RunState::Restarted => "RESTARTED",
        }
    }

    /// True where `State.Type.isTerminated()` is true for the same name. The
    /// list is Kestra's, read from
    /// `core/src/main/java/io/kestra/core/models/flows/State.java` at the
    /// checkout and already relied on by `oracle-exec/harvest.py`; only the
    /// members this enum currently has appear here.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Success
                | RunState::Failed
                | RunState::Retried
                | RunState::Killed
                | RunState::Skipped
        )
    }
}

/// Record one [`Command`] against the state.
///
/// `advance()` decides; this records. It is in the crate rather than in the
/// oracle test because the oracle is a *caller* and W3b's worker will be
/// another, and two callers writing their own bookkeeping is two chances for
/// the thing being compared to differ from the thing that ships.
///
/// It contains no decision: every branch is a mechanical write of what the
/// command already says. That is also why [`Command::StartRun`] carries the
/// instant rather than this function taking a `now`: with the instant on the
/// command there is exactly one value it can be — the one `advance()` saw —
/// and no caller can record a different one.
///
/// # The two rules in here, and where the first is knowingly wrong
///
/// ## 1. A terminated [`Attempt`] takes no further state (Task 7)
/// Before `retry:` a run
/// had at most one attempt and never outlived it, so every `SetRunState` was
/// also an attempt state. A retried run's history reads `… RUNNING FAILED
/// RETRYING CREATED SUBMITTED RUNNING FAILED …` while each of its attempts
/// reads exactly `CREATED RUNNING FAILED` — measured on
/// `valids/retry-failed-task-attempts`, whose four attempts are identical and
/// carry neither the `RETRYING` nor the second `CREATED` that sit between them
/// in the run's own history. Without this rule the `RETRYING` and the re-`CREATED`
/// would land on the attempt that has already finished.
///
/// It is a rule and not a mechanism, so it can be wrong, and there is a
/// measured shape it *is* wrong for: a **`Subflow`**. `valids/retry-subflow`'s
/// second attempt reads `CREATED RUNNING FAILED RETRYING CREATED RUNNING
/// FAILED` — Kestra accumulates the sub-execution's retries inside the parent's
/// attempt — and `valids/subflow-parent-retry` is the same shape. Both are
/// baselined -- `Subflow` is a [`TaskKind::Executable`](super::TaskKind::Executable)
/// since W6, not opaque, and the two entries stay baselined on exactly this
/// rule -- and no reachable entry has an attempt that continues past its own
/// terminal. That is why the rule is stated here with its counter-example
/// rather than left as an invariant somebody later discovers is not one.
///
/// **W6 measured a SECOND `Subflow` shape this rule is wrong for, and left the
/// rule alone because nothing reachable can express it.** When a `Subflow`'s
/// child terminates, Kestra writes the CHILD's state onto the parent run and
/// then, if `ExecutableUtils.guessState` disagrees, writes that too -- and the
/// attempt takes **both**. Measured on the pinned artifact,
/// `egret.probe/probe-notransmit-parent` (`transmitFailed: false` over a child
/// that FAILS) gives a run AND an attempt each reading
/// `["CREATED","RUNNING","FAILED","SUCCESS"]`, and the golden holds the same
/// shape for `valids/task-allow-failure-executable-flow` (`... FAILED,
/// WARNING`). Neither is reachable here: [`RunState`] has no `Warning`,
/// `models::Task` carries neither `allowFailure` nor `allowWarning`, and **no
/// corpus `Subflow` writes `transmitFailed: false`** -- re-derived over all 25
/// declarers by `tests/executor.rs`'s
/// `no_corpus_subflow_turns_transmit_failed_off`.
/// `worker::subflow::guess_state` records the same boundary from the other
/// side.
///
/// ## 2. A run re-entering `CREATED` has no render (W3c fix round 2)
///
/// [`Command::CreateRun`] sets [`TaskRun::rendered`] and
/// [`TaskRun::rendered_run_if`] to `None`, and
/// [`reopen_due`](super::retry::reopen_due) puts a retried run back into
/// `CREATED` with a plain [`Command::SetRunState`]. Without clearing them there,
/// **attempt 2 is decided by attempt 1's render** — and for a `runIf:` that is a
/// narrower instance of the over-acceptance W3c exists to close: Kestra
/// re-renders `runIf:` at every worker-task emission
/// (`ExecutionEventMessageHandler.java:269`), so a condition that has become
/// falsy skips attempt 2, and a cached `"true"` runs it.
///
/// It is a *restoration* of the invariant `CreateRun` sets rather than a
/// decision, which is why it is here and not in `advance`. **`rendered_run_if`
/// is the first slot that can reach this at all**:
/// [`scan`](super::retry::scan) only retries a `Runnable`, and a `Runnable`
/// never had a `rendered`. `rendered` is cleared with it because the invariant
/// is one invariant, and because a later task that retries a flowable would
/// otherwise inherit the same defect silently.
///
/// `a_run_if_is_re_asked_on_every_attempt` holds it, and
/// `no_corpus_flow_has_both_a_retry_and_a_run_if` derives why no golden entry
/// can.
/// Whether `ix` is a descendant of `ancestor`, at any depth — Kestra's
/// `ExecutionService.isDescendantOf`, walking the same `parentTaskRunId` chain.
///
/// It terminates for any state, cyclic or not: a run's `parent` is fixed at
/// creation to a run that already existed, so parent indices strictly decrease
/// and the walk is bounded by `ix` itself. That is the same argument
/// `flowable::descend` records for having no depth guard, and the `> ix` break
/// below is what makes it true of a state a caller stored and read back rather
/// than only of one this executor built.
fn is_descendant_of(state: &ExecutionState, ix: usize, ancestor: RunIx) -> bool {
    let mut cur = state.runs[ix].parent;
    while let Some(p) = cur {
        if p == ancestor {
            return true;
        }
        if p >= ix {
            return false;
        }
        cur = state.runs[p].parent;
    }
    false
}

pub fn apply(state: &mut ExecutionState, cmd: &Command) {
    match cmd {
        Command::CreateRun {
            task,
            parent,
            value,
        } => state.runs.push(TaskRun {
            task: *task,
            parent: *parent,
            value: value.clone(),
            rendered: None,
            rendered_run_if: None,
            history: vec![RunState::Created],
            attempts: Vec::new(),
            not_before: None,
        }),
        Command::StartRun { run, at } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Running);
            r.attempts.push(Attempt {
                history: vec![RunState::Created, RunState::Running],
                started_at: *at,
            });
        }
        // The `SKIPPED` state and the one attempt Kestra records with it, in
        // one write -- see `Command::SkipRun` for why this is not a
        // `SetRunState`. The attempt's own history is `[CREATED, SKIPPED]`
        // because Java's `new State()` starts at `CREATED` and `.withState`
        // appends, which is what the golden holds for all six skipped runs.
        Command::SkipRun { run, at } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Skipped);
            r.attempts.push(Attempt {
                history: vec![RunState::Created, RunState::Skipped],
                started_at: *at,
            });
        }
        Command::ScheduleRetry { run, not_before } => {
            state.runs[*run].not_before = Some(*not_before);
        }
        // `PAUSED` on the RUN's history and deliberately not on the attempt --
        // see `Command::PauseRun` for the 0-of-878 measurement that makes this
        // a command of its own rather than a `SetRunState`. `until` is written
        // to `not_before` because that is the one field the environment reads
        // to decide the clock has to move (`oracle-exec/driver.rs::pass_time`),
        // and an unbounded pause writes none: the run is left with whatever
        // `not_before` it already had, which for a `Pause` is always `None`
        // because nothing else ever sets one on a run this executor drives.
        Command::PauseRun { run, until } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Paused);
            if let Some(t) = until {
                r.not_before = Some(*t);
            }
        }
        // W8 Task 4 -- Kestra's `ExecutionService.retryWaitFor`, whose two
        // halves are the two loops below. See `Command::RestartLoop`.
        Command::RestartLoop { run } => {
            let descendants: Vec<usize> = (0..state.runs.len())
                .filter(|&ix| is_descendant_of(state, ix, *run))
                .collect();
            for ix in descendants {
                let r = &mut state.runs[ix];
                // **Empty, not `[CREATED]`** -- see `Command::RecreateRun` for
                // the measurement that forced the distinction. Kestra deletes
                // these taskruns; the walk builds them back one at a time, and
                // an empty history is what lets each one be built back at the
                // round it is actually reached.
                r.history.clear();
                r.attempts.clear();
                r.rendered = None;
                r.rendered_run_if = None;
                r.not_before = None;
            }
            let r = &mut state.runs[*run];
            // `RUNNING` on the run and NOT on the attempt -- the same
            // separation `PauseRun` makes, and for the same measurement: the
            // golden holds ONE attempt per `LoopUntil` run whatever its
            // iteration count.
            r.history.push(RunState::Running);
            // The condition is re-rendered every iteration. Kestra re-renders
            // it too -- `runContext.render(this.condition).skipCache()`, and
            // `skipCache()` is in the artifact's source for exactly this
            // reason -- so a cached answer would decide iteration k+1 from
            // iteration k's outputs, which is the same over-acceptance
            // `apply`'s rule 2 closes for a retried `runIf:`.
            r.rendered = None;
            r.not_before = None;
        }
        // The run's own history always takes the state. The attempt takes it
        // only while the attempt is still open, and that is a **rule** rather
        // than bookkeeping -- see the block comment below.
        Command::SetRunState { run, state: s } => {
            let r = &mut state.runs[*run];
            r.history.push(*s);
            // Rule 2 (W3c fix round 2): a run re-entering `CREATED` has no
            // render for the attempt it is about to take. See the block comment
            // above -- this restores exactly the invariant `CreateRun` sets, and
            // `retry::reopen_due` is the only writer that can reach it.
            if *s == RunState::Created {
                r.rendered = None;
                r.rendered_run_if = None;
            }
            match r.attempts.last_mut() {
                Some(a) if !a.history.last().is_some_and(|last| last.is_terminal()) => {
                    a.history.push(*s)
                }
                _ => {}
            }
        }
        // W8 Task 4. The counterpart of `RestartLoop`'s emptying -- see
        // `Command::RecreateRun`. It is deliberately unconditional rather than
        // guarded on the history being empty: `apply` takes no decisions, and
        // the only emitter asks only for runs whose history is empty.
        Command::RecreateRun { run } => state.runs[*run].history.push(RunState::Created),
        Command::SetExecutionState { state: s } => state.history.push(*s),
        // Nothing to record. A sub-execution is a different execution with its
        // own `ExecutionState`, and this one holds no field for it -- which is
        // exactly why `advance()` cannot schedule a loop's SECOND batch and
        // why `Loop::concurrency_limit` reaches only the first. Writing
        // anything here would be inventing W3b's protocol; leaving the arm out
        // would be a non-exhaustive match rather than a decision, so it is
        // spelled out.
        Command::SpawnExecution { .. } => {}
        // Nothing to record, for `SpawnExecution`'s reason and with one extra
        // consequence spelled out on `Command::SpawnSubflow` itself: because
        // this leaves the state untouched, a round that emitted it without also
        // starting the run would emit it again for ever, which is why
        // `flowable::start` emits it BESIDE the `StartRun` rather than instead
        // of one.
        Command::SpawnSubflow { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`RunState::ALL`] and [`RunState::slot`] agree, in both directions this
    /// pair can check.
    ///
    /// `slot` is exhaustive, so a new variant cannot be added to the enum
    /// without an arm; this requires the arm's number to be the variant's
    /// actual position here. The residual gap — a variant given a slot beyond
    /// the end of `ALL` — shows up as an index panic the first time anything
    /// walks `ALL` looking for it, and is named on `ALL`'s own doc rather than
    /// papered over.
    #[test]
    fn run_state_all_holds_every_variant_in_order() {
        for (i, state) in RunState::ALL.iter().enumerate() {
            assert_eq!(
                state.slot(),
                i,
                "{state:?} is at index {i} of RunState::ALL but claims slot {}",
                state.slot()
            );
        }
        let wires: std::collections::BTreeSet<&str> =
            RunState::ALL.iter().map(|s| s.wire()).collect();
        assert_eq!(
            wires.len(),
            RunState::ALL.len(),
            "two variants share a wire spelling, so the golden cannot tell them apart"
        );
    }

    /// **`SKIPPED` is terminal, and terminal in the half that does not fail.**
    ///
    /// `State.java:288` lists it in `isTerminated()` beside `SUCCESS` and
    /// `:293` lists it again in `isTerminatedNoFail()`. Both halves matter here:
    /// the first is what lets a sequence walk past a skipped child, and the
    /// second is what keeps a skipped sibling from failing a `Parallel`.
    #[test]
    fn a_skipped_run_is_terminal() {
        assert!(RunState::Skipped.is_terminal());
        assert_eq!(RunState::Skipped.wire(), "SKIPPED");
    }

    /// **Neither state W8 Task 2 added is terminal, and the control is the
    /// other nine.**
    ///
    /// One-sided would be worthless here: `is_terminal` is an allow-list, so a
    /// pair of `assert!(!…)` passes on an implementation that answers `false`
    /// for everything. Both directions are asserted against Kestra's own
    /// `State.Type.isTerminated()`, decompiled from `/app/kestra` in
    /// `kestra/kestra@sha256:28f59bf1…`:
    ///
    /// ```text
    /// return this == FAILED || this == WARNING || this == SUCCESS
    ///     || this == KILLED || this == CANCELLED || this == RETRIED
    ///     || this == SKIPPED || this == RESUBMITTED;
    /// ```
    ///
    /// Five of those eight are members of this enum and every one of them is
    /// asserted true; the six non-members of the list are asserted false. So a
    /// `Paused` quietly added to `is_terminal` fails, and so does a `Success`
    /// quietly removed.
    ///
    /// The wire spellings are here for the same reason `run_state_all_holds…`
    /// checks them: a variant whose `wire()` collides with another's is
    /// invisible to the golden.
    #[test]
    fn neither_paused_nor_restarted_is_terminal() {
        for state in [
            RunState::Success,
            RunState::Failed,
            RunState::Killed,
            RunState::Retried,
            RunState::Skipped,
        ] {
            assert!(
                state.is_terminal(),
                "{state:?} is in State.Type.isTerminated() and must stay terminal"
            );
        }
        for state in [
            RunState::Created,
            RunState::Submitted,
            RunState::Running,
            RunState::Retrying,
            RunState::Paused,
            RunState::Restarted,
        ] {
            assert!(
                !state.is_terminal(),
                "{state:?} is NOT in State.Type.isTerminated() -- a run in it is \
                 still the executor's business"
            );
        }
        assert_eq!(RunState::Paused.wire(), "PAUSED");
        assert_eq!(RunState::Restarted.wire(), "RESTARTED");
        assert_eq!(
            RunState::ALL.len(),
            11,
            "the golden's vocabulary is 13; this enum now holds 11 of it, and \
             `tests/executor.rs::the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` \
             is what derives the rest"
        );
    }

    /// **`Paused` enters `walk`'s waiting arm rather than its terminal arm**,
    /// driven rather than asserted about.
    ///
    /// [`RunState::is_terminal`] is read by `flowable::is_terminal`, which is
    /// read by `sequential::walk`; this is the only one of the three a caller
    /// can drive with a hand-built state, and driving it is what makes the
    /// difference between "the predicate says false" and "the executor waits".
    ///
    /// The control is the row above it: the SAME fixture with the run in
    /// `Success` retires that child and moves on, so a `walk` that answered
    /// `Waiting` for everything fails here.
    #[test]
    fn a_paused_run_makes_its_sequence_wait() {
        use crate::executor::flowable::{sequential, Step};

        let plan = crate::executor::build_plan(
            "id: f\nnamespace: n\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: x\n  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: y\n",
        )
        .expect("the fixture flow must plan");
        let roots = plan.roots.clone();

        let mut state = ExecutionState::new(&plan);
        apply(
            &mut state,
            &Command::CreateRun {
                task: roots[0],
                parent: None,
                value: None,
            },
        );
        apply(
            &mut state,
            &Command::StartRun {
                run: 0,
                at: Tick(0),
            },
        );

        for (set, expect_waiting) in [(RunState::Paused, true), (RunState::Success, false)] {
            let mut s = state.clone();
            apply(&mut s, &Command::SetRunState { run: 0, state: set });
            let step = sequential::drive(&plan, &s, &roots, &[], None, Tick(1));
            assert_eq!(
                matches!(step, Step::Waiting),
                expect_waiting,
                "with run 0 in {set:?} the sequence should {}; got {step:?}",
                if expect_waiting { "wait" } else { "move on" }
            );
        }
    }

    /// **A skip writes the state AND the attempt Kestra records with it.**
    ///
    /// Measured on `valids/task-runif`'s `notexecuted`: the run reads
    /// `["CREATED","SKIPPED"]` and carries **one** attempt reading the same two
    /// states — Java's `new State()` starts at `CREATED` and `.withState(SKIPPED)`
    /// appends. A `SetRunState { Skipped }` cannot produce that, and this test
    /// asserts the difference rather than describing it.
    #[test]
    fn skipping_a_run_records_the_attempt_kestra_records() {
        let plan = Plan::default();
        let mut state = ExecutionState::new(&plan);
        apply(
            &mut state,
            &Command::CreateRun {
                task: 0,
                parent: None,
                value: None,
            },
        );
        apply(
            &mut state,
            &Command::SkipRun {
                run: 0,
                at: Tick(7),
            },
        );
        assert_eq!(
            state.runs[0].history,
            vec![RunState::Created, RunState::Skipped]
        );
        assert_eq!(state.runs[0].attempts.len(), 1);
        assert_eq!(
            state.runs[0].attempts[0].history,
            vec![RunState::Created, RunState::Skipped]
        );
        assert_eq!(state.runs[0].attempts[0].started_at, Tick(7));

        // The control: the shape a FAILED render leaves, which is the other
        // thing a `runIf:` can do and is a different trace -- no attempt at all.
        // `valids/task-runif`'s `willfailedtheflow` is that one.
        let mut other = ExecutionState::new(&plan);
        apply(
            &mut other,
            &Command::CreateRun {
                task: 0,
                parent: None,
                value: None,
            },
        );
        apply(
            &mut other,
            &Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
        );
        assert!(
            other.runs[0].attempts.is_empty(),
            "a run terminated without being started has no attempt; if this ever \
             grows one, `valids/task-runif`'s `willfailedtheflow` stops being \
             reproducible"
        );
    }
}
