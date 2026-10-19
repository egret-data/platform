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
    /// **Who ends this run's wait** — `None` for a run that has never been
    /// paused. W8 Task 4 fix round 2, and it is a field rather than a rule.
    ///
    /// See [`Resume`] for the defect it closes. Written only by
    /// [`Command::PauseRun`](super::Command::PauseRun) and cleared only by
    /// [`Command::RestartLoop`](super::Command::RestartLoop), which is the one
    /// command that takes a run out of `PAUSED` without terminating it.
    pub resume_with: Option<Resume>,
    /// Where a `WorkingDirectory` block's own prepare and finish claims stand
    /// (spec §5.1). `None` on every run that is not such a block. A claim is
    /// work the worker does for the block, not a run of its own, so the
    /// history stays Kestra's and this field is outside it.
    pub working_dir: Option<WorkingDirPhase>,
}

/// Where a `WorkingDirectory` block's claims stand (spec §5.1, §5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingDirPhase {
    /// The prepare claim is with the worker, and no child may start.
    Preparing,
    /// Prepared: the children run in order.
    Prepared,
    /// The finish claim is with the worker.
    Finishing,
    /// Finished: the block's terminal state is the children's aggregate.
    Finished,
}

impl WorkingDirPhase {
    /// The column's spelling in `workflow.task_runs.working_dir`.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Preparing => "PREPARING",
            Self::Prepared => "PREPARED",
            Self::Finishing => "FINISHING",
            Self::Finished => "FINISHED",
        }
    }

    /// The inverse of [`WorkingDirPhase::wire`]; `None` for any other spelling,
    /// which the column's CHECK already refuses.
    pub fn parse(wire: &str) -> Option<Self> {
        Some(match wire {
            "PREPARING" => Self::Preparing,
            "PREPARED" => Self::Prepared,
            "FINISHING" => Self::Finishing,
            "FINISHED" => Self::Finished,
            _ => return None,
        })
    }
}

/// **How a `PAUSED` run leaves `PAUSED`.** W8 Task 4 fix round 2.
///
/// # The defect this exists to make unrepresentable
///
/// [`RunState::Paused`] is a unit variant and
/// [`Command::PauseRun`](super::Command::PauseRun) recorded no owner, so
/// "which construct ends this wait" lived in **two hand-written lists of
/// `TaskKind`s** — one in [`sequential::walk`](super::flowable::sequential),
/// one in [`parallel::drive_concurrent`](super::flowable::parallel) — each
/// behind a catch-all answering *wait* for everything else.
///
/// **Measured, not feared**: a real `PauseRun` applied to a run of any third
/// kind leaves it at `["CREATED","SUBMITTED","RUNNING","PAUSED"]` for ever —
/// 20 rounds past its deadline, **0 commands emitted** — under `Sequential`,
/// `Parallel` *and* `Dag`, while a `Pause` in the same fixture moves. Nothing
/// failed. W8 Task 4 shipped exactly that defect once (a `LoopUntil` under a
/// `Parallel`) and fix round 1 repaired the instance by adding a row to both
/// lists, **which is the same shape again**.
///
/// Carrying the answer on the command is what changes the shape: the resume
/// side reads the RUN and never the kind, the two lists collapse to one site,
/// and a construct that pauses **cannot fail to say who resumes it**, because
/// it is a field the compiler requires and not a list to remember.
///
/// **This changes `advance()`'s command vocabulary**, which the W8 plan's
/// Global Constraints call a contract change and therefore a finding rather
/// than a step. It is ruled in scope for fix round 2 on the same standard that
/// opened `src/models/`: say exactly what it buys. It buys the deletion of a
/// silent-stall class that three enumeration methods and two review rounds did
/// not see, and `advance()` stays pure over `(plan, state, now)`.
///
/// # The deadline is carried here and stored once
///
/// Both bounded variants carry the instant the wait ends;
/// [`apply`](super::apply) writes it to [`TaskRun::not_before`], which is the
/// one field `worker::claim::reopen_due` and `oracle-exec/driver.rs::pass_time`
/// read — both with no state term at all. Migration 0141 stores only the
/// discriminant, so there is one clock rather than two that can disagree, and
/// its CHECK makes the pairing structural: `Never` has no deadline and the
/// others must have one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// **Nothing this executor holds ends this wait** — a `Pause` with neither
    /// `pauseDuration:` nor `timeout:`.
    ///
    /// Measured on the pinned artifact: seven corpus flows are written that
    /// way and every one of them "reached PAUSED and stayed there for the full
    /// 90s", because such a pause ends only on an explicit resume and a
    /// standalone harvest issues none. **None of the seven is in
    /// `golden.json`**, so a `Pause` that resumed itself by some default would
    /// leave every figure in `baseline.txt` unchanged — which is why this is a
    /// variant with a name rather than a `None` that reads like an omission.
    Never,
    /// At `at`, put the run into `state` — a bounded `Pause`.
    ///
    /// The terminal is decided by WHICH bound was written and is resolved once,
    /// when the pause is entered: `pauseDuration:` ends `SUCCESS` and
    /// `timeout:` ends `FAILED` (`valids/pause-delay` and
    /// `valids/pause-timeout`, two otherwise identical flows).
    Terminal { at: Tick, state: RunState },
    /// At `at`, start the next iteration — a `LoopUntil`.
    ///
    /// It resumes into another `RUNNING` rather than into a terminal, which is
    /// visible in the execution's own history: of the golden's 18 `PAUSED`
    /// executions, 12 carry a `RESTARTED` (all `Pause`) and 6 do not (all
    /// `LoopUntil`).
    Iterate { at: Tick },
}

