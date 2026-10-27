//! `Sequential`, and the implicit root task list.
//!
//! Kestra's `Sequential` and a flow's top-level `tasks:` are the same rule:
//! run the children one at a time, in declaration order — and, when one of them
//! fails for good, run the `errors:` list instead. The root list is that rule
//! with no `PlanTask` of its own, which is why [`drive`] takes the child list
//! and the parent run rather than a task index.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{aggregate, current_list, descend, open, run_of, Current, Step};

/// Drive one sequential task list, its `errors:` and its `finally:`.
///
/// `children` is the `tasks:` list, `errors` its `errors:`, `finally` its
/// `finally:`, and `parent` the run that owns all three (`None` for the root
/// list). Returns the first thing that needs doing, or the list's terminal
/// state -- which is [`aggregate`] over `children` alone, even after `errors`
/// and `finally` have run (spec §0.2: a failing `finally` child changes
/// nothing).
///
/// Returning at the **first** thing is what makes this sequential, and it is
/// the one line of difference from
/// [`parallel`](super::parallel::drive_concurrent), which collects from every
/// branch before it returns.
///
/// # `parent.is_none()` is the flow's own list, and it answers one question
/// differently
///
/// [`Current::RetryPending`] — a child that failed and will be retried — leaves
/// a **flowable** running and gives the **execution** its state anyway. That is
/// not a convenience: `FlowableUtils.resolveState`'s empty arm returns a state
/// only `if (execution.hasFailedNoRetry(...))`, while
/// `ExecutorService.handleEnd`'s guard is `execution.isTerminated(currentTasks)`,
/// which is `0 == 0` for the same empty list. The two entries that pin the
/// difference are `valids/retry-failed` (root: the execution reads
/// `… FAILED RETRYING RUNNING FAILED …`, one `FAILED` per attempt) and
/// `valids/retry-flowable` (nested: the execution reads `… RETRYING RUNNING
/// RETRYING …` with no `FAILED` until the end, because the `Sequential` holding
/// the retried child never terminated).
///
/// `parent` is the flag rather than a separate boolean because it already
/// carries exactly that fact — a flowable's list always has `Some(run)` and the
/// flow's own list is the only caller that passes `None`.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    errors: &[TaskIx],
    finally: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Step {
    let is_flow = parent.is_none();
    let list = match current_list(plan, state, children, errors, finally, parent, now) {
        Current::Tasks(list) | Current::Errors(list) | Current::Finally(list) => list,
        Current::RetryPending if is_flow => return Step::Done(aggregate(state, children, parent)),
        Current::RetryPending => return Step::Waiting,
        Current::Ended => return Step::Done(aggregate(state, children, parent)),
    };
    match walk(plan, state, list, parent, now) {
        Some(step) => step,
        None => Step::Done(aggregate(state, children, parent)),
    }
}

