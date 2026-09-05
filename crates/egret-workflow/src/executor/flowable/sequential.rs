//! `Sequential`, and the implicit root task list.
//!
//! Kestra's `Sequential` and a flow's top-level `tasks:` are the same rule:
//! run the children one at a time, in declaration order, and stop at the first
//! one that does not succeed. The root list is that rule with no `PlanTask` of
//! its own, which is why [`drive`] takes the child list and the parent run
//! rather than a task index.

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

/// Drive one sequential task list.
///
/// `children` is the list, `parent` the run that owns it (`None` for the root
/// list). Returns the first thing that needs doing, or the list's terminal
/// state.
///
/// # Recursion, and why there is no depth guard here
///
/// [`drive`] recurses only through a child that is **already `Running`**, and a
/// running child is a run in `state.runs`. Each level therefore descends from
/// one existing run to a *different* existing run — the next level searches for
/// runs whose `parent` is the level above, and a run's parent is fixed at
/// creation to a run that already existed, so parent indices strictly decrease
/// going up. Recursion depth is therefore at most `state.runs.len()`, for
/// **any** `Plan`, including a hand-built cyclic one.
///
/// A `if depth > state.runs.len()` guard would therefore be unreachable code —
/// a guard nobody could ever see fire, which this package has already paid for
/// twice. The bound over *author* input, which is the one the programme's
/// constraint is about, is enforced once at
/// [`build_plan`](crate::executor::build_plan).
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Step {
    for &task in children {
        let Some(ix) = run_of(state, parent, task) else {
            // The previous child terminated successfully (or this is the
            // first), so the next one is created — and only now. Creating the
            // whole list up front would make every pair `overlapping` where
            // Kestra records `before`.
            return Step::Commands(vec![Command::CreateRun {
                task,
                parent,
                value: None,
            }]);
        };

        match state.runs[ix].history.last() {
            Some(RunState::Created) => return Step::Commands(start(plan, ix, task, now)),
            Some(RunState::Running) => match &plan.tasks[task].kind {
                // A flowable the executor drives: descend, and terminate it
                // when its own children are done.
                TaskKind::Sequential { tasks } => {
                    return match drive(plan, state, tasks, Some(ix), now) {
                        Step::Done(state) => {
                            Step::Commands(vec![Command::SetRunState { run: ix, state }])
                        }
                        other => other,
                    }
                }
                // A task the worker holds. Nothing to decide until it lands.
                TaskKind::Runnable { .. } => return Step::Waiting,
            },
            Some(s) if s.is_terminal() => {
                if *s != RunState::Success {
                    // The sequence stops at the first child that does not
                    // succeed, and the list takes that child's state. The
                    // remaining children are never created, which is why a
                    // flow whose second task fails has fewer runs than it has
                    // tasks.
                    return Step::Done(*s);
                }
            }
            // `Submitted` (handed to a worker, not picked up) and `Retrying`
            // (Task 7) are both "in flight, not the executor's turn".
            _ => return Step::Waiting,
        }
    }

    Step::Done(RunState::Success)
}

/// The run this list created for `task`, if it has created one.
///
/// A sequential list creates at most one run per child, so the first match is
/// the only match.
fn run_of(state: &ExecutionState, parent: Option<RunIx>, task: TaskIx) -> Option<RunIx> {
    (0..state.runs.len()).find(|&ix| state.runs[ix].parent == parent && state.runs[ix].task == task)
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
fn start(plan: &Plan, run: RunIx, task: TaskIx, now: Tick) -> Vec<Command> {
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