impl Resume {
    /// The instant this wait ends, or `None` for [`Resume::Never`].
    pub fn at(&self) -> Option<Tick> {
        match self {
            Resume::Never => None,
            Resume::Terminal { at, .. } | Resume::Iterate { at } => Some(*at),
        }
    }

    /// The discriminant migration 0141 stores. The instant is NOT in it — see
    /// this enum's doc.
    pub fn wire(&self) -> String {
        match self {
            Resume::Never => "NEVER".to_string(),
            Resume::Iterate { .. } => "ITERATE".to_string(),
            Resume::Terminal { state, .. } => format!("TERMINAL:{}", state.wire()),
        }
    }
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
    /// When this attempt reached a terminal state, on the same clock as
    /// [`Self::started_at`]; `None` while it is open, and for an attempt
    /// written before migration 0183.
    ///
    /// **[`apply`] never writes it, and [`advance`](super::advance) never
    /// reads it.** It is a fact about when the environment committed the end,
    /// not one of the executor's decisions: `worker::stamps::
    /// stamp_attempt_ends` sets it in the settle that ended the attempt, so
    /// the `UPDATE` that writes the attempt's terminal state writes its end
    /// too (W7b Task 1). It is carried here, rather than only in the jsonb,
    /// because `persist` re-serialises `attempts` whole and would otherwise
    /// drop it on the run's next write.
    pub ended_at: Option<Tick>,
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
/// measured examples — and **on the day Task 2 shipped** nothing in this crate
/// emitted either new variant: the constructs that do were `Pause` and
/// `LoopUntil`, both planned opaque by
/// [`build_plan`](super::plan::build_plan) at that commit. The variants were
/// added together because those two constructs need the **same** one, so
/// splitting them across packages would have blocked each on the other's state.
///
/// **That paragraph is kept in the past tense it was always about.** Its claim
/// is what Task 2's sixteen bought *on the day*, which is still nothing; its
/// present-tense form — *"both still planned opaque"* — went on being read as a
/// fact about HEAD and was false from W8 Task 3 onward. It is one of five
/// claims of that class the Task 4 fix-round-2 sweep corrected, and the
/// instrument that finds them is a question rather than a grep: **does this
/// claim have a program-checked twin?** This one's twins are
/// `a_pause_enters_paused_without_putting_it_on_the_attempt` and
/// `a_falsy_condition_pauses_for_the_interval_and_runs_the_body_again`, one per
/// producer.
///
/// **Both producers exist now** — `Pause` at W8 Task 3 (eight banked entries)
/// and `LoopUntil` at W8 Task 4 (one) — and they leave the state by different
/// doors, which is what `advance`'s `pause_mark` has three arms for: a resumed
/// `Pause` owes the execution `RESTARTED` and a resumed `LoopUntil` owes it
/// `RUNNING`. Measured: of the golden's 18 `PAUSED` execution histories, 12
/// carry a `RESTARTED` and 6 do not, and the 6 are exactly `LoopUntil`'s.
///
/// **`WARNING` arrived at W8 Task 5 and `CANCELLED` at W8 Task 6, which is the
/// LAST of the golden's thirteen**: the vocabulary this enum holds and the
/// vocabulary the golden uses are now the same set, and
/// `tests/executor.rs::the_state_vocabulary_this_enum_holds_is_derived_from_the_golden`
/// is what says so rather than this sentence.
/// `oracle-exec/README.md` prints the full table and names which later task owns
/// each.
///
/// **That absence was not inert, and W8 Task 4 fix round 3 handed the instance
/// to Task 5.** While there was no `Warning` variant, a `WARNING` child did not
/// reach `is_terminal()` at all — `worker::from_wire` could not decode it — so
/// `flowable::loop_until::children_executed` answered *no* for ever and the
/// loop paused to its bound. `valids/waitfor-child-task-warning` is that flow,
/// and this sentence said it was **already banked**, which in this crate's
/// vocabulary means the opposite of what it meant: a banked entry is one the
/// ratchet finds MATCHING, and the entry is in `baseline.txt`.
/// `loop_until.rs`'s copy of the same paragraph had it right (*"already in
/// `baseline.txt`"*) and this one was read from it.
///
/// **W8 Task 5 closed both halves and the entry is still baselined.** Its
/// `WARNING` child is byte-identical to the golden and `children_executed`
/// lets it through, so the loop reaches its SECOND child for the first time —
/// an `io.kestra.plugin.core.output.OutputValues` with no handler — and stalled
/// there. Measured by unbaselining it, not inferred. *(Since no-silent-stall
/// Task 1 `OutputValues` is `unknown_task_type`, so the entry is refused at
/// start and never reaches that child; a stored execution of it that does
/// fails the run with `WorkerError::NoHandler`'s line, spec §5.)*
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
    /// **Non-terminal: a kill has been asked for and has not yet landed.**
    /// Kestra's `State.Type.KILLING`, which `isTerminated()` does not list and
    /// `isRunning()` does — the execution, or the run, is still live.
    ///
    /// Written by the kill route (on the execution, and on its leased runs)
    /// and by `advance()` (on flowable runs, when it closes a `KILLING`
    /// execution). It is left by one transition only: to [`RunState::Killed`].
    /// It is **not** `RUNNING`, so the normal claim (`state = 'RUNNING'`) does
    /// not hand a `KILLING` run out; it is not paused and it is not a failure.
    ///
    /// Slot 13 — appended to [`RunState::ALL`] rather than inserted beside
    /// `KILLED`, so no existing variant's slot moves.
    Killing,
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
    /// **The terminal `allowFailure: true` leaves behind, and it is terminal in
    /// the BENIGN half** — W8 Task 5, which is what took this enum from 11 of
    /// the golden's 13 states to **12**. `CANCELLED` was the one that
    /// remained, and W8 Task 6 added it: the enum is complete.
    ///
    /// **Terminal, and on which side of `isTerminatedNoFail` it falls is
    /// measured rather than inferred from the name.** Decompiled from
    /// `/app/kestra` in `kestra/kestra@sha256:28f59bf1…`,
    /// `io/kestra/core/models/flows/State.java`: `isTerminated()` is
    /// `FAILED || WARNING || SUCCESS || KILLED || CANCELLED || RETRIED ||
    /// SKIPPED || RESUBMITTED` and `isTerminatedNoFail()` is
    /// `WARNING || SUCCESS || RETRIED || SKIPPED || RESUBMITTED`. So `WARNING`
    /// is terminal AND not a failure, which is two separate facts and both of
    /// them move something here: [`RunState::is_terminal`] gains it, and
    /// `flowable::loop_until::children_executed` — whose predicate is
    /// `isTerminatedNoFail` — must count a `WARNING` child as executed rather
    /// than stall on it.
    ///
    /// **It has exactly one producer and that is what keeps it from
    /// over-accepting.** `crate::executor::allow_failure::soften` is the only
    /// expression in this crate that returns it, and it returns it only for
    /// `(Failed, allow_failure = true, allow_warning = false)`. No handler can
    /// report it — `worker::from_wire` will now decode the wire spelling, but
    /// nothing egret runs emits one — and no aggregate invents one: a
    /// `WARNING` in [`flowable::aggregate`](super::flowable::aggregate) can
    /// only have come from a child run that already held it.
    ///
    /// **The golden's `WARNING` population is 10 run histories**, and this
    /// variant arriving banks four of them: `valids/task-allow-failure-`
    /// `{runnable,flowable,executable-flow}` and
    /// `valids/pause-timeout-allow-failure`. The other six are held by
    /// something else, entry by entry, in `oracle-exec/README.md`'s residual
    /// table — four by the `AllowFailure` FLOWABLE, which is a different
    /// construct from the `allowFailure:` flag and is still planned opaque, one
    /// (`valids/waitfor-child-task-warning`) by an
    /// `io.kestra.plugin.core.output.OutputValues` child `driver.rs` ships no
    /// handler for, and one (`valids/retry-success`) by `warningOnRetry`.
    Warning,
    /// **The terminal a violated `sla:` leaves on the EXECUTION** — W8 Task 6,
    /// which is what took this enum from 12 of the golden's 13 states to all
    /// **13**. Nothing is left outside it.
    ///
    /// **Terminal, and terminal in the FAILING half — both halves measured.**
    /// Decompiled from `/app/kestra` in `kestra/kestra@sha256:28f59bf1…`,
    /// `io/kestra/core/models/flows/State.java`: `isTerminated()` is `FAILED ||
    /// WARNING || SUCCESS || KILLED || CANCELLED || RETRIED || SKIPPED ||
    /// RESUBMITTED` — so it is terminal; `isTerminatedNoFail()` is `WARNING ||
    /// SUCCESS || RETRIED || SKIPPED || RESUBMITTED` — so it is **not** benign;
    /// and `isTerminatedInError()` is `FAILED || KILLED || CANCELLED` — so it is
    /// an error. Those are three predicates and not one, and each of them has a
    /// consumer in this crate that had to gain the row: [`is_terminal`], the
    /// new [`is_terminated_no_fail`] (which `flowable::loop_until::
    /// children_executed` reads), and `worker::spawn::terminated_in_error`.
    /// **That triple is the shape W8 Task 5 fix round 2 paid for** — adding
    /// `Warning` to `is_terminal` alone left a family of sibling predicates
    /// wrong — so this variant's arms were written by enumerating the family
    /// first. `flowable::aggregate`'s priority chain is the fourth consumer and
    /// is not a terminality predicate at all:
    /// `Execution.guessFinalState`'s single pass is `KILLED > FAILED >
    /// CANCELLED > WARNING > PAUSED` (`Execution.java:774-789`), so this
    /// variant sits **between** `Failed` and `Warning` there.
    ///
    /// **It has exactly one producer, and it is an EXECUTION state rather than
    /// a run state.** [`crate::sla`]'s `EXECUTION_ASSERTION` with `behavior:
    /// CANCEL` emits `SetExecutionState { Cancelled }`; nothing emits a
    /// `SetRunState { Cancelled }` and no handler can report one. A *run* can
    /// still hold it, by one road only — a child execution that ends
    /// `CANCELLED` is reported to the parent's `Subflow` run
    /// (`worker::subflow::on_child_terminal`) or to a `Loop`'s
    /// (`worker::spawn::on_execution_terminal`) — and **no corpus entry takes
    /// that road**: the only two flows whose `sla:` can cancel
    /// (`valids/sla-execution-condition`,
    /// `valids/flow-concurrency-sla-assert-cancel`) are named as a `Subflow`
    /// target by nothing. The predicates above are therefore correct on an
    /// input the corpus cannot supply, which is why each carries a unit row
    /// that supplies it.
    ///
    /// **The golden's `CANCELLED` population is 3 execution histories and 0 run
    /// histories**, and this variant arriving banks two of them:
    /// `valids/sla-execution-condition` and
    /// `valids/flow-concurrency-sla-assert-cancel`. The third,
    /// `valids/exit-cancelled`, is an `Exit` — `oracle-exec/README.md`'s
    /// `ExecutionUpdatableTask` row — and stayed baselined until kill-and-exit
    /// Task 3 (2026-10-17) implemented `Exit` (`executor::exit`) and banked it.
    Cancelled,
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
        RunState::Warning,
        RunState::Cancelled,
        RunState::Killing,
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
            RunState::Warning => 11,
            RunState::Cancelled => 12,
            RunState::Killing => 13,
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
            RunState::Warning => "WARNING",
            RunState::Cancelled => "CANCELLED",
            RunState::Killing => "KILLING",
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
                // W8 Task 5. `State.Type.isTerminated()` lists `WARNING`
                // second, beside `FAILED`; `isTerminatedNoFail()` lists it
                // first, beside `SUCCESS`. Both readings matter and they are
                // not the same predicate -- see `RunState::Warning`.
                | RunState::Warning
                // W8 Task 6. `isTerminated()` lists `CANCELLED` fifth;
                // `isTerminatedNoFail()` does NOT list it, which is what
                // `is_terminated_no_fail` below is for.
                | RunState::Cancelled
        )
    }

    /// True where `State.Type.isTerminatedNoFail()` is true for the same name —
    /// **terminal AND not an error**, which is a different question from
    /// [`is_terminal`](RunState::is_terminal) and has its own consumer.
    ///
    /// `WARNING || SUCCESS || RETRIED || SKIPPED || RESUBMITTED`, decompiled
    /// from `/app/kestra` in `kestra/kestra@sha256:28f59bf1…`,
    /// `io/kestra/core/models/flows/State.java`. Of those five this enum holds
    /// four; `RESUBMITTED` is not a `RunState` and never will be from this
    /// corpus (it appears in 0 golden histories).
    ///
    /// # Why it exists, and it is a structural answer to a measured class of bug
    ///
    /// `flowable::loop_until::children_executed` spelled this predicate as
    /// *the complement of `Failed | Killed` inside the terminals this enum
    /// holds*. A complement is right only for the enum it was written against:
    /// W8 Task 5's `Warning` happened to fall on the side that complement gave
    /// it, and W8 Task 6's `Cancelled` does **not** — it is terminal, it is not
    /// no-fail, and the complement would have called it executed. Writing
    /// Kestra's predicate down once, as a positive list with the same partition
    /// guard [`is_terminal`](RunState::is_terminal) has, is what stops the
    /// third variant repeating it. `the_two_state_vocabularies_agree` in
    /// `tests/executor.rs` cross-checks every variant of this against
    /// `models::State::is_terminated_no_fail`, which is a second, independently
    /// written copy of the same Java.
    ///
    /// **It implies terminality and that is asserted rather than assumed**: all
    /// five Java members are in `isTerminated()`, so
    /// `is_terminated_no_fail() => is_terminal()` holds for every variant, and
    /// `no_fail_is_a_subset_of_terminal` is the row that says so.
    pub fn is_terminated_no_fail(self) -> bool {
        matches!(
            self,
            RunState::Success | RunState::Warning | RunState::Retried | RunState::Skipped
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
/// **W6 measured a SECOND `Subflow` shape this rule is wrong for and left the
/// rule alone because nothing reachable could express it; W8 Task 5 made it
/// reachable and fixed it with a COMMAND rather than by widening this rule.**
/// When a `Subflow`'s child terminates, Kestra writes the CHILD's state onto
/// the parent run and then, if `ExecutableUtils.guessState` disagrees, writes
/// that too -- and the attempt takes **both**, because
/// `ExecutableUtils.subflowExecutionResult` rebuilds the attempt from the
/// parent taskrun's whole `State`. Measured on the pinned artifact,
/// `egret.probe/probe-notransmit-parent` (`transmitFailed: false` over a child
/// that FAILS) gives a run AND an attempt each reading
/// `["CREATED","RUNNING","FAILED","SUCCESS"]`, and the golden holds the same
/// shape for `valids/task-allow-failure-executable-flow` (`... FAILED,
/// WARNING`).
///
/// **What made it unreachable was three things at once and all three are
/// gone**: [`RunState`] had no `Warning`, `models::Task` carried neither
/// `allowFailure` nor `allowWarning`, and **no corpus `Subflow` writes
/// `transmitFailed: false`** -- the last is still true, re-derived over all 25
/// declarers by `tests/executor.rs`'s
/// `no_corpus_subflow_turns_transmit_failed_off`, and the other two changed at
/// W8 Task 5, so `allowWarning:` reaches the shape by a different road.
///
/// **The rule below is unchanged, and that is the decision.**
/// [`Command::CompleteRun`] carries the second state and pushes it onto the
/// attempt whether or not the attempt has closed; widening this rule instead
/// would be wrong for every retry, whose attempts each end `FAILED` with a
/// `RETRYING` that must not join them.
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
            resume_with: None,
            working_dir: None,
        }),
        Command::StartRun { run, at } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Running);
            r.attempts.push(Attempt {
                history: vec![RunState::Created, RunState::Running],
                started_at: *at,
                ended_at: None,
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
                ended_at: None,
            });
        }
        // `FAILED` and the one attempt Kestra records with it, in one write --
        // see `Command::FailUnstarted`. The attempt reads `[FAILED]`, not
        // `[CREATED, FAILED]`: the golden's failed updates
        // (`valids/set-variables-duplicate`) hold `[["FAILED"]]`, and a `Loop`
        // failing at `CREATED` (`valids/loop-null`, `[]`) is the shape this
        // command exists to NOT produce.
        Command::FailUnstarted { run, at } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Failed);
            r.attempts.push(Attempt {
                history: vec![RunState::Failed],
                started_at: *at,
                ended_at: None,
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
        Command::PauseRun { run, resume } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Paused);
            // **The deadline comes OUT of the `Resume`, so the two cannot
            // disagree.** It used to be a separate `until` field beside it,
            // which is two ways to say "this wait is unbounded" and one of them
            // silently winning. `Resume::Never` writes none, which is what
            // keeps the environment's clock from ever being asked to move for
            // a pause that waits on a human -- see `Command::PauseRun`.
            r.not_before = resume.at();
            r.resume_with = Some(*resume);
        }
        // W8 Task 5 -- the run and the attempt take DIFFERENT states, or the
        // same one after the attempt has closed. See `Command::CompleteRun`
        // for the two artifact shapes that need it and for why widening the
        // `SetRunState` arm below instead would be wrong for every retry.
        Command::CompleteRun {
            run,
            run_state,
            attempt,
        } => {
            let r = &mut state.runs[*run];
            r.history.push(*run_state);
            if let Some(a) = r.attempts.last_mut() {
                a.history.push(*attempt);
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
                r.resume_with = None;
                // A `WorkingDirectory` block is prepared again: Kestra's new
                // task run has no working directory yet. Left at `FINISHED`,
                // the next iteration's block took its aggregate at once and
                // its children never ran (task 2.3b).
                r.working_dir = None;
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
            // **Cleared with the deadline, not left behind.** The run is not
            // waiting on anything any more, and migration 0141's
            // `workflow_task_runs_resume_bound_matches` pairs the discriminant
            // with `not_before_ms` -- so a stale `ITERATE` beside a NULL
            // deadline is not merely untidy, it is a row Postgres refuses.
            r.resume_with = None;
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
        Command::BeginWorkingDirectory { run } => {
            state.runs[*run].working_dir = Some(WorkingDirPhase::Preparing);
        }
        Command::FinishWorkingDirectory { run } => {
            state.runs[*run].working_dir = Some(WorkingDirPhase::Finishing);
        }
        // A claim that completed moves the block on. The phase it is in says
        // which claim it was, so the worker needs to name no phase of its own;
        // a completion that arrives in any other phase changes nothing.
        Command::CompleteWorkingDirectoryClaim { run } => {
            let r = &mut state.runs[*run];
            r.working_dir = match r.working_dir {
                Some(WorkingDirPhase::Preparing) => Some(WorkingDirPhase::Prepared),
                Some(WorkingDirPhase::Finishing) => Some(WorkingDirPhase::Finished),
                other => other,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `KILLING` is a live state: held by `ALL`, not terminal, and its wire
    /// spelling round-trips through the one inverse the crate reads rows with.
    #[test]
    fn killing_is_a_live_state_that_round_trips() {
        assert!(RunState::ALL.contains(&RunState::Killing));
        assert!(!RunState::Killing.is_terminal());
        assert!(!RunState::Killing.is_terminated_no_fail());
        assert_eq!(RunState::Killing.wire(), "KILLING");
        assert_eq!(crate::worker::from_wire("KILLING"), Some(RunState::Killing));
    }

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

    /// **`RunState::wire()` is `models::State`'s serde spelling, for every
    /// `RunState`** -- W13a Task 3, a Task 2 review carry-over.
    ///
    /// Two derivations of one spelling meet in the Flow-trigger outbox:
    /// `scheduler::sync` writes `flow_listeners.states` from `models::State`
    /// through serde, and `claim::settle` matches `$state = ANY(states)`
    /// binding `RunState::wire()`. A disagreement would not be an error
    /// anywhere -- the `EXISTS` would simply never match, and a listener would
    /// go deaf to that state in silence. Nothing tied the two together before
    /// this test.
    ///
    /// The `RunState -> State` map is written out by hand and is exhaustive,
    /// so it is an independent third statement of which state is which: a
    /// round trip through `State`'s own `Deserialize` would accept a `wire()`
    /// that named the WRONG state as long as that state exists.
    #[test]
    fn run_state_wire_is_the_model_states_serde_spelling() {
        use crate::models::State;
        fn model(r: RunState) -> State {
            match r {
                RunState::Created => State::Created,
                RunState::Submitted => State::Submitted,
                RunState::Running => State::Running,
                RunState::Success => State::Success,
                RunState::Failed => State::Failed,
                RunState::Retrying => State::Retrying,
                RunState::Retried => State::Retried,
                RunState::Killed => State::Killed,
                RunState::Skipped => State::Skipped,
                RunState::Paused => State::Paused,
                RunState::Restarted => State::Restarted,
                RunState::Warning => State::Warning,
                RunState::Cancelled => State::Cancelled,
                RunState::Killing => State::Killing,
            }
        }
        for r in RunState::ALL {
            let serde = serde_json::to_value(model(*r)).expect("State serialises");
            assert_eq!(
                serde,
                serde_json::Value::String(r.wire().to_string()),
                "{r:?}: RunState::wire() and models::State's serde spelling disagree, \
                 so the outbox's `$state = ANY(flow_listeners.states)` cannot match it"
            );
        }
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
    /// ~~Five~~ ~~SIX~~ **SEVEN** of those eight are members of this enum and
    /// every one of them is asserted true; the six non-members of the list are
    /// asserted false. So a `Paused` quietly added to `is_terminal` fails, and
    /// so does a `Success` quietly removed. Only `RESUBMITTED` is left outside,
    /// and it appears in 0 golden histories.
    ///
    /// # It said FIVE while the enum held six, and the totality assertion is
    /// the fix
    ///
    /// W8 Task 5 added `Warning`, which `State.Type.isTerminated()` lists
    /// **second**, and did not add it here — so **the guard named for
    /// terminality was silent about the terminal that task introduced**.
    /// Measured in fix round 1: deleting `| RunState::Warning` from
    /// [`RunState::is_terminal`] kills **8 tests and not this one**.
    ///
    /// The row is added, and so is the thing that makes the omission
    /// impossible rather than merely fixed: **the two lists are asserted to
    /// PARTITION [`RunState::ALL`]**. Counting `ALL.len()` alone could not do
    /// it — 7 + 6 = 13 is true of the wrong thirteen too. **W8 Task 6 is the
    /// first variant to arrive with the partition already in place, and it is
    /// what made the omission visible at compile-run time rather than at review
    /// time**: `Cancelled` added to the enum and to neither list fails the
    /// coverage assert before any oracle runs.
    ///
    /// **Which of the three asserts catches what, corrected** — fix round 2's
    /// review measured this and the first version of the paragraph credited
    /// the wrong line:
    ///
    /// | the mistake | what fails |
    /// |---|---|
    /// | a variant in NEITHER list | the **coverage** assert (`named == all`) |
    /// | a variant in BOTH lists | the **pre-existing loops** — it is asserted terminal and non-terminal, and no `is_terminal` can satisfy both |
    /// | a variant TWICE in ONE list | the **disjointness** assert (`named.len() == 7 + 6`), 12 against 13 |
    ///
    /// So the disjointness line never fires for the case its name suggests. It
    /// is **not** dead: a duplicate inside one list reaches it, and that is a
    /// real edit — the obvious way to "add" a variant is to copy a neighbouring
    /// line, and copying without editing the name is what it catches.
    ///
    /// The wire spellings are here for the same reason `run_state_all_holds…`
    /// checks them: a variant whose `wire()` collides with another's is
    /// invisible to the golden.
    #[test]
    fn neither_paused_nor_restarted_is_terminal() {
        // `State.Type.isTerminated()`'s members that this enum holds. `Warning`
        // is the sixth, added in W8 Task 5 fix round 1; `Cancelled` is the
        // seventh and last, added in W8 Task 6 -- see this test's doc.
        const TERMINAL: [RunState; 7] = [
            RunState::Success,
            RunState::Failed,
            RunState::Warning,
            RunState::Killed,
            RunState::Cancelled,
            RunState::Retried,
            RunState::Skipped,
        ];
        const NOT_TERMINAL: [RunState; 7] = [
            RunState::Created,
            RunState::Submitted,
            RunState::Running,
            RunState::Retrying,
            RunState::Paused,
            RunState::Restarted,
            // `isTerminated()` does not list `KILLING`; `isRunning()` does.
            RunState::Killing,
        ];
        for state in TERMINAL {
            assert!(
                state.is_terminal(),
                "{state:?} is in State.Type.isTerminated() and must stay terminal"
            );
        }
        for state in NOT_TERMINAL {
            assert!(
                !state.is_terminal(),
                "{state:?} is NOT in State.Type.isTerminated() -- a run in it is \
                 still the executor's business"
            );
        }
        // **The two lists PARTITION the enum**, which is what stops the next
        // variant being omitted from both the way `Warning` was omitted from
        // the first. Disjointness is asserted beside coverage, and the table
        // on this test's doc says which of the three asserts each mistake
        // reaches -- a variant in BOTH lists is caught by the loops above,
        // not by the line below.
        let named: std::collections::BTreeSet<&str> = TERMINAL
            .iter()
            .chain(NOT_TERMINAL.iter())
            .map(|s| s.wire())
            .collect();
        let all: std::collections::BTreeSet<&str> =
            RunState::ALL.iter().map(|s| s.wire()).collect();
        assert_eq!(
            named.len(),
            TERMINAL.len() + NOT_TERMINAL.len(),
            "a variant is named TWICE IN ONE list -- which is what copying a \
             neighbouring line and forgetting to edit the name produces. A \
             variant in BOTH lists is caught by the two loops above instead"
        );
        assert_eq!(
            named, all,
            "every RunState variant must be on exactly one side of \
             State.Type.isTerminated(); this fails for a variant added to the \
             enum and to neither list -- which is how `Warning` slipped past \
             this guard in the commit that introduced it"
        );
        assert_eq!(RunState::Paused.wire(), "PAUSED");
        assert_eq!(RunState::Restarted.wire(), "RESTARTED");
        assert_eq!(
            RunState::ALL.len(),
            14,
            "the golden's vocabulary is 13 and this enum holds ALL of it \
             (11 until W8 Task 5 added `Warning`, 12 until W8 Task 6 added \
             `Cancelled`), plus `Killing`, which the kill route writes and the \
             golden never holds; \
             `tests/executor.rs::the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` \
             is what derives that rather than this line"
        );
    }

    /// **`isTerminatedNoFail` is a SUBSET of `isTerminated`, over every
    /// variant** -- the relation Java's two lists have and the one a
    /// hand-written pair can lose.
    ///
    /// Both directions are exercised, so it is not an assertion an
    /// answer-`false`-to-everything implementation passes: the subset arm is
    /// checked on all thirteen, and the four variants that are terminal and
    /// NOT no-fail are named, which is what would fail if
    /// [`RunState::is_terminated_no_fail`] were widened to the whole terminal
    /// set.
    #[test]
    fn no_fail_is_a_subset_of_terminal() {
        for s in RunState::ALL {
            if s.is_terminated_no_fail() {
                assert!(
                    s.is_terminal(),
                    "{s:?} is in State.Type.isTerminatedNoFail() and every one \
                     of those five is also in isTerminated()"
                );
            }
        }
        let failing: std::collections::BTreeSet<&str> = RunState::ALL
            .iter()
            .filter(|s| s.is_terminal() && !s.is_terminated_no_fail())
            .map(|s| s.wire())
            .collect();
        assert_eq!(
            failing,
            std::collections::BTreeSet::from(["CANCELLED", "FAILED", "KILLED"]),
            "`isTerminated() && !isTerminatedNoFail()` is exactly \
             `State.Type.isTerminatedInError()` -- `FAILED || KILLED || \
             CANCELLED` (`State.java:296`) -- and `worker::spawn::\
             terminated_in_error` spells that list out by hand. **This message \
             said \"this is the assertion that would notice\" and it would \
             NOT**: the comparison below is against a LITERAL written here, so \
             it pins one spelling to a third copy and never calls the other \
             function at all. Dropping `Cancelled` from `terminated_in_error` \
             survived this test 3 of 3 runs. \
             `worker::spawn::tests::the_two_spellings_of_terminated_in_error_agree` \
             is the one that notices, because it calls it"
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
