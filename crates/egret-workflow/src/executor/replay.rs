//! Replay from a task run, and restart on another source, as copies --
//! workflow package 7b (spec `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`
//! D1-D5; Kestra `ExecutionService.replay`, `ES:393-512`, and `.restart` with
//! a revision, `ES:281-373`).
//!
//! Pure, like `executor::restart`: these functions say which of the old
//! execution's runs the new execution keeps and which of those it resets.
//! The worker copies the kept runs (`worker::copy`) and runs the reset as
//! 7a's restart job.

use std::collections::BTreeSet;

use super::plan::{Plan, TaskIx, TaskKind};
use super::restart::{self, RestartRefusal};
use super::state::{is_descendant_of, ExecutionState, RunIx, RunState};

/// Why a copy cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    /// The chosen run is not a run of this execution (out of range, or emptied).
    NoSuchRun,
    /// `restart_copy`: the execution is not `FAILED` or `PAUSED` (7a's rule).
    NotRestartable(RunState),
    /// `restart_copy`: no run is `FAILED` or `PAUSED`.
    NothingToRestart,
    /// `replay_from`: the chosen run has a `WorkingDirectory` ancestor (D9).
    InsideWorkingDirectory,
    /// `copy_onto` (A2): a copied run's task does not fit the target source (D5, S3).
    TaskNotInSource { task_id: String, why: MatchFault },
}

/// Which of S3's three conditions a copied run's task fails in the target source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchFault {
    /// No task of that id.
    Missing,
    /// A task of that id under another parent task or in another list.
    Moved,
    /// A task of that id in the same place, of another kind.
    OtherKind,
}

/// The runs a copy keeps, and the kept runs it resets, as indexes of the OLD state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopySelection {
    /// Ascending. Never an empty run, never a run whose parent is not kept.
    pub keep: Vec<RunIx>,
    /// A subset of `keep`.
    pub reset: BTreeSet<RunIx>,
}

/// Spec D2 (Kestra `ES:393-512`, with S1's divergence): replay from `chosen`.
///
/// The reset set is `chosen` and its ancestors (Kestra `taskRunToRestart`,
/// `ES:375-390`). The dropped set is Kestra's successors of `chosen`
/// (`successors`), plus S1's `scope_errors`, plus every run beneath a
/// reset `WorkingDirectory` (`removeWorkerTask`, `ES:1179-1200`). A kept run
/// that had not ended is reset too (`ES:456-484`).
///
/// Refuses [`CopyRefusal::NoSuchRun`] for a run out of range or emptied, and
/// [`CopyRefusal::InsideWorkingDirectory`] for a run beneath a
/// `WorkingDirectory` (D9).
pub fn replay_from(
    plan: &Plan,
    state: &ExecutionState,
    chosen: RunIx,
) -> Result<CopySelection, CopyRefusal> {
    let n = state.runs.len();
    if chosen >= n || state.runs[chosen].history.is_empty() {
        return Err(CopyRefusal::NoSuchRun);
    }
    let above = restart::ancestors(state, chosen);
    if above.iter().any(|&a| is_working_directory(plan, state, a)) {
        return Err(CopyRefusal::InsideWorkingDirectory);
    }
    // Kestra `taskRunToRestart` (`ES:375-390`): the run and its ancestors.
    let mut reset: BTreeSet<RunIx> = above.into_iter().collect();
    reset.insert(chosen);
    // `ES:435-454`, plus S1's divergence.
    let mut dropped = successors(plan, state, chosen);
    dropped.extend(scope_errors(plan, state, chosen));
    dropped.retain(|r| !reset.contains(r));
    // `removeWorkerTask` (`ES:1179-1200`): a reset `WorkingDirectory` re-runs whole.
    for &w in &reset {
        if is_working_directory(plan, state, w) {
            dropped.extend((0..n).filter(|&ix| is_descendant_of(state, ix, w)));
        }
    }
    let keep = kept(state, &dropped);
    // `ES:456-484`: a kept run that had not ended is reset too.
    for &ix in &keep {
        if !state.runs[ix]
            .history
            .last()
            .is_some_and(|s| s.is_terminal())
        {
            reset.insert(ix);
        }
    }
    reset.retain(|r| keep.binary_search(r).is_ok());
    Ok(CopySelection { keep, reset })
}

