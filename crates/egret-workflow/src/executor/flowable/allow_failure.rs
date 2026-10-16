//! `AllowFailure`: a `Sequential` whose failure is a warning, not a failure.
//!
//! Kestra's `AllowFailure` runs its children and its `errors:` exactly as a
//! `Sequential` does, and changes only the block's terminal state: a child that
//! failed makes the block `WARNING` (spec §3.1), and a child that warned is
//! `SUCCESS` when the block's own `allowWarning:` is set (spec §3.2).

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::sequential;
use super::Step;

/// Drive one `AllowFailure` block: its children and `errors:` as a
/// `Sequential` does, then its `finally:`, then its terminal.
///
/// The terminal is emitted as a [`Command::SetRunState`] here rather than
/// returned as a [`Step::Done`]. `flowable::descend` applies `allowFailure:` and
/// `allowWarning:` to every `Done` it is handed, and the block's `WARNING` for a
/// failed child is a literal that must not be softened again by the block's own
/// `allowWarning:` (measured on the image: `probe-failwarn`).
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    tasks: &[TaskIx],
    finally: &[TaskIx],
    now: Tick,
) -> Step {
    let errors = plan.tasks[task].errors.as_slice();
    let children = match sequential::drive(plan, state, tasks, errors, Some(run), now) {
        Step::Done(children) => children,
        other => return other,
    };
    if !finally.is_empty() {
        // The block's state is already decided, so `finally` only decides when
        // the block is done. A failing `finally:` child does not change it.
        if let step @ (Step::Commands(_) | Step::Waiting) =
            sequential::drive(plan, state, finally, &[], Some(run), now)
        {
            return step;
        }
    }
    let state = block_state(children, plan.tasks[task].allow_warning);
    Step::Commands(vec![Command::SetRunState { run, state }])
}

/// The block's terminal state, from the aggregate of its children.
///
/// `children` is what [`super::sequential::drive`] reports for the block's
/// `tasks:`. The block's `errors:` run once a child has failed and do not
/// change the answer: the block is `WARNING` whether or not an error handler
/// then succeeds (spec §3.1, measured on the image).
///
/// `KILLED`, `CANCELLED` and every non-warning terminal pass through, as they do
/// for a `Sequential`.
pub(crate) fn block_state(children: RunState, allow_warning: bool) -> RunState {
    match (children, allow_warning) {
        // A literal `WARNING`, not passed through `allowWarning:`: measured on
        // the image as `probe-failwarn`, a `FAILED` child under `allowWarning:
        // true` is still `WARNING` (spec §3.2).
        (RunState::Failed, _) => RunState::Warning,
        (RunState::Warning, true) => RunState::Success,
        _ => children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per §5.1 outcome, with both values of `allowWarning:` on every
    /// child state the flags can move. The rows are measured on the image
    /// (spec §5.1, `probe-*` flows): the failed-child row is `WARNING` with the
    /// flag off and on, and the warned-child row is the only one the flag
    /// changes.
    #[test]
    fn block_state_is_the_section_5_1_table() {
        let table = [
            (RunState::Success, false, RunState::Success),
            (RunState::Success, true, RunState::Success),
            (RunState::Failed, false, RunState::Warning),
            (RunState::Failed, true, RunState::Warning),
            (RunState::Warning, false, RunState::Warning),
            (RunState::Warning, true, RunState::Success),
            (RunState::Killed, false, RunState::Killed),
            (RunState::Killed, true, RunState::Killed),
            (RunState::Cancelled, false, RunState::Cancelled),
            (RunState::Cancelled, true, RunState::Cancelled),
        ];
        for (children, allow_warning, expected) in table {
            assert_eq!(
                block_state(children, allow_warning),
                expected,
                "children={children:?} allowWarning={allow_warning}"
            );
        }
    }
}
