//! One module per flowable, because each is an independent semantic unit with
//! its own corpus slice — and because a single `executor.rs` is how
//! `dataapi/src/lifecycle.rs` reached 12,091 lines.
//!
//! Task 3 shipped `sequential`; Task 4 added `parallel` and `dag` beside it;
//! Task 5 adds `branch` (`If` and `Switch`). Tasks 6-8 add `loop_` and the
//! attempt lifecycle.
//!
//! What lives *here* rather than in one of them is the part every task list
//! shares whatever its scheduling rule: the answer type, how a run is looked
//! up, how a run is started, and how a `Running` child is descended into. Task
//! 3 had one flowable and so kept all four in `sequential.rs`; with three they
//! would otherwise be three copies, and three copies of "a `Runnable` gets
//! `SUBMITTED` first" is three chances for one of them to stop being true.

pub(crate) mod branch;
pub(crate) mod dag;
pub(crate) mod parallel;
pub(crate) mod sequential;

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx, TaskKind};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

/// What one task list wants from the caller.
///
/// Three answers, not two, because "nothing to emit" and "finished" are
/// different: the first means a worker is holding a task and the executor must
/// wait, the second means the list has a terminal state to report upward. The
/// oracle's environment distinguishes them too — an execution with nothing to
/// do and nothing in flight that has *not* finished is `STUCK`, a broken
/// instrument rather than a wrong answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Work for the caller to apply. Never empty.
    Commands(Vec<Command>),
    /// A task is in flight; the executor has nothing to decide until it lands.
    Waiting,
    /// Every child reached a terminal state, and this is the list's own.
    Done(RunState),
}

/// The run a list created for `task` under `parent`, if it has created one.
///
/// Every list this executor drives creates at most one run per child — a
/// `Loop`, which does not, spawns sub-executions instead (Task 6) — so the
/// first match is the only match.
pub(crate) fn run_of(state: &ExecutionState, parent: Option<RunIx>, task: TaskIx) -> Option<RunIx> {
    (0..state.runs.len()).find(|&ix| state.runs[ix].parent == parent && state.runs[ix].task == task)
}

/// This run's latest state, or `None` if it somehow has no history.
pub(crate) fn latest(state: &ExecutionState, run: RunIx) -> Option<RunState> {
    state.runs[run].history.last().copied()
}

/// Whether this run has reached a terminal state.
pub(crate) fn is_terminal(state: &ExecutionState, run: RunIx) -> bool {
    latest(state, run).is_some_and(RunState::is_terminal)
}

/// Start a created run.
///
/// **A `Runnable` gets `SUBMITTED` first and a flowable does not**, and that is
/// measured rather than stylistic: on `valids/parallel`, `parent` (the
/// `Parallel`) goes `CREATED RUNNING SUCCESS` while each of `t1`..`t6` goes
/// `CREATED SUBMITTED RUNNING SUCCESS`. `SUBMITTED` is in 580 of the golden's
/// 840 run histories and is the only thing in Kestra's record that separates a
/// task the worker ran from a flowable the executor ran, so a state machine
/// that omits it reproduces neither.
///
/// It is a separate `SetRunState` rather than something
/// [`apply`](crate::executor::apply) infers from `StartRun`, because `apply`
/// does not see the [`Plan`] and so cannot tell the two apart — and because
/// `apply` carrying that inference would be `apply` taking a decision, which is
/// exactly what it is documented not to do.
pub(crate) fn start(plan: &Plan, run: RunIx, task: TaskIx, now: Tick) -> Vec<Command> {
    let mut cmds = Vec::new();
    if matches!(plan.tasks[task].kind, TaskKind::Runnable { .. }) {
        cmds.push(Command::SetRunState {
            run,
            state: RunState::Submitted,
        });
    }
    cmds.push(Command::StartRun { run, at: now });
    cmds
}

/// Drive the flowable that a **`Running`** child run is, and turn its `Done`
/// into the command that terminates that child.
///
/// A `Runnable` is [`Step::Waiting`]: the worker is holding it and the executor
/// has nothing to decide until it lands.
///
/// # Recursion, and why there is no depth guard here
///
/// This descends only through a child that is **already `Running`**, and a
/// running child is a run in `state.runs`. Each level therefore descends from
/// one existing run to a *different* existing run — the next level searches for
/// runs whose `parent` is the level above, and a run's parent is fixed at
/// creation to a run that already existed, so parent indices strictly decrease
/// going up. Recursion depth is at most `state.runs.len()`, for **any** `Plan`,
/// including a hand-built cyclic one.
///
/// A `if depth > state.runs.len()` guard would therefore be unreachable code —
/// a guard nobody could ever see fire, which this package has already paid for
/// twice. The bound over *author* input, which is the one the programme's
/// constraint is about, is enforced once at
/// [`build_plan`](crate::executor::build_plan).
pub(crate) fn descend(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    let step = match &plan.tasks[task].kind {
        TaskKind::Runnable { .. } => return Step::Waiting,
        TaskKind::Sequential { tasks } => sequential::drive(plan, state, tasks, Some(run), now),
        TaskKind::Parallel { tasks, concurrency } => {
            parallel::drive(plan, state, tasks, Some(run), *concurrency, now)
        }
        TaskKind::Dag { nodes } => dag::drive(plan, state, nodes, Some(run), now),
        TaskKind::If { then_, else_, .. } => branch::drive_if(plan, state, run, then_, else_, now),
        TaskKind::Switch { cases, default, .. } => {
            branch::drive_switch(plan, state, run, cases, default, now)
        }
    };
    match step {
        Step::Done(s) => Step::Commands(vec![Command::SetRunState { run, state: s }]),
        other => other,
    }
}