/// Spec D3 (Kestra `ES:311-351`): 7a's [`restart`](restart::restart) sets,
/// as a selection. Its emptied runs are dropped; its reset and `Loop` runs
/// are the reset set.
pub fn restart_copy(plan: &Plan, state: &ExecutionState) -> Result<CopySelection, CopyRefusal> {
    // 7a's sets, its tick irrelevant: only the sets are read.
    let cmds = restart::restart(plan, state, super::state::Tick(0)).map_err(|r| match r {
        RestartRefusal::NothingToRestart => CopyRefusal::NothingToRestart,
        RestartRefusal::NotRestartable(s) => CopyRefusal::NotRestartable(s),
        RestartRefusal::InsideWorkingDirectory => unreachable!("only mark_as returns it"),
    })?;
    let dropped: BTreeSet<RunIx> = cmds.emptied.iter().copied().collect();
    let keep = kept(state, &dropped);
    let reset = cmds
        .reset
        .iter()
        .chain(&cmds.loops)
        .copied()
        .filter(|r| keep.binary_search(r).is_ok())
        .collect();
    Ok(CopySelection { keep, reset })
}

fn is_working_directory(plan: &Plan, state: &ExecutionState, run: RunIx) -> bool {
    matches!(
        restart::kind(plan, state, run),
        Some(TaskKind::WorkingDirectory { .. })
    )
}

/// Every run neither dropped nor empty, whose parent is kept too (parents
/// precede children, so one ascending pass decides it).
fn kept(state: &ExecutionState, dropped: &BTreeSet<RunIx>) -> Vec<RunIx> {
    let mut keep: Vec<RunIx> = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        let parent_kept = run.parent.is_none_or(|p| keep.binary_search(&p).is_ok());
        if !dropped.contains(&ix) && !run.history.is_empty() && parent_kept {
            keep.push(ix);
        }
    }
    keep
}

/// The list a run's task sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Tasks,
    Errors,
    Finally,
    AfterExecution,
}

/// How a list's members follow each other in Kestra's graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// A chain (`GraphUtils.fillGraph`, SEQUENTIAL): every list but these two.
    Chain,
    /// `Parallel`'s tasks: each from the task node, none after another.
    Parallel,
    /// A `Dag`'s nodes: by `dependsOn` (`GraphUtils.fillGraphDag`, `:523-633`).
    Dag,
}

struct Slot {
    parent: Option<RunIx>,
    role: Role,
    shape: Shape,
    list: Vec<TaskIx>,
}

/// Where `run`'s task sits under its parent run (or the flow).
fn slot(plan: &Plan, state: &ExecutionState, run: RunIx) -> Option<Slot> {
    let task = state.runs[run].task;
    let parent = state.runs[run].parent;
    let lists: Vec<(Role, Shape, Vec<TaskIx>)> = match parent {
        None => vec![
            (Role::Tasks, Shape::Chain, plan.roots.clone()),
            (Role::Errors, Shape::Chain, plan.errors.clone()),
            (Role::Finally, Shape::Chain, plan.finally.clone()),
            (
                Role::AfterExecution,
                Shape::Chain,
                plan.after_execution.clone(),
            ),
        ],
        Some(p) => {
            let pt = plan.tasks.get(state.runs[p].task)?;
            let mut v = vec![
                (Role::Errors, Shape::Chain, pt.errors.clone()),
                (Role::Finally, Shape::Chain, pt.finally.clone()),
            ];
            match &pt.kind {
                TaskKind::Sequential { tasks }
                | TaskKind::AllowFailure { tasks }
                | TaskKind::WorkingDirectory { tasks }
                | TaskKind::LoopUntil { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Chain, tasks.clone()))
                }
                TaskKind::Parallel { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Parallel, tasks.clone()))
                }
                TaskKind::If { then_, else_, .. } => {
                    v.push((Role::Tasks, Shape::Chain, then_.clone()));
                    v.push((Role::Tasks, Shape::Chain, else_.clone()));
                }
                TaskKind::Switch { cases, default, .. } => {
                    for (_, c) in cases {
                        v.push((Role::Tasks, Shape::Chain, c.clone()));
                    }
                    v.push((Role::Tasks, Shape::Chain, default.clone()));
                }
                TaskKind::Dag { nodes } => v.push((
                    Role::Tasks,
                    Shape::Dag,
                    nodes.iter().map(|d| d.task).collect(),
                )),
                _ => {}
            }
            v
        }
    };
    lists
        .into_iter()
        .find(|(_, _, l)| l.contains(&task))
        .map(|(role, shape, list)| Slot {
            parent,
            role,
            shape,
            list,
        })
}

