//! `Dag` — [`parallel`](super::parallel) plus a dependency predicate.
//!
//! Everything that is not the predicate lives in
//! [`drive_concurrent`](super::parallel::drive_concurrent), which is also how
//! Kestra builds it: `Dag.resolveNexts` calls `FlowableUtils.resolveDagNexts`,
//! which calls `resolveParallelNexts` with a filter over the same stream.
//!
//! The two things a `Dag` can be *wrong* about — a `dependsOn` naming a task
//! that is not one of its nodes, and a cycle — are not here. They are refused
//! in [`build_plan`](crate::executor::build_plan), which is both the plan's
//! instruction and the artifact's shape: Kestra refuses both at publish time in
//! `DagTaskValidator`, which is why `invalids/dag-cyclicdependency` and
//! `invalids/dag-notexist-task` are `refused` entries in the golden rather than
//! executions. Resolving `dependsOn` to a [`TaskIx`] once, at plan-build time,
//! makes the run-time copy of that error unrepresentable.

use crate::executor::plan::{DagNode, Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{latest, Step};

/// Drive one `Dag`.
///
/// # The predicate is Kestra's, word for word — since W8 Task 5 fix round 2
///
/// A node is startable when every task it `dependsOn` has a run whose latest
/// state **is terminated**. Measured on the pinned artifact, not read from the
/// checkout: `javap -p -c io/kestra/core/runners/FlowableUtils.class`,
/// `lambda$resolveDagNexts$3` is `getState().isTerminated()` and nothing else
/// — four instructions — and `State.Type.isTerminated()` is
/// `FAILED || WARNING || SUCCESS || KILLED || CANCELLED || RETRIED || SKIPPED
/// || RESUBMITTED`.
///
/// ## What this section said for five packages, and why it was safe until it
/// was not
///
/// It said the predicate was `SUCCESS` (later `SUCCESS` **or** `SKIPPED`), that
/// *"the two spellings cannot be told apart"*, and that *"`FAILED` and `KILLED`
/// remain unreachable here"*. The argument was: rule 1 of [`drive_concurrent`](super::parallel::drive_concurrent)
/// closes the branch on any terminal that is not `SUCCESS`, so no
/// terminated-but-not-successful dependency ever reaches this line.
///
/// **That argument had a dependency it did not name — rule 1's exclusion list
/// — and W8 Task 5 removed it.** Rule 1 is `isFailed()` now, Kestra's own
/// predicate, so a `WARNING`, `KILLED` or `RETRIED` sibling no longer closes
/// the branch. Under the old `SUCCESS | SKIPPED` spelling that combination
/// **stalls the `Dag` for ever**: the dependant is never ready and nothing
/// fails it. Reverting this line while keeping rule 1 is not a style
/// difference — it panics
/// `a_dag_node_downstream_of_a_warning_dependency_still_runs` with
/// *"down has a run"*, because the node is never created at all.
///
/// So the two lines are **coupled**, and the coupling runs both ways:
/// narrowing rule 1 without widening this one hangs the `Dag`, and widening
/// this one without narrowing rule 1 would let a dependant start under a
/// failure the branch was about to close. Neither is safe alone, and the fix
/// round that found the first had to make both.
///
/// **The generalisation is the part worth keeping**: *an argument that a
/// spelling is unobservable is a claim about ANOTHER predicate, and it expires
/// silently when that predicate changes.* This one survived W3c (which
/// weakened it to add `SKIPPED`) and died at W8 Task 5, in a commit that
/// rewrote the function thirty lines below and left this doc contradicting it.
///
/// The guards, one per state that separates the spellings:
/// `a_dag_node_downstream_of_a_skipped_dependency_still_runs` (W3c),
/// `a_dag_node_downstream_of_a_warning_dependency_still_runs` and
/// `a_dag_node_downstream_of_a_killed_dependency_still_runs` (W8 Task 5), and
/// `a_dag_creates_nothing_downstream_of_a_failed_dependency`, which is the row
/// that says rule 1 still closes the branch on a real failure. No corpus `Dag`
/// node carries a `runIf:` or an `allowFailure:`
/// (`no_corpus_parallel_or_dag_child_declares_a_run_if` re-derives the first
/// from all 351 flows), so **the oracle scores none of them** and those four
/// rows are the whole instrument.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    nodes: &[DagNode],
    errors: &[TaskIx],
    finally: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Step {
    let children: Vec<TaskIx> = nodes.iter().map(|n| n.task).collect();
    // `Dag.java:106` carries `errors` like every other flowable, and
    // `Dag.resolveNexts` passes it (and `finally`) to `resolveDagNexts`, which
    // is `resolveParallelNexts` with the `dependsOn` filter below.
    //
    // **End hooks A4 (2026-10-27): this used to defer to `parallel::drive`
    // whenever `errors:` was non-empty, and so dropped the `dependsOn` gate
    // while `tasks:` ran** -- every node started at once. No corpus `Dag` had
    // an `errors:` then; `valids/finally-dag` has one. Both lists now go
    // through `parallel::drive_lists`, which asks `current_list` and walks
    // `errors:`/`finally:` sequentially, with this predicate for `tasks:`.
    //
    // **Kestra's SECOND gate, applied to the one path that did not go through
    // it** -- W8 Task 5 fix round 4.
    //
    // `findTaskDependingFlowState` consults two predicates in order
    // (`javap -p -c io/kestra/core/models/executions/Execution.class`):
    // `hasFailed` at bci 110, then `hasFailedNoRetry` at bci 120, whose lambda
    // is `getState().isFailed() && shouldNotBeRetried(...)` over bci 0-24 and
    // whose `shouldNotBeRetried` is `!taskRun.shouldBeRetried(task.getRetry())`
    // (bci 55-74, with an absent `ResolvedTask` answering `false` at 53). When
    // every failure will be retried, the method returns `emptyList()`: start
    // nothing, **fail nothing**, wait.
    //
    // egret has that predicate -- [`current_list`]'s `Current::RetryPending` is
    // `failed.iter().all(will_retry)`, the exact complement of
    // `hasFailedNoRetry`'s `anyMatch` -- and
    // [`parallel::drive`](super::parallel::drive) has always consulted it.
    // **`dag::drive` did not.** With an empty `errors:` it called
    // `drive_concurrent` directly, so rule 1 saw a `FAILED` child whose retry
    // was still pending and `fail_the_rest` killed its in-flight siblings,
    // where the artifact waits. That is egret acting where the artifact does
    // not, and the corpus cannot see it.
    //
    // It is one call because it is one Java method: `Dag.resolveNexts` ->
    // `resolveDagNexts` -> `resolveParallelNexts` ->
    // `findTaskDependingFlowState`, the same function a `Parallel` reaches.
    // Since end hooks A4 it is asked inside `parallel::drive_lists`, for the
    // `Dag` with and without lists alike.
    super::parallel::drive_lists(
        plan,
        state,
        &children,
        errors,
        finally,
        parent,
        // Kestra's `Dag` carries `concurrent` exactly as `Parallel` does; no
        // corpus flow sets it on a `Dag`, so `TaskKind::Dag` does not carry it
        // and nothing here can cap. Recorded as a gap on `TaskKind::Dag`.
        None,
        now,
        |task, run_for| {
            let Some(node) = nodes.iter().find(|n| n.task == task) else {
                return false;
            };
            node.depends_on.iter().all(|&dep| {
                // **`isTerminated()`, not "succeeded"** -- W8 Task 5 fix round
                // 2. Measured on the pinned artifact (`javap -p -c`,
                // `io/kestra/core/runners/FlowableUtils.class`):
                // `lambda$resolveDagNexts$3` is
                // `getState().isTerminated()` and nothing else. A dependency
                // that FAILED satisfies the dependency gate on the artifact;
                // the failure is handled one layer out, by
                // `findTaskDependingFlowState` switching to the `errors:` list,
                // which here is `parallel::drive_concurrent`'s rule 1.
                //
                // It read `Some(Success) | Some(Skipped)` -- an enumeration of
                // two terminals, which was the same set only while no other
                // terminal could reach a `Dag` child. `Warning` can since W8
                // Task 5, and under the FIXED rule 1 above (which no longer
                // fires on a `WARNING` sibling) that combination **stalls the
                // DAG for ever**: the dependant is never ready and nothing
                // fails it. Fixing rule 1 alone would have traded one
                // divergence for a worse one, which is why these two lines
                // move together.
                run_for(dep).is_some_and(|ix| latest(state, ix).is_some_and(RunState::is_terminal))
            })
        },
    )
}
