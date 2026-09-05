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
/// **That union is 13 states**, measured over `golden.json` by
/// `oracle-exec/normalise.py`; this enum holds **7** of them. Task 3 added
/// `Submitted` to the six the plan drafted, and that one addition is what makes
/// the sequential slice reachable at all: measured over the 266 `runs` entries,
/// **35** use only the drafted six and **229** use only these seven. The six
/// still missing, with the number of `runs` entries whose trace uses each:
/// `PAUSED` 18, `RESTARTED` 12, `WARNING` 10, `SKIPPED` 4, `RETRIED` 4,
/// `CANCELLED` 3. `oracle-exec/README.md` prints the full table and names which
/// later task owns each.
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
    Killed,
}

impl RunState {
    /// Kestra's spelling, which is what the golden holds.
    pub fn wire(self) -> &'static str {
        match self {
            RunState::Created => "CREATED",
            RunState::Submitted => "SUBMITTED",
            RunState::Running => "RUNNING",
            RunState::Success => "SUCCESS",
            RunState::Failed => "FAILED",
            RunState::Retrying => "RETRYING",
            RunState::Killed => "KILLED",
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
            RunState::Success | RunState::Failed | RunState::Killed
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
        Command::ScheduleRetry { run, not_before } => {
            state.runs[*run].not_before = Some(*not_before);
        }
        Command::SetRunState { run, state: s } => {
            let r = &mut state.runs[*run];
            r.history.push(*s);
            if let Some(a) = r.attempts.last_mut() {
                a.history.push(*s);
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
    }
}