/// The `finally:` of the scope a run of `parent` (or the flow) sits in.
fn scope_finally(plan: &Plan, state: &ExecutionState, parent: Option<RunIx>) -> Vec<TaskIx> {
    match parent {
        None => plan.finally.clone(),
        Some(p) => plan.tasks[state.runs[p].task].finally.clone(),
    }
}

/// The `errors:` of the scope a run of `parent` (or the flow) sits in.
fn scope_errors_of(plan: &Plan, state: &ExecutionState, parent: Option<RunIx>) -> Vec<TaskIx> {
    match parent {
        None => plan.errors.clone(),
        Some(p) => plan.tasks[state.runs[p].task].errors.clone(),
    }
}

/// `r` and every run beneath it.
fn subtree(state: &ExecutionState, out: &mut BTreeSet<RunIx>, r: RunIx) {
    out.insert(r);
    out.extend((0..state.runs.len()).filter(|&ix| is_descendant_of(state, ix, r)));
}

/// The runs whose parent is `parent` (the flow's own runs for `None`).
fn under(state: &ExecutionState, parent: Option<RunIx>) -> Vec<RunIx> {
    (0..state.runs.len())
        .filter(|&ix| state.runs[ix].parent == parent)
        .collect()
}

/// Kestra `GraphUtils.successors(graph, {chosen})` (`GraphUtils.java:144-239`)
/// over egret's plan and runs (S1).
fn successors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let mut out = BTreeSet::new();
    subtree(state, &mut out, chosen);

    // `:157-169`: a node whose incoming edge comes from a cluster ROOT is
    // also walked from that root -- the first top-level task (`root → t1`)
    // and the first task of an `errors:` list (`root → e1`, `:468-472`).
    // Only for a task NODE: the scope root links to `toEdgeTarget` (`:516`),
    // which for a cluster is the cluster's own root, so a chosen flowable's
    // only root edge is its own (`GraphCluster.java:74`), and its own root
    // reaches only its own errors, already in its subtree.
    if let Some(s) = slot(plan, state, chosen) {
        let first = s.list.first() == Some(&state.runs[chosen].task);
        let top_tasks = s.parent.is_none() && s.role == Role::Tasks;
        if first && (top_tasks || s.role == Role::Errors) && !is_cluster(plan, state, chosen) {
            for r in under(state, s.parent) {
                subtree(state, &mut out, r);
            }
        }
    }

    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        let task = state.runs[cur].task;
        let later: BTreeSet<TaskIx> = match s.shape {
            Shape::Chain => s
                .list
                .iter()
                .skip_while(|&&t| t != task)
                .skip(1)
                .copied()
                .collect(),
            Shape::Parallel => BTreeSet::new(),
            Shape::Dag => dag_dependents(plan, state, s.parent, task),
        };
        // The scope's later phases (`:488-489`, `:413-421`).
        let finally = scope_finally(plan, state, s.parent);
        let after: &[TaskIx] = if s.parent.is_none() {
            &plan.after_execution
        } else {
            &[]
        };
        let phases: Vec<TaskIx> = match s.role {
            Role::Tasks | Role::Errors => finally.iter().chain(after).copied().collect(),
            Role::Finally => after.to_vec(),
            Role::AfterExecution => Vec::new(),
        };
        for r in under(state, s.parent) {
            let t = state.runs[r].task;
            // `fillGraph` chains a task's several runs in order, then the next task's.
            // Reachable only from a hand-built state: egret's `LoopUntil` reuses its runs.
            let same_task_later = s.shape == Shape::Chain && t == task && r > cur;
            if later.contains(&t) || same_task_later || phases.contains(&t) {
                subtree(state, &mut out, r);
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}

/// Whether Kestra draws `run`'s task as a `GraphCluster` (its own root,
/// task node and end) rather than a task node. Every `FlowableTask` is one
/// (`GraphUtils.java:453-454`), except a `Pause` with no `errors:` and no
/// `finally:`, whose `tasksTree` is a plain `GraphTask` (`Pause.java:208-211`).
/// A `Subflow` is a `SubflowGraphTask` and `Exit`/an update task a
/// `GraphTask`: task nodes. A task the plan no longer has is taken as a node.
fn is_cluster(plan: &Plan, state: &ExecutionState, run: RunIx) -> bool {
    let Some(task) = plan.tasks.get(state.runs[run].task) else {
        return false;
    };
    match &task.kind {
        TaskKind::Runnable { .. }
        | TaskKind::Executable { .. }
        | TaskKind::Exit { .. }
        | TaskKind::ExecutionUpdate { .. } => false,
        TaskKind::Pause { .. } => !task.errors.is_empty() || !task.finally.is_empty(),
        TaskKind::Sequential { .. }
        | TaskKind::AllowFailure { .. }
        | TaskKind::WorkingDirectory { .. }
        | TaskKind::Parallel { .. }
        | TaskKind::If { .. }
        | TaskKind::Switch { .. }
        | TaskKind::Dag { .. }
        | TaskKind::Loop { .. }
        | TaskKind::LoopUntil { .. } => true,
    }
}

/// The `Dag` nodes that depend on `task`, transitively.
fn dag_dependents(
    plan: &Plan,
    state: &ExecutionState,
    parent: Option<RunIx>,
    task: TaskIx,
) -> BTreeSet<TaskIx> {
    let Some(p) = parent else {
        return BTreeSet::new();
    };
    let TaskKind::Dag { nodes } = &plan.tasks[state.runs[p].task].kind else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    let mut frontier = vec![task];
    while let Some(t) = frontier.pop() {
        for d in nodes.iter().filter(|d| d.depends_on.contains(&t)) {
            if out.insert(d.task) {
                frontier.push(d.task);
            }
        }
    }
    out
}

/// **S1's divergence (spec §5 item 6):** every scope that the chosen run, or
/// an ancestor the walk climbs through, sits in through its `tasks` (any
/// branch) has its `errors:` runs dropped with their subtrees. Kestra keeps
/// them, and a kept errors run holds the list in error mode
/// (`Execution.java:595-606`; egret `flowable::current_list`), so the
/// chosen task would never run.
fn scope_errors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let mut out = BTreeSet::new();
    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        if s.role == Role::Tasks {
            let errors = scope_errors_of(plan, state, s.parent);
            for r in under(state, s.parent) {
                if errors.contains(&state.runs[r].task) {
                    subtree(state, &mut out, r);
                }
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_support::{drive, drive_from, driven, flow, leaf, run, T0};
    use crate::executor::{build_plan, ExecutionState, RunState};

    const PAR: &str = "io.kestra.plugin.core.flow.Parallel";
    const SEQ: &str = "io.kestra.plugin.core.flow.Sequential";
    const WD: &str = "io.kestra.plugin.core.flow.WorkingDirectory";

    fn ids(
        plan: &Plan,
        state: &ExecutionState,
        ixs: impl IntoIterator<Item = RunIx>,
    ) -> Vec<String> {
        let mut v: Vec<String> = ixs
            .into_iter()
            .map(|ix| plan.tasks[state.runs[ix].task].id.clone())
            .collect();
        v.sort();
        v
    }

    fn block(id: &str, ty: &str, indent: usize, body: &str) -> String {
        let pad = " ".repeat(indent);
        format!("{pad}- id: {id}\n{pad}  type: {ty}\n{pad}  tasks:\n{body}")
    }

    fn sorted(v: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    fn kept(sel: &CopySelection, plan: &Plan, state: &ExecutionState) -> Vec<String> {
        ids(plan, state, sel.keep.iter().copied())
    }

    fn reset(sel: &CopySelection, plan: &Plan, state: &ExecutionState) -> Vec<String> {
        ids(plan, state, sel.reset.iter().copied())
    }

    /// `a, b` then `errors:` holding `errs`, with `b` failing.
    fn with_errors(errs: &[&str]) -> (Plan, ExecutionState) {
        let errors: String = errs.iter().map(|e| leaf(e, 2)).collect();
        let src = flow(&format!(
            "tasks:\n{}{}errors:\n{errors}",
            leaf("a", 2),
            leaf("b", 2)
        ));
        driven(&src, &[("b", RunState::Failed)])
    }

    // 1. Kestra `replaySimple` (`ExecutionServiceTest.java:197-215`).
    #[test]
    fn replay_simple_keeps_what_came_before() {
        let body: String = ["t1", "t2", "t3", "t4", "t5"]
            .iter()
            .map(|t| leaf(t, 2))
            .collect();
        let (plan, state) = driven(&flow(&format!("tasks:\n{body}")), &[]);
        assert_eq!(state.runs.len(), 5);

        let sel = replay_from(&plan, &state, run(&plan, &state, "t2")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["t1", "t2"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["t2"]));
    }

    // 2. Kestra `replayParallel` (`:239-258`), over `parallel-nested.yaml`.
    #[test]
    fn replay_parallel_nested_keeps_eight_of_eleven() {
        let inner_seq = block(
            "1-3-2_par",
            SEQ,
            10,
            &format!("{}{}", leaf("1-3-2-1", 14), leaf("1-3-2-2_end", 14)),
        );
        let inner_par = block(
            "1-3_par",
            PAR,
            6,
            &format!("{}{inner_seq}{}", leaf("1-3-1", 10), leaf("1-3-3_end", 10)),
        );
        let outer = block(
            "1_par",
            PAR,
            2,
            &format!(
                "{}{}{inner_par}{}",
                leaf("1-1", 6),
                leaf("1-2", 6),
                leaf("1-4_end", 6)
            ),
        );
        let src = flow(&format!("tasks:\n{outer}{}", leaf("2_end", 2)));
        let (plan, state) = driven(&src, &[]);
        assert_eq!(state.runs.len(), 11);

        let sel = replay_from(&plan, &state, run(&plan, &state, "1-3-2_par")).expect("replayable");

        assert_eq!(sel.keep.len(), 8, "kept {:?}", kept(&sel, &plan, &state));
        let all: BTreeSet<RunIx> = (0..state.runs.len()).collect();
        let keep: BTreeSet<RunIx> = sel.keep.iter().copied().collect();
        assert_eq!(
            ids(&plan, &state, all.difference(&keep).copied()),
            sorted(&["1-3-2-1", "1-3-2-2_end", "2_end"])
        );
        assert_eq!(
            reset(&sel, &plan, &state),
            sorted(&["1-3-2_par", "1-3_par", "1_par"])
        );
    }

    // 3. Kestra `replaySequentialWithErrorHandler` (`:563-596`).
    #[test]
    fn replay_from_a_flowable_drops_its_errors_runs() {
        let src = flow(&format!(
            "tasks:\n{}  - id: sequential\n    type: {SEQ}\n    tasks:\n{}    errors:\n{}",
            leaf("before", 2),
            leaf("failing-task", 6),
            leaf("error-handler", 6)
        ));
        let (plan, state) = driven(&src, &[("failing-task", RunState::Failed)]);
        assert_eq!(state.runs.len(), 4);

        let sel = replay_from(&plan, &state, run(&plan, &state, "sequential")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["before", "sequential"]));
    }

    // 4.
    #[test]
    fn the_first_top_level_task_drops_everything_else() {
        let src = flow(&format!(
            "tasks:\n{}{}{}",
            leaf("a", 2),
            leaf("b", 2),
            leaf("c", 2)
        ));
        let (plan, state) = driven(&src, &[]);

        let sel = replay_from(&plan, &state, run(&plan, &state, "a")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a"]));
    }

    // 5.
    #[test]
    fn finally_and_after_execution_runs_are_dropped() {
        let src = flow(&format!(
            "tasks:\n{}{}finally:\n{}afterExecution:\n{}",
            leaf("a", 2),
            leaf("b", 2),
            leaf("f", 2),
            leaf("x", 2)
        ));
        let (plan, state) = driven(&src, &[]);
        assert_eq!(state.runs.len(), 4, "f and x ran");

        let sel = replay_from(&plan, &state, run(&plan, &state, "b")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "b"]));
    }

    // 6.
    #[test]
    fn a_kept_scope_errors_run_is_dropped_s1() {
        let (plan, state) = with_errors(&["e"]);
        assert_eq!(state.runs.len(), 3);

        let sel = replay_from(&plan, &state, run(&plan, &state, "b")).expect("replayable");

        // Kestra keeps `e` (its successors reach errors only through a
        // cluster root), and the kept errors run would hold the list in error
        // mode so `b` never runs again. Spec correction S1 drops it.
        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "b"]));
    }

    // 7.
    #[test]
    fn the_second_error_task_keeps_the_first() {
        let (plan, state) = with_errors(&["e", "e2"]);

        let sel = replay_from(&plan, &state, run(&plan, &state, "e2")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "b", "e", "e2"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["e2"]));
    }

    // 8.
    #[test]
    fn the_first_error_task_reaches_the_whole_scope() {
        let (plan, state) = with_errors(&["e", "e2"]);

        let sel = replay_from(&plan, &state, run(&plan, &state, "e")).expect("replayable");

        // Kestra's root rule (`GraphUtils.java:157-169`): `root → e`.
        assert_eq!(kept(&sel, &plan, &state), sorted(&["e"]));
    }

    // Fix round 1. Kestra links an `errors:` list's first entry from the
    // scope root to `toEdgeTarget` (`GraphUtils.java:474-484`, `:516`): for a
    // flowable that is ITS OWN cluster root, so `successors` (`:157-169`)
    // adds only that root, never the scope's, and the scope's runs stay.
    #[test]
    fn a_flowable_first_in_errors_keeps_the_scope() {
        let src = flow(&format!(
            "tasks:\n{}{}errors:\n{}",
            leaf("a", 2),
            leaf("b", 2),
            block("E", SEQ, 2, &leaf("e1", 6))
        ));
        let (plan, state) = driven(&src, &[("b", RunState::Failed)]);
        assert_eq!(
            ids(&plan, &state, 0..state.runs.len()),
            sorted(&["a", "b", "E", "e1"])
        );

        let sel = replay_from(&plan, &state, run(&plan, &state, "E")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "b", "E"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["E"]));
    }

    // Fix round 1, in a nested scope.
    #[test]
    fn a_flowable_first_in_a_nested_errors_keeps_the_scope() {
        let src = flow(&format!(
            "tasks:\n{}  - id: s\n    type: {SEQ}\n    tasks:\n{}    errors:\n{}",
            leaf("a", 2),
            leaf("x", 6),
            block("E", SEQ, 6, &leaf("e1", 10))
        ));
        let (plan, state) = driven(&src, &[("x", RunState::Failed)]);
        assert_eq!(
            ids(&plan, &state, 0..state.runs.len()),
            sorted(&["a", "s", "x", "E", "e1"])
        );

        let sel = replay_from(&plan, &state, run(&plan, &state, "E")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "s", "x", "E"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["E", "s"]));
    }

    // 9.
    #[test]
    fn in_a_parallel_siblings_are_not_successors() {
        let src = flow(&format!(
            "tasks:\n{}{}",
            block("p", PAR, 2, &format!("{}{}", leaf("x", 6), leaf("y", 6))),
            leaf("z", 2)
        ));
        let (plan, state) = driven(&src, &[]);

        let sel = replay_from(&plan, &state, run(&plan, &state, "x")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["p", "x", "y"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["p", "x"]));
    }

    // 10.
    #[test]
    fn in_a_switch_later_case_tasks_are_dropped() {
        let src = flow(&format!(
            "tasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"A\"\n    cases:\n      A:\n{}{}{}",
            leaf("a1", 8),
            leaf("a2", 8),
            leaf("after", 2)
        ));
        let plan = build_plan(&src).expect("plans");
        let mut state = ExecutionState::new(&plan);
        drive(&plan, &mut state, &[]);
        let s = run(&plan, &state, "s");
        state.runs[s].rendered = Some("A".into());
        drive(&plan, &mut state, &[]);
        assert_eq!(state.runs.len(), 4, "s, a1, a2 and after ran");

        let sel = replay_from(&plan, &state, run(&plan, &state, "a1")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["s", "a1"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["s", "a1"]));
    }

    // 11. `same_task_later`.
    #[test]
    fn a_loop_until_iteration_drops_later_iterations() {
        let src = flow(&format!(
            "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1S\n      maxIterations: 2\n    tasks:\n{}{}",
            leaf("c1", 6),
            leaf("c2", 6)
        ));
        let plan = build_plan(&src).expect("plans");
        let mut state = ExecutionState::new(&plan);
        drive_from(&plan, &mut state, T0, true, &mut |_| RunState::Success);
        // egret's `LoopUntil` runs every iteration on the SAME runs
        // (`Command::RestartLoop` empties them in place), so the drive leaves
        // one `c1` and one `c2`. Kestra's graph chains a task's several runs
        // in order (`fillGraph`); write a second iteration by hand, as the
        // `state.rs` tests write states, to reach `same_task_later`.
        let first: Vec<RunIx> = (0..state.runs.len())
            .filter(|&ix| state.runs[ix].parent.is_some())
            .collect();
        assert_eq!(first.len(), 2, "one iteration's c1 and c2");
        for ix in first {
            let again = state.runs[ix].clone();
            state.runs.push(again);
        }
        let of = |id: &str| -> Vec<RunIx> {
            (0..state.runs.len())
                .filter(|&ix| plan.tasks[state.runs[ix].task].id == id)
                .collect()
        };
        let (c1, c2) = (of("c1"), of("c2"));
        assert_eq!((c1.len(), c2.len()), (2, 2), "two iterations ran");

        let sel = replay_from(&plan, &state, c1[0]).expect("replayable");

        assert!(sel.keep.contains(&c1[0]));
        assert!(!sel.keep.contains(&c1[1]), "the second iteration's c1");
        assert!(!sel.keep.contains(&c2[1]), "the second iteration's c2");
        assert!(
            !sel.keep.contains(&c2[0]),
            "c2 follows c1 in its own iteration"
        );
        assert_eq!(kept(&sel, &plan, &state), sorted(&["lu", "c1"]));
    }

    // 12.
    #[test]
    fn a_dag_drops_only_dependents() {
        let node = |id: &str, deps: &str| {
            format!(
                "      - task:\n          id: {id}\n          type: io.kestra.plugin.core.log.Log\n          message: {id}\n{deps}"
            )
        };
        let src = flow(&format!(
            "tasks:\n  - id: dag\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n{}{}{}",
            node("d1", ""),
            node("d2", "        dependsOn:\n          - d1\n"),
            node("d3", "")
        ));
        let (plan, state) = driven(&src, &[]);
        assert_eq!(state.runs.len(), 4);

        let sel = replay_from(&plan, &state, run(&plan, &state, "d1")).expect("replayable");

        let k = kept(&sel, &plan, &state);
        assert!(k.contains(&"d3".to_string()), "{k:?}");
        assert!(!k.contains(&"d2".to_string()), "{k:?}");
        assert_eq!(k, sorted(&["dag", "d1", "d3"]));
    }

    fn working_directory() -> (Plan, ExecutionState) {
        let src = flow(&format!(
            "tasks:\n{}{}",
            block("w", WD, 2, &format!("{}{}", leaf("w1", 6), leaf("w2", 6))),
            leaf("c", 2)
        ));
        driven(&src, &[])
    }

    // 13.
    #[test]
    fn a_working_directory_is_replayed_whole() {
        let (plan, state) = working_directory();
        assert_eq!(state.runs.len(), 4);

        let sel = replay_from(&plan, &state, run(&plan, &state, "w")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["w"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["w"]));
    }

    // 14.
    #[test]
    fn a_run_inside_a_working_directory_is_refused() {
        let (plan, state) = working_directory();

        assert_eq!(
            replay_from(&plan, &state, run(&plan, &state, "w1")),
            Err(CopyRefusal::InsideWorkingDirectory)
        );
    }

    // 15. Kestra `replayParallelRestartsRunningSibling` (`:260-284`).
    #[test]
    fn an_unfinished_kept_run_is_reset() {
        let src = flow(&format!(
            "tasks:\n{}",
            block("p", PAR, 2, &format!("{}{}", leaf("x", 6), leaf("y", 6)))
        ));
        let (plan, mut state) = driven(&src, &[]);
        let y = run(&plan, &state, "y");
        state.runs[y].history.pop();
        state.runs[y].history.push(RunState::Running);

        let sel = replay_from(&plan, &state, run(&plan, &state, "x")).expect("replayable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["p", "x", "y"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["p", "x", "y"]));
    }

    // 16.
    #[test]
    fn an_empty_or_unknown_run_is_refused() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, mut state) = driven(&src, &[]);

        assert_eq!(
            replay_from(&plan, &state, state.runs.len()),
            Err(CopyRefusal::NoSuchRun)
        );
        let b = run(&plan, &state, "b");
        state.runs[b].history.clear();
        assert_eq!(replay_from(&plan, &state, b), Err(CopyRefusal::NoSuchRun));
    }

    // 17.
    #[test]
    fn restart_copy_resets_the_failed_and_drops_the_hooks() {
        let src = flow(&format!(
            "tasks:\n{}{}{}errors:\n{}finally:\n{}afterExecution:\n{}",
            leaf("a", 2),
            leaf("b", 2),
            leaf("c", 2),
            leaf("e", 2),
            leaf("f", 2),
            leaf("x", 2)
        ));
        let (plan, state) = driven(&src, &[("b", RunState::Failed)]);
        assert_eq!(
            ids(&plan, &state, 0..state.runs.len()),
            sorted(&["a", "b", "e", "f", "x"])
        );

        let sel = restart_copy(&plan, &state).expect("restartable");

        assert_eq!(kept(&sel, &plan, &state), sorted(&["a", "b"]));
        assert_eq!(reset(&sel, &plan, &state), sorted(&["b"]));
    }

    // 18.
    #[test]
    fn restart_copy_refusals() {
        let (plan, state) = driven(&flow(&format!("tasks:\n{}", leaf("a", 2))), &[]);

        assert_eq!(
            restart_copy(&plan, &state),
            Err(CopyRefusal::NotRestartable(RunState::Success))
        );
    }
}