/// Kestra's `FlowableUtils.innerResolveSequentialNexts` over one already-chosen
/// list: create the first child that has no run, drive the one that is going,
/// and otherwise move to the next.
///
/// `None` means the list is exhausted — every child has a run and every run is
/// terminal — which is the caller's cue to take the list's own state.
///
/// # It does NOT stop at a child that failed, and that is the artifact
///
/// Task 3's version returned `Step::Done(child_state)` at the first child that
/// did not succeed, and reproduced everything up to Task 6 because the only way
/// a list could hold a failed child was a failure that ended it. Java stops for
/// a different reason: `innerResolveSequentialNexts` walks to
/// `currentTasks.get(lastIndex + 1)` whatever the last one's state was, and what
/// ends the sequence is [`current_list`] switching the list out from under it.
/// The two readings agree on `FAILED` and disagree on every other non-success
/// terminal — a `KILLED` child leaves a Kestra sequence *running*, and
/// `RETRIED` (Task 7's new state) does too. Taking Java's reading is what keeps
/// the `errors:` list from stopping at its own first failure, which is a shape
/// the corpus does reach.
fn walk(
    plan: &Plan,
    state: &ExecutionState,
    list: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Option<Step> {
    for &task in list {
        let Some(ix) = run_of(state, parent, task) else {
            // The previous child terminated (or this is the first), so the next
            // one is created — and only now. Creating the whole list up front
            // would make every pair `overlapping` where Kestra records
            // `before`.
            return Some(Step::Commands(vec![Command::CreateRun {
                task,
                parent,
                value: None,
            }]));
        };

        match state.runs[ix].history.last() {
            // W8 Task 4. A run `Command::RestartLoop` threw away, reached by
            // the walk in list order — which is the whole point of it being
            // reached here rather than refilled in bulk. See
            // `Command::RecreateRun`.
            None => {
                return Some(Step::Commands(vec![Command::RecreateRun { run: ix }]));
            }
            Some(RunState::Created) => return Some(open(plan, state, ix, task, now)),
            // **The one door out of `PAUSED`, and it reads the RUN.** W8 Task
            // 4 fix round 2. This arm used to be two, each guarded on a
            // `TaskKind` -- `Pause` at W8 Task 3, `LoopUntil` at Task 4 fix
            // round 1 -- with everything else falling to the trailing arm and
            // stalling for ever. `super::resume_paused` asks
            // `TaskRun::resume_with`, which `Command::PauseRun` is required to
            // carry, so there is no list of pausing kinds here to be missing
            // from. See `executor::Resume`.
            //
            // It cannot be folded into the `Running` arm below: `descend` is
            // reached at `RUNNING` and ENTERS the pause, this is reached at
            // `PAUSED` and LEAVES it, and a single door would have to branch on
            // the state anyway.
            Some(RunState::Paused) => return Some(super::resume_paused(state, ix, now)),
            // A flowable the executor drives: descend, and terminate it when
            // its own children are done. A `Runnable` is `Waiting` — the worker
            // holds it and there is nothing to decide until it lands.
            Some(RunState::Running) => return Some(descend(plan, state, ix, task, now)),
            Some(s) if s.is_terminal() => {}
            // `Submitted` (handed to a worker, not picked up), `Retrying`
            // (waiting for its own delay) and `Restarted` are "in flight, not
            // the executor's turn". So is a run with no history at all, which
            // is `Command::RestartLoop`'s thrown-away descendant and has an arm
            // of its own above.
            //
            // **`Paused` is NOT in this arm any more, for any kind.** It left
            // for `TaskKind::Pause` at W8 Task 3, for `TaskKind::LoopUntil` at
            // W8 Task 4 fix round 1, and for EVERY kind at fix round 2 — the
            // arm above is unguarded, because `TaskRun::resume_with` answers
            // for the run rather than a list answering for the kind. **This
            // comment said "`LoopUntil` is still planned opaque, so nothing can
            // put one of its runs into `PAUSED` today", which its own task
            // commit had already falsified**, and it is one of three found by
            // asking of each corrected claim whether it has a program-checked
            // twin.
            //
            // `Restarted` is untouched and still reaches nothing: 0 of the
            // golden's 840 run histories hold it.
            _ => return Some(Step::Waiting),
        }
    }

    None
}

/// [`walk`], for [`parallel`](super::parallel) (and so `Dag`), whose `errors:`
/// and `finally:` lists are driven sequentially even though its `tasks:` list
/// is not — Kestra says so in as many words: "errors & finally must be run as
/// sequential tasks" (`FlowableUtils.resolveParallelNexts`).
pub(crate) fn walk_errors(
    plan: &Plan,
    state: &ExecutionState,
    list: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Option<Step> {
    walk(plan, state, list, parent, now)
}

/// [`walk`], for the flow's `afterExecution:` (end hooks #2b, spec D8): the
/// post-terminal phase walks its list one task at a time under no parent,
/// with no `errors:`/`finally:` and no list choice -- the execution's own
/// lists are over. `None` is "the list is exhausted".
pub(crate) fn drive_list(
    plan: &Plan,
    state: &ExecutionState,
    list: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Option<Step> {
    walk(plan, state, list, parent, now)
}
