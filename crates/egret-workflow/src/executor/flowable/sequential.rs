//! `Sequential`, and the implicit root task list.
//!
//! Kestra's `Sequential` and a flow's top-level `tasks:` are the same rule:
//! run the children one at a time, in declaration order, and stop at the first
//! one that does not succeed. The root list is that rule with no `PlanTask` of
//! its own, which is why [`drive`] takes the child list and the parent run
//! rather than a task index.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{descend, run_of, start, Step};

/// Drive one sequential task list.
///
/// `children` is the list, `parent` the run that owns it (`None` for the root
/// list). Returns the first thing that needs doing, or the list's terminal
/// state.
///
/// Returning at the **first** thing is what makes this sequential, and it is
/// the one line of difference from
/// [`parallel`](super::parallel::drive_concurrent), which collects from every
/// branch before it returns.
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
            // A flowable the executor drives: descend, and terminate it when
            // its own children are done. A `Runnable` is `Waiting` — the worker
            // holds it and there is nothing to decide until it lands.
            Some(RunState::Running) => return descend(plan, state, ix, task, now),
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
