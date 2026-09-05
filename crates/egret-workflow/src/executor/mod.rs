//! `advance()` — the pure state machine that decides what an execution does
//! next.
//!
//! # This module is a STUB
//!
//! W3a Task 2 creates it so that the differential oracle
//! (`tests/executor.rs`, `oracle-exec/`) has something to drive and a shape to
//! be judged against. [`advance`] returns `vec![]` and [`build_plan`] returns
//! an empty [`Plan`] for every input, so **every** comparable golden entry
//! mismatches and every one of them is in `oracle-exec/baseline.txt`. That is
//! the starting debt; Tasks 3-8 bank it a flowable at a time.
//!
//! The types below are transcribed from the plan's Task 3 section so the stub
//! can carry the real signature. Task 3 moves them into `plan.rs`, `state.rs`
//! and `command.rs` unchanged; it does not get to redesign them here first.
//!
//! # No I/O, no clock
//!
//! Nothing in `executor/` may reach a database, the network or the wall clock
//! (spec §6.5, and the plan's Global Constraints). `now` arrives as a
//! parameter, spelled [`Tick`] rather than `std::time::Instant` because an
//! `Instant` cannot be constructed at an arbitrary value and every retry and
//! timeout test would then need a real clock — in the one module whose whole
//! point is that it has none.

use std::time::Duration;

/// An index into [`Plan::tasks`].
pub type TaskIx = usize;

/// An index into [`ExecutionState::runs`].
pub type RunIx = usize;

/// The published, reference-resolved flow. Built once, never mutated.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Plan {
    /// Flattened; children are addressed by index.
    pub tasks: Vec<PlanTask>,
    /// The flow's top-level `tasks:`.
    pub roots: Vec<TaskIx>,
    /// The flow's top-level `errors:`.
    pub errors: Vec<TaskIx>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanTask {
    /// The author's task id, as written.
    pub id: String,
    pub kind: TaskKind,
    pub retry: Option<RetryPolicy>,
    pub timeout: Option<Duration>,
    /// This task's own `errors:`.
    pub errors: Vec<TaskIx>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Runnable { type_id: String },
    Sequential { tasks: Vec<TaskIx> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Kestra spells it `maxAttempts`, measured.
    pub max_attempts: u32,
    /// A span, not a clock read.
    pub interval: Duration,
    pub max_duration: Option<Duration>,
    pub behavior: RetryBehavior,
    pub warning_on_retry: bool,
}

/// Wire spellings `RETRY_FAILED_TASK` / `CREATE_NEW_EXECUTION`, measured from
/// the eight `valids/retry-*.yml` flows Task 1 recovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryBehavior {
    RetryFailedTask,
    CreateNewExecution,
}

/// Milliseconds since the execution was created. See this module's doc for why
/// this is not `std::time::Instant`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Tick(pub i64);

/// The snapshot [`advance`] reads. Owned by the caller.
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
    pub value: Option<String>,
    /// The caller's render of this task's expression — an `If` condition or a
    /// `Switch` value. `advance()` is pure and cannot render, so it reads the
    /// result here. `None` means "not rendered yet", which is not an error.
    pub rendered: Option<String>,
    pub history: Vec<RunState>,
    pub attempts: Vec<Attempt>,
    pub not_before: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub history: Vec<RunState>,
}

/// The states a run passes through — **and, through
/// [`Command::SetExecutionState`], the states an execution passes through too**.
/// One type for both, so the vocabulary it has to cover is the union of both.
///
/// **That union is 13 states and this enum has 6**, and it is deliberately left
/// as the plan drafted it: Task 3 owns this type. Measured over `golden.json`,
/// the seven it cannot represent are `SUBMITTED` (on every task the *worker*
/// ran, never on a flowable, which the executor runs), `PAUSED`, `RETRIED`,
/// `WARNING`, `SKIPPED`, `CANCELLED`, and `RESTARTED` — which occurs 60 times
/// in **execution** histories and not once in a run history, and would have
/// been missed by looking at run states alone.
///
/// `oracle-exec/README.md` prints the measured vocabulary with counts. Until
/// this enum covers more of it, every entry whose trace uses a missing state is
/// unreachable — which is one reason the baseline starts at everything. It is
/// not the whole reason: **35 of the 266 `runs` entries use only these six**, so
/// zero is reachable today on a real trace, and adding `SUBMITTED` alone takes
/// that to 229.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Created,
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

/// What [`advance`] asks the caller to do. The caller does the I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CreateRun {
        task: TaskIx,
        parent: Option<RunIx>,
        value: Option<String>,
    },
    StartRun {
        run: RunIx,
    },
    /// The plan's draft spells this `Instant`; it is a [`Tick`] for the reason
    /// in this module's doc, and the divergence is recorded there rather than
    /// left for a reader to wonder about.
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
}

/// Why a flow cannot be planned — the refusal the `invalids` corpus is about.
///
/// Task 8 asserts on the message per group, so that a refusal which becomes a
/// *different* refusal is visible instead of silently still passing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanError {
    pub message: String,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlanError {}

/// Build a [`Plan`] from a flow's YAML source, or refuse it.
///
/// **Stub.** Returns an empty plan for every input, including the 28 flows the
/// pinned Kestra refuses — so the oracle's over-acceptance count starts at 28,
/// which is Task 8's starting debt written down rather than estimated.
pub fn build_plan(_source: &str) -> Result<Plan, PlanError> {
    Ok(Plan::default())
}

/// Decide what this execution does next.
///
/// **Stub.** Returns no commands, so an execution driven by it never leaves
/// `[Created]`. Task 3 implements the frontier and `sequential`; Tasks 4-8 add
/// a flowable each.
pub fn advance(_plan: &Plan, _state: &ExecutionState, _now: Tick) -> Vec<Command> {
    Vec::new()
}

/// Record one [`Command`] against the state.
///
/// `advance()` decides; this records. It is in the crate rather than in the
/// oracle test because the oracle is a *caller* and W3b's worker will be
/// another, and two callers writing their own bookkeeping is two chances for
/// the thing being compared to differ from the thing that ships.
///
/// It contains no decision: every branch is a mechanical write of what the
/// command already says.
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
        Command::StartRun { run } => {
            let r = &mut state.runs[*run];
            r.history.push(RunState::Running);
            r.attempts.push(Attempt {
                history: vec![RunState::Created, RunState::Running],
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
    }
}
