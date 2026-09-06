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

use super::parallel::drive_concurrent;
use super::{latest, Step};

/// Drive one `Dag`.
///
/// # The predicate, and the one word this spells differently from Kestra
///
/// A node is startable when every task it `dependsOn` has a run whose latest
/// state is `SUCCESS`. Kestra's filter says **terminated**, not succeeded —
/// `resolveDagNexts`'s comment is literally "Check if have no dependencies OR
/// all dependencies are terminated" — so read alone, Kestra would create a
/// node whose dependency had *failed*.
///
/// It never does, and the two spellings cannot be told apart, because the
/// branch closes first: rule 1 of [`drive_concurrent`] stops a branch creating
/// anything the moment one of its children reaches a terminal state that is not
/// `SUCCESS`. Every state that separates "terminated" from "succeeded" is a
/// state that has already ended the branch, so no input reaches the predicate
/// with a terminated-but-not-successful dependency.
///
/// That is an argument, and an argument is not a guard: it is why
/// `a_dag_creates_nothing_downstream_of_a_failed_dependency` exists, and why it
/// asserts on the *runs* rather than on which spelling this line uses. If a
/// later task gives a branch a way to survive a failed child — `errors:` is
/// Task 7's and does exactly that — this line and that test have to be
/// revisited together, and the test is the one that will notice.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    nodes: &[DagNode],
    errors: &[TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Step {
    let children: Vec<TaskIx> = nodes.iter().map(|n| n.task).collect();
    // `Dag.java:106` carries `errors` like every other flowable, and
    // `Dag.resolveNexts` passes it to `resolveDagNexts`. No corpus `Dag` writes
    // one -- `every_corpus_errors_list_is_on_a_flowable_egret_plans` derives
    // that -- so this arm is here for the shape rather than for a measurement,
    // and it defers to `parallel::drive`, which is what `resolveDagNexts` does
    // too.
    if !errors.is_empty() {
        return super::parallel::drive(plan, state, &children, errors, parent, None, now);
    }
    drive_concurrent(
        plan,
        state,
        &children,
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
                run_for(dep).is_some_and(|ix| latest(state, ix) == Some(RunState::Success))
            })
        },
    )
}
