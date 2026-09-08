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

/// The snapshot [`advance`](super::advance) reads. Owned by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionState {
    /// CREATION order — the oracle compares this order.
    pub runs: Vec<TaskRun>,
    pub history: Vec<RunState>,
}

impl ExecutionState {
    /// A freshly created execution: no runs, history `[Created]`.
    pub fn new(_plan: &Plan) -> Self {
        Self {
            runs: Vec::new(),
            history: vec![RunState::Created],
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
/// **9** of them. Every number in this paragraph is derived on every run by
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
/// `Skipped`. `Submitted` is the addition that made the sequential slice
/// reachable at all; `Retried` buys exactly the four `retry-new-execution-*`
/// entries, none of which can bank, because the replacement execution a
/// `CREATE_NEW_EXECUTION` retry starts is not representable here at all.
///
/// The four still missing, with the number of `runs` entries whose trace uses
/// each: `PAUSED` 18, `RESTARTED` 12, `WARNING` 10, `CANCELLED` 3.
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
