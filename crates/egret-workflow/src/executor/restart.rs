//! Restart an execution in place, and mark one of its task runs -- workflow
//! package 7a (spec D2, D3, D4, D6, D11).
//!
//! Both operations are pure: they read `(plan, state, now)` and return the
//! [`Command`]s that reset the runs in memory. The worker applies them under
//! the execution's lock and settles, as it does for a manual resume.

use std::collections::BTreeSet;

use super::after_execution::in_phase;
use super::command::Command;
use super::plan::{Plan, TaskIx, TaskKind};
use super::state::{apply, is_descendant_of, ExecutionState, RunIx, RunState, Tick};

/// Why [`restart`] or [`mark_as`] did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartRefusal {
    /// No run is `FAILED` or `PAUSED` (Kestra's "No task found to restart
    /// execution from!"), or the run to mark does not exist.
    NothingToRestart,
    /// The execution is in a state the operation does not act on; the state
    /// it is in.
    NotRestartable(RunState),
    /// [`mark_as`] only: the run has a `WorkingDirectory` ancestor. The
    /// block's directory is closed once its phase is Finished; marking a child
    /// would reopen the block with no directory. Restart the execution
    /// instead, which re-runs the block whole (spec D3).
    InsideWorkingDirectory,
}

/// The commands a restart or a mark applies, in order, and the runs the
/// worker has more to do for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestartCommands {
    pub commands: Vec<Command>,
    /// Runs emptied by D4/D3 (WorkingDirectory descendants): the worker renews their ids.
    pub emptied: Vec<RunIx>,
    /// Runs reset to CREATED/RUNNING: the worker clears their outputs/message/secret_outputs.
    pub reset: Vec<RunIx>,
    /// `Loop` runs set back to RUNNING whose last failing iteration the worker restarts (D8).
    pub loops: Vec<RunIx>,
    /// `Subflow` runs reset to CREATED: the worker looks up the earlier child (D9).
    pub subflows: Vec<RunIx>,
}

/// Spec D2–D4, D6. `now` is the restart's tick.
///
/// - **Reset set (D2):** every run that is `FAILED` or `PAUSED`, plus every
///   ancestor of one. A `LoopUntil` that is `PAUSED` waits between iterations
///   and is not a failure to restart from; it is not a seed.
/// - **End-hook set (D4):** every run of a task in the flow's or any task's
///   `errors:`/`finally:` (with all their descendants in the plan) and every
///   run of the `afterExecution:` phase. These are emptied. A run in both sets
///   is emptied, never reset: the seeds are taken outside the end-hook set,
///   and an ancestor of a run outside it is outside it too (the end-hook set
///   is closed under the plan's children).
/// - **State per kind (D3):** a leaf or a `Subflow` goes back to `CREATED`; a
///   `WorkingDirectory` goes back to `CREATED` with every descendant emptied;
///   every other flowable, `Pause`, `LoopUntil` and `Loop` included, is
///   reopened (`RUNNING` and a new attempt at `now`, D6). A flowable ancestor
///   that is still `RUNNING` (the parent of a `PAUSED` run in a `PAUSED`
///   execution) is left alone: Kestra's `withState(RUNNING)` on a run already
///   `RUNNING` changes nothing either (`State.java:73-77`).
///
/// Refuses [`RestartRefusal::NotRestartable`] unless the execution is `FAILED`
/// or `PAUSED`, and [`RestartRefusal::NothingToRestart`] when the reset set
/// is empty.
pub fn restart(
    plan: &Plan,
    state: &ExecutionState,
    now: Tick,
) -> Result<RestartCommands, RestartRefusal> {
    let current = execution_state(state);
    if !matches!(current, RunState::Failed | RunState::Paused) {
        return Err(RestartRefusal::NotRestartable(current));
    }
    let hooks = end_hook_runs(plan, state);
    let seeds: Vec<RunIx> = (0..state.runs.len())
        .filter(|&ix| !hooks.contains(&ix) && is_seed(plan, state, ix))
        .collect();
    if seeds.is_empty() {
        return Err(RestartRefusal::NothingToRestart);
    }
    let mut to_reset = BTreeSet::new();
    for &seed in &seeds {
        to_reset.insert(seed);
        to_reset.extend(ancestors(state, seed));
    }

    // A reset `WorkingDirectory` re-runs whole: everything beneath it is
    // emptied, whatever it was (D3, Kestra `ES:335-337`).
    let blocks: Vec<RunIx> = to_reset
        .iter()
        .copied()
        .filter(|&ix| {
            matches!(
                kind(plan, state, ix),
                Some(TaskKind::WorkingDirectory { .. })
            )
        })
        .collect();
    let in_block = |ix: RunIx| blocks.iter().any(|&b| is_descendant_of(state, ix, b));
    let mut to_empty = hooks;
    to_empty.extend((0..state.runs.len()).filter(|&ix| in_block(ix)));
    to_reset.retain(|&ix| !in_block(ix));

    let mut out = RestartCommands::default();
    empty(state, &to_empty, &mut out);
    for ix in to_reset {
        reset(plan, state, ix, now, &mut out);
    }
    out.commands.push(Command::SetExecutionState {
        state: RunState::Restarted,
    });
    Ok(out)
}

/// Spec D11 (Kestra `markAs`). `target` is one of SUCCESS/WARNING/FAILED/CANCELLED.
///
/// The run takes `target` on its history and on its last attempt, with no new
/// attempt. **There is no `OverwriteLastAttempt` command, by decision:** the
/// existing [`Command::CompleteRun`] with the same state twice already pushes
/// onto the run and onto the last attempt whether that attempt has closed or
/// not, which is Kestra's `attempts.getLast().withState(targetState)` (an
/// append). A second variant would duplicate it. Its flowable ancestors are
/// reopened as [`restart`] reopens them -- except for a `CANCELLED` target,
/// which ends the execution at once and reopens nothing (final review I3) --
/// and the end-hook set is emptied, the run itself included when it lies in
/// an `errors:`/`finally:`/`afterExecution:` branch (Kestra's
/// `changeTaskRunState`, final review M5): such a run takes no state, and the
/// branch runs again. The execution then goes `CANCELLED` for a `CANCELLED`
/// target, stays as it is while any run is still `PAUSED` (Kestra: "there
/// are still some tasks paused"), and goes `RESTARTED` otherwise.
///
/// Refuses [`RestartRefusal::NotRestartable`] unless the execution is
/// terminal and not `KILLED` (`State.java:189-191`),
/// [`RestartRefusal::NothingToRestart`] for a run that does not exist, and
/// [`RestartRefusal::InsideWorkingDirectory`] for a run beneath a
/// `WorkingDirectory`. A
/// target equal to the run's state is the caller's to refuse (D11's no-op).
pub fn mark_as(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    target: RunState,
    now: Tick,
) -> Result<RestartCommands, RestartRefusal> {
    let current = execution_state(state);
    if !current.is_terminal() || current == RunState::Killed {
        return Err(RestartRefusal::NotRestartable(current));
    }
    if run >= state.runs.len() {
        return Err(RestartRefusal::NothingToRestart);
    }
    let above = ancestors(state, run);
    if above.iter().any(|&a| {
        matches!(
            kind(plan, state, a),
            Some(TaskKind::WorkingDirectory { .. })
        )
    }) {
        return Err(RestartRefusal::InsideWorkingDirectory);
    }
    // Kestra's `changeTaskRunState` removes every end-hook run AFTER
    // `markAs`, the marked run included (`ES:532-552`, final review M5): a
    // marked run inside an `errors:`/`finally:`/`afterExecution:` branch is
    // emptied with the rest, and so are its ancestors inside that branch
    // (the set is closed under the plan's children), so the branch runs
    // again. It then takes no state of its own.
    let to_empty = end_hook_runs(plan, state);

    let mut out = RestartCommands::default();
    empty(state, &to_empty, &mut out);
    if !to_empty.contains(&run) {
        out.commands.push(Command::CompleteRun {
            run,
            run_state: target,
            attempt: target,
        });
    }
    // **A `CANCELLED` target reopens nothing** (final review I3). The
    // execution ends `CANCELLED` in this same list, and `advance()` does
    // nothing for a terminal execution, so a reopened ancestor would stay
    // `RUNNING` for good: the execution never idle, never pruned, every
    // later change refused `after_execution_running`. Kestra reopens them
    // and cancels at once, "so the executor will not process the next task";
    // not reopening them is the same outcome without the stranded runs.
    if target != RunState::Cancelled {
        let mut above: Vec<RunIx> = above
            .into_iter()
            .filter(|a| !to_empty.contains(a))
            .collect();
        above.sort_unstable();
        for a in above {
            reopen(state, a, now, &mut out);
        }
    }

    let mut projected = state.clone();
    for c in &out.commands {
        apply(&mut projected, c);
    }
    let still_paused = projected
        .runs
        .iter()
        .any(|r| r.history.last() == Some(&RunState::Paused));
    if target == RunState::Cancelled {
        out.commands.push(Command::SetExecutionState {
            state: RunState::Cancelled,
        });
    } else if !still_paused {
        out.commands.push(Command::SetExecutionState {
            state: RunState::Restarted,
        });
    }
    Ok(out)
}

fn execution_state(state: &ExecutionState) -> RunState {
    state.history.last().copied().unwrap_or(RunState::Created)
}

fn kind<'a>(plan: &'a Plan, state: &ExecutionState, run: RunIx) -> Option<&'a TaskKind> {
    plan.tasks.get(state.runs[run].task).map(|t| &t.kind)
}

/// D2's predicate on one run's own state.
fn is_seed(plan: &Plan, state: &ExecutionState, run: RunIx) -> bool {
    match state.runs[run].history.last() {
        Some(RunState::Failed) => true,
        // A `LoopUntil` is `PAUSED` only between two iterations (W8 Task 4):
        // it is waiting on its interval, not on a person or a failure.
        Some(RunState::Paused) => {
            !matches!(kind(plan, state, run), Some(TaskKind::LoopUntil { .. }))
        }
        _ => false,
    }
}

/// Every ancestor of `run`, nearest first (Kestra's `taskRunWithAncestors`).
fn ancestors(state: &ExecutionState, run: RunIx) -> Vec<RunIx> {
    let mut out = Vec::new();
    let mut cur = state.runs[run].parent;
    while let Some(p) = cur {
        // Parents strictly precede their children (`is_descendant_of`'s
        // argument), which bounds this walk for a stored state too.
        if p >= run || out.contains(&p) {
            break;
        }
        out.push(p);
        cur = state.runs[p].parent;
    }
    out
}

/// D4's end-hook set, by the plan's own structure: the tasks of the flow's
/// `errors:`/`finally:`, of every task's `errors:`/`finally:`, and everything
/// beneath them ([`Plan::children_of`], Kestra's `allErrorsWithChildren` /
/// `allFinallyWithChildren`); plus every run of the `afterExecution:` phase
/// ([`in_phase`]). Never by task id.
fn end_hook_runs(plan: &Plan, state: &ExecutionState) -> BTreeSet<RunIx> {
    let mut frontier: Vec<TaskIx> = plan.errors.iter().chain(&plan.finally).copied().collect();
    for t in &plan.tasks {
        frontier.extend(t.errors.iter().chain(&t.finally).copied());
    }
    let mut tasks: BTreeSet<TaskIx> = BTreeSet::new();
    while let Some(t) = frontier.pop() {
        if t < plan.tasks.len() && tasks.insert(t) {
            frontier.extend(plan.children_of(t));
        }
    }
    (0..state.runs.len())
        .filter(|&ix| tasks.contains(&state.runs[ix].task) || in_phase(plan, state, ix))
        .collect()
}

/// Empty each run that is not empty already, in run order.
fn empty(state: &ExecutionState, runs: &BTreeSet<RunIx>, out: &mut RestartCommands) {
    for &run in runs {
        if !state.runs[run].history.is_empty() {
            out.commands.push(Command::EmptyRun { run });
            out.emptied.push(run);
        }
    }
}

/// D3's state for one run of the reset set.
fn reset(plan: &Plan, state: &ExecutionState, run: RunIx, now: Tick, out: &mut RestartCommands) {
    match kind(plan, state, run) {
        Some(TaskKind::Executable { .. }) => {
            to_created(run, out);
            out.subflows.push(run);
        }
        // Its descendants were emptied by the caller.
        Some(TaskKind::WorkingDirectory { .. }) => to_created(run, out),
        // Never `CREATED`: that would spawn iteration 0 again (D3, D8).
        Some(TaskKind::Loop { .. }) => {
            if reopen(state, run, now, out) {
                out.loops.push(run);
            }
        }
        Some(
            TaskKind::Sequential { .. }
            | TaskKind::AllowFailure { .. }
            | TaskKind::Parallel { .. }
            | TaskKind::If { .. }
            | TaskKind::Switch { .. }
            | TaskKind::Dag { .. }
            | TaskKind::Pause { .. }
            | TaskKind::LoopUntil { .. },
        ) => {
            reopen(state, run, now, out);
        }
        // A runnable leaf, an `Exit`, an update task, or a task the plan no
        // longer has: the walk opens it again with a new attempt, as
        // `retry::reopen_due` has it re-opened.
        Some(
            TaskKind::Runnable { .. } | TaskKind::Exit { .. } | TaskKind::ExecutionUpdate { .. },
        )
        | None => to_created(run, out),
    }
}

fn to_created(run: RunIx, out: &mut RestartCommands) {
    out.commands.push(Command::SetRunState {
        run,
        state: RunState::Created,
    });
    out.reset.push(run);
}

/// Reopen a flowable (D3, D6), unless it is already `RUNNING`. Whether it was.
fn reopen(state: &ExecutionState, run: RunIx, now: Tick, out: &mut RestartCommands) -> bool {
    if state.runs[run].history.last() == Some(&RunState::Running) {
        return false;
    }
    out.commands.push(Command::ReopenFlowable { run, at: now });
    out.reset.push(run);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{
        advance, apply, build_plan, loop_condition_is_asked, terminated, TaskKind, WorkingDirPhase,
    };

    const T0: Tick = Tick(0);
    /// The restart's tick: later than anything the drive reached.
    const NOW: Tick = Tick(5_000);

    const LOG: &str = "io.kestra.plugin.core.log.Log";

    fn leaf(id: &str, indent: usize) -> String {
        let pad = " ".repeat(indent);
        format!("{pad}- id: {id}\n{pad}  type: {LOG}\n{pad}  message: {id}\n")
    }

    fn flow(body: &str) -> String {
        format!("id: f\nnamespace: io.kestra.tests\n{body}")
    }

    /// Play the executor and the worker until nothing moves: every round's
    /// commands are applied; a leaf (`Runnable`, `Subflow`, `Loop`) that is
    /// `RUNNING` ends in `outcomes`' state for its task id (default
    /// `SUCCESS`); a `WorkingDirectory` claim completes; a `Loop` gets its
    /// values rendered; a `LoopUntil` condition is answered `false`.
    fn drive(plan: &Plan, state: &mut ExecutionState, outcomes: &[(&str, RunState)]) {
        let mut outcome = |id: &str| {
            outcomes
                .iter()
                .find(|(t, _)| *t == id)
                .map_or(RunState::Success, |(_, s)| *s)
        };
        drive_from(plan, state, T0, false, &mut outcome);
    }

    /// [`drive`] from `start`, with each leaf's outcome asked of `outcome`
    /// (by task id), and -- when `pass_time` -- the clock moved to the
    /// earliest `not_before` once nothing else moves, as the oracle's driver
    /// does. Returns the tick it came to rest at.
    fn drive_from(
        plan: &Plan,
        state: &mut ExecutionState,
        start: Tick,
        pass_time: bool,
        outcome: &mut dyn FnMut(&str) -> RunState,
    ) -> Tick {
        let mut now = start;
        for _ in 0..500 {
            let cmds = advance(plan, state, now);
            if !cmds.is_empty() {
                for c in &cmds {
                    apply(state, c);
                }
                continue;
            }
            let n = state.runs.len();
            let claim = (0..n).find(|&ix| {
                matches!(
                    state.runs[ix].working_dir,
                    Some(WorkingDirPhase::Preparing | WorkingDirPhase::Finishing)
                ) && !is_terminal(state, ix)
            });
            if let Some(run) = claim {
                apply(state, &Command::CompleteWorkingDirectoryClaim { run });
                continue;
            }
            let unrendered_loop = (0..n).find(|&ix| {
                matches!(plan.tasks[state.runs[ix].task].kind, TaskKind::Loop { .. })
                    && last(state, ix) == Some(RunState::Created)
                    && state.runs[ix].rendered.is_none()
            });
            if let Some(ix) = unrendered_loop {
                state.runs[ix].rendered = Some("[\"a\"]".into());
                continue;
            }
            let condition = (0..n).find(|&ix| {
                last(state, ix) == Some(RunState::Running)
                    && state.runs[ix].rendered.is_none()
                    && loop_condition_is_asked(plan, state, ix, state.runs[ix].task, now)
            });
            if let Some(ix) = condition {
                state.runs[ix].rendered = Some("false".into());
                continue;
            }
            let running_leaf = (0..n).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. } | TaskKind::Executable { .. } | TaskKind::Loop { .. }
                ) && last(state, ix) == Some(RunState::Running)
            });
            if let Some(run) = running_leaf {
                let ended = outcome(plan.tasks[state.runs[run].task].id.as_str());
                apply(state, &Command::SetRunState { run, state: ended });
                continue;
            }
            let due = state
                .runs
                .iter()
                .filter_map(|r| r.not_before)
                .filter(|&t| t > now)
                .min();
            match due {
                Some(t) if pass_time => now = t,
                _ => return now,
            }
        }
        panic!("the drive did not come to rest");
    }

    fn driven(src: &str, outcomes: &[(&str, RunState)]) -> (Plan, ExecutionState) {
        let plan = build_plan(src).expect("the fixture flow plans");
        let mut state = ExecutionState::new(&plan);
        drive(&plan, &mut state, outcomes);
        (plan, state)
    }

    fn last(state: &ExecutionState, ix: RunIx) -> Option<RunState> {
        state.runs[ix].history.last().copied()
    }

    fn is_terminal(state: &ExecutionState, ix: RunIx) -> bool {
        last(state, ix).is_some_and(RunState::is_terminal)
    }

    /// The run of the task called `id` (the first, as `run_of` takes it).
    fn run(plan: &Plan, state: &ExecutionState, id: &str) -> RunIx {
        (0..state.runs.len())
            .find(|&ix| plan.tasks[state.runs[ix].task].id == id)
            .unwrap_or_else(|| panic!("no run of {id}"))
    }

    fn has_run(plan: &Plan, state: &ExecutionState, id: &str) -> bool {
        state.runs.iter().any(|r| plan.tasks[r.task].id == id)
    }

    fn applied(state: &ExecutionState, out: &RestartCommands) -> ExecutionState {
        let mut s = state.clone();
        for c in &out.commands {
            apply(&mut s, c);
        }
        s
    }

    fn one_round(plan: &Plan, state: &ExecutionState, now: Tick) -> (Vec<Command>, ExecutionState) {
        let cmds = advance(plan, state, now);
        let mut s = state.clone();
        for c in &cmds {
            apply(&mut s, c);
        }
        (cmds, s)
    }

    fn assert_empty(state: &ExecutionState, ix: RunIx) {
        let r = &state.runs[ix];
        assert!(r.history.is_empty(), "run {ix} history {:?}", r.history);
        assert!(r.attempts.is_empty(), "run {ix} attempts {:?}", r.attempts);
        assert_eq!(r.rendered, None);
        assert_eq!(r.rendered_run_if, None);
        assert_eq!(r.not_before, None);
        assert_eq!(r.resume_with, None);
        assert_eq!(r.working_dir, None);
    }

    // 1.
    #[test]
    fn only_the_failed_task_of_a_sequence_is_reset_to_created() {
        let src = flow(&format!(
            "tasks:\n{}{}{}",
            leaf("a", 2),
            leaf("b", 2),
            leaf("c", 2)
        ));
        let (plan, state) = driven(&src, &[("b", RunState::Failed)]);
        assert_eq!(state.history.last(), Some(&RunState::Failed));
        let (a, b) = (run(&plan, &state, "a"), run(&plan, &state, "b"));
        assert!(!has_run(&plan, &state, "c"));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);

        assert_eq!(s.runs[a], state.runs[a], "a is untouched");
        assert_eq!(last(&s, b), Some(RunState::Created));
        assert_eq!(
            s.runs[b].attempts, state.runs[b].attempts,
            "D5: the attempts are kept"
        );
        assert_eq!(out.reset, vec![b]);
        assert!(out.emptied.is_empty() && out.loops.is_empty() && out.subflows.is_empty());
        assert!(!has_run(&plan, &s, "c"));
        assert_eq!(
            &s.history[s.history.len() - 2..],
            &[RunState::Failed, RunState::Restarted]
        );
        assert_eq!(
            out.commands.last(),
            Some(&Command::SetExecutionState {
                state: RunState::Restarted
            })
        );

        let (cmds, next) = one_round(&plan, &s, NOW);
        assert_eq!(next.history.last(), Some(&RunState::Running));
        assert!(
            cmds.contains(&Command::StartRun { run: b, at: NOW }),
            "the CREATED leaf is opened, not left waiting: {cmds:?}"
        );
        assert_eq!(next.runs[b].attempts.len(), 2, "a new attempt is appended");
    }

    // 2.
    #[test]
    fn flow_errors_and_finally_runs_are_emptied_and_the_walk_is_back_in_tasks() {
        let src = flow(&format!(
            "tasks:\n{}errors:\n{}finally:\n{}",
            leaf("a", 2),
            leaf("e", 2),
            leaf("fin", 2)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let (a, e, fin) = (
            run(&plan, &state, "a"),
            run(&plan, &state, "e"),
            run(&plan, &state, "fin"),
        );
        assert_eq!(last(&state, e), Some(RunState::Success));
        assert_eq!(last(&state, fin), Some(RunState::Success));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_empty(&s, e);
        assert_empty(&s, fin);
        let mut emptied = out.emptied.clone();
        emptied.sort();
        assert_eq!(emptied, vec![e, fin]);
        assert_eq!(out.reset, vec![a]);

        // Roadmap note 7: the walk resumes the Tasks phase, not Errors/Finally.
        let (cmds, next) = one_round(&plan, &s, NOW);
        assert!(
            cmds.contains(&Command::StartRun { run: a, at: NOW }),
            "the walk must open `a`, not resume an end hook: {cmds:?}"
        );
        assert!(next.runs[e].history.is_empty() && next.runs[fin].history.is_empty());
    }

    // 3.
    #[test]
    fn a_local_errors_branch_is_emptied_and_its_sequential_reopened_at_now() {
        let src = flow(&format!(
            "tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}    errors:\n{}",
            leaf("x", 6),
            leaf("e", 6)
        ));
        let (plan, state) = driven(&src, &[("x", RunState::Failed)]);
        let (seq, x, e) = (
            run(&plan, &state, "seq"),
            run(&plan, &state, "x"),
            run(&plan, &state, "e"),
        );
        assert_eq!(last(&state, seq), Some(RunState::Failed));
        let attempts_before = state.runs[seq].attempts.len();

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_empty(&s, e);
        assert_eq!(out.emptied, vec![e]);
        assert_eq!(last(&s, seq), Some(RunState::Running));
        assert_eq!(s.runs[seq].attempts.len(), attempts_before + 1);
        assert_eq!(s.runs[seq].attempt_started_at(), Some(NOW));
        assert_eq!(last(&s, x), Some(RunState::Created));
        let mut reset = out.reset.clone();
        reset.sort();
        assert_eq!(reset, vec![seq, x]);

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(
            cmds.contains(&Command::StartRun { run: x, at: NOW }),
            "{cmds:?}"
        );
    }

    // 4.
    #[test]
    fn after_execution_runs_are_emptied() {
        let src = flow(&format!(
            "tasks:\n{}afterExecution:\n{}",
            leaf("a", 2),
            leaf("ae", 2)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let ae = run(&plan, &state, "ae");
        assert_eq!(last(&state, ae), Some(RunState::Success));
        assert!(terminated(&plan, &state));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_empty(&s, ae);
        assert_eq!(out.emptied, vec![ae]);

        let (_, next) = one_round(&plan, &s, NOW);
        assert_eq!(next.history.last(), Some(&RunState::Running));
        assert!(next.runs[ae].history.is_empty(), "the after phase waits");
    }

    // 5.
    #[test]
    fn a_parallel_whose_sibling_was_failed_by_rule_1_resets_both() {
        let src = flow(&format!(
            "tasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}",
            leaf("a", 6),
            leaf("b", 6)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let (par, a, b) = (
            run(&plan, &state, "par"),
            run(&plan, &state, "a"),
            run(&plan, &state, "b"),
        );
        assert_eq!(last(&state, a), Some(RunState::Failed));
        assert_eq!(
            last(&state, b),
            Some(RunState::Failed),
            "rule 1 fails the running sibling"
        );

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, a), Some(RunState::Created));
        assert_eq!(last(&s, b), Some(RunState::Created));
        assert_eq!(last(&s, par), Some(RunState::Running));
        assert_eq!(s.runs[par].attempt_started_at(), Some(NOW));

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(
            cmds.contains(&Command::StartRun { run: a, at: NOW })
                && cmds.contains(&Command::StartRun { run: b, at: NOW }),
            "both siblings are opened again: {cmds:?}"
        );
    }

    // 6.
    #[test]
    fn a_working_directory_is_created_again_with_every_descendant_emptied() {
        let src = flow(&format!(
            "tasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n{}{}",
            leaf("a", 6),
            leaf("b", 6)
        ));
        let (plan, state) = driven(&src, &[("b", RunState::Failed)]);
        let (block, a, b) = (
            run(&plan, &state, "block"),
            run(&plan, &state, "a"),
            run(&plan, &state, "b"),
        );
        assert_eq!(last(&state, block), Some(RunState::Failed));
        assert_eq!(
            state.runs[block].working_dir,
            Some(WorkingDirPhase::Finished)
        );

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, block), Some(RunState::Created));
        assert_eq!(s.runs[block].working_dir, None);
        assert_eq!(
            s.runs[block].attempts, state.runs[block].attempts,
            "D5: the block's own attempts are kept"
        );
        assert_empty(&s, a);
        assert_empty(&s, b);
        let mut emptied = out.emptied.clone();
        emptied.sort();
        assert_eq!(emptied, vec![a, b]);
        assert_eq!(out.reset, vec![block]);

        let (cmds, next) = one_round(&plan, &s, NOW);
        assert!(
            cmds.contains(&Command::StartRun {
                run: block,
                at: NOW
            }),
            "{cmds:?}"
        );
        let (cmds, _) = one_round(&plan, &next, NOW);
        assert_eq!(
            cmds,
            vec![Command::BeginWorkingDirectory { run: block }],
            "the block is prepared again in a fresh directory"
        );
    }

    // 7.
    #[test]
    fn a_loop_until_is_reopened_and_its_clock_restarts() {
        let src = flow(&format!(
            "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1S\n      maxIterations: 5\n    tasks:\n{}",
            leaf("body", 6)
        ));
        let (plan, state) = driven(&src, &[("body", RunState::Failed)]);
        let (lu, body) = (run(&plan, &state, "lu"), run(&plan, &state, "body"));
        assert_eq!(last(&state, lu), Some(RunState::Failed));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, lu), Some(RunState::Running));
        assert_eq!(s.runs[lu].attempt_started_at(), Some(NOW), "D6");
        assert_eq!(last(&s, body), Some(RunState::Created));

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(
            cmds.contains(&Command::StartRun { run: body, at: NOW }),
            "{cmds:?}"
        );
    }

    // 8.
    #[test]
    fn a_paused_pause_is_reopened_and_ends_success_as_if_resumed() {
        let src = flow(&format!(
            "tasks:\n  - id: wait\n    type: io.kestra.plugin.core.flow.Pause\n{}",
            leaf("after", 2)
        ));
        let (plan, state) = driven(&src, &[]);
        let wait = run(&plan, &state, "wait");
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert_eq!(last(&state, wait), Some(RunState::Paused));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, wait), Some(RunState::Running));
        assert!(s.runs[wait].history.contains(&RunState::Paused));
        assert_eq!(s.runs[wait].attempt_started_at(), Some(NOW));
        assert_eq!(s.runs[wait].resume_with, None);
        assert_eq!(s.runs[wait].not_before, None);
        assert_eq!(s.history.last(), Some(&RunState::Restarted));

        let (_, next) = one_round(&plan, &s, NOW);
        assert_eq!(next.history.last(), Some(&RunState::Running));
        assert_eq!(
            last(&next, wait),
            Some(RunState::Success),
            "Kestra `Pause.needPause`: a Pause whose history holds PAUSED does not pause again"
        );
    }

    // 9.
    #[test]
    fn a_failed_loop_is_reopened_and_listed_never_created() {
        let src = flow(
            "tasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\"]'\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        );
        let (plan, state) = driven(&src, &[("each", RunState::Failed)]);
        let each = run(&plan, &state, "each");

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, each), Some(RunState::Running));
        assert!(
            !s.runs[each].history[state.runs[each].history.len()..].contains(&RunState::Created)
        );
        assert_eq!(out.loops, vec![each]);
        assert_eq!(out.reset, vec![each]);

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::SpawnExecution { .. })),
            "a reopened Loop never re-spawns iteration 0: {cmds:?}"
        );
    }

    // 10.
    #[test]
    fn a_failed_subflow_is_created_again_and_listed() {
        let src = flow(
            "tasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: io.kestra.tests\n    flowId: child\n",
        );
        let (plan, state) = driven(&src, &[("sub", RunState::Failed)]);
        let sub = run(&plan, &state, "sub");

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, sub), Some(RunState::Created));
        assert_eq!(out.subflows, vec![sub]);
        assert_eq!(out.reset, vec![sub]);

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(cmds.contains(&Command::SpawnSubflow {
            task: s.runs[sub].task,
            run: sub
        }));
    }

    // 11.
    #[test]
    fn restart_refuses_a_successful_execution_and_one_with_nothing_failed() {
        let src = flow(&format!("tasks:\n{}", leaf("a", 2)));
        let (plan, state) = driven(&src, &[]);
        assert_eq!(
            restart(&plan, &state, NOW),
            Err(RestartRefusal::NotRestartable(RunState::Success))
        );

        let mut failed = state.clone();
        apply(
            &mut failed,
            &Command::SetExecutionState {
                state: RunState::Failed,
            },
        );
        assert_eq!(
            restart(&plan, &failed, NOW),
            Err(RestartRefusal::NothingToRestart)
        );
    }

    // Overlap (controller ruling): a FAILED run inside an `errors:` branch is
    // emptied, never reset, and so is its end-hook ancestor.
    #[test]
    fn a_failed_errors_task_is_emptied_not_reset() {
        let src = flow(&format!(
            "tasks:\n{}errors:\n  - id: eseq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}",
            leaf("a", 2),
            leaf("e2", 6)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed), ("e2", RunState::Failed)]);
        let (a, eseq, e2) = (
            run(&plan, &state, "a"),
            run(&plan, &state, "eseq"),
            run(&plan, &state, "e2"),
        );
        assert_eq!(last(&state, e2), Some(RunState::Failed));
        assert_eq!(last(&state, eseq), Some(RunState::Failed));

        let out = restart(&plan, &state, NOW).expect("restartable");
        let s = applied(&state, &out);
        assert_empty(&s, eseq);
        assert_empty(&s, e2);
        let mut emptied = out.emptied.clone();
        emptied.sort();
        assert_eq!(emptied, vec![eseq, e2]);
        assert_eq!(out.reset, vec![a]);
        assert_eq!(last(&s, a), Some(RunState::Created));
    }

    // Controller ruling: a PAUSED execution whose only waiting run is a
    // `LoopUntil` between iterations has nothing to restart.
    #[test]
    fn a_loop_until_waiting_between_iterations_is_nothing_to_restart() {
        let src = flow(&format!(
            "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1H\n      maxIterations: 5\n    tasks:\n{}",
            leaf("body", 6)
        ));
        let (plan, state) = driven(&src, &[]);
        let lu = run(&plan, &state, "lu");
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert_eq!(last(&state, lu), Some(RunState::Paused));
        assert_eq!(
            restart(&plan, &state, NOW),
            Err(RestartRefusal::NothingToRestart)
        );
    }

    // 12.
    #[test]
    fn mark_as_success_on_a_failed_first_task_lets_the_next_one_run() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let a = run(&plan, &state, "a");
        assert!(!has_run(&plan, &state, "b"));
        let attempts = state.runs[a].attempts.len();

        let out = mark_as(&plan, &state, a, RunState::Success, NOW).expect("markable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, a), Some(RunState::Success));
        assert_eq!(s.runs[a].attempts.len(), attempts, "no new attempt");
        assert_eq!(
            s.runs[a].attempts.last().and_then(|t| t.history.last()),
            Some(&RunState::Success)
        );
        assert_eq!(s.history.last(), Some(&RunState::Restarted));

        let (cmds, next) = one_round(&plan, &s, NOW);
        assert_eq!(next.history.last(), Some(&RunState::Running));
        assert!(
            cmds.iter().any(
                |c| matches!(c, Command::CreateRun { task, .. } if plan.tasks[*task].id == "b")
            ),
            "{cmds:?}"
        );
    }

    // 13.
    #[test]
    fn mark_as_empties_the_error_branch() {
        let src = flow(&format!(
            "tasks:\n{}{}errors:\n{}",
            leaf("a", 2),
            leaf("b", 2),
            leaf("e", 2)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let (a, e) = (run(&plan, &state, "a"), run(&plan, &state, "e"));
        assert_eq!(last(&state, e), Some(RunState::Success));

        let out = mark_as(&plan, &state, a, RunState::Success, NOW).expect("markable");
        let s = applied(&state, &out);
        assert_empty(&s, e);
        assert_eq!(out.emptied, vec![e]);

        let (cmds, _) = one_round(&plan, &s, NOW);
        assert!(
            cmds.iter().any(
                |c| matches!(c, Command::CreateRun { task, .. } if plan.tasks[*task].id == "b")
            ),
            "{cmds:?}"
        );
    }

    // 14.
    #[test]
    fn mark_as_cancelled_cancels_the_execution() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let a = run(&plan, &state, "a");

        let out = mark_as(&plan, &state, a, RunState::Cancelled, NOW).expect("markable");
        let s = applied(&state, &out);
        assert_eq!(last(&s, a), Some(RunState::Cancelled));
        assert_eq!(s.history.last(), Some(&RunState::Cancelled));
    }

    /// `LoopUntil` over `body`, `maxIterations: 3`, condition never true.
    fn loop_until_three() -> String {
        flow(&format!(
            "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1S\n      maxIterations: 3\n    tasks:\n{}",
            leaf("body", 6)
        ))
    }

    /// Controller fix 1: a restarted `LoopUntil` iteration is the SAME
    /// iteration (Kestra keeps the run's `iteration`, `RestartCaseTest`
    /// `restartOrReplayLoopUntil`), so `iterationCount` does not move and
    /// `maxIterations` is reached exactly where a run that never failed
    /// reaches it.
    #[test]
    fn a_restarted_loop_until_iteration_is_not_a_new_iteration() {
        use crate::executor::loop_iteration;
        let src = loop_until_three();
        let plan = build_plan(&src).expect("plans");

        // The control: the same loop with a body that never fails.
        let mut control = ExecutionState::new(&plan);
        drive_from(&plan, &mut control, T0, true, &mut |_| RunState::Success);
        let lu = run(&plan, &control, "lu");
        assert!(is_terminal(&control, lu));

        // The body fails on its second end, i.e. in iteration 2.
        let mut state = ExecutionState::new(&plan);
        let mut ends = 0;
        let rest = drive_from(&plan, &mut state, T0, true, &mut |_| {
            ends += 1;
            if ends == 2 {
                RunState::Failed
            } else {
                RunState::Success
            }
        });
        assert_eq!(last(&state, lu), Some(RunState::Failed));
        let before = loop_iteration(&state, lu);
        assert_eq!(before, 2);

        let at = Tick(rest.0 + 1_000);
        let out = restart(&plan, &state, at).expect("restartable");
        let mut s = applied(&state, &out);
        assert_eq!(
            loop_iteration(&s, lu),
            before,
            "the restarted iteration is the same iteration"
        );

        drive_from(&plan, &mut s, at, true, &mut |_| RunState::Success);
        assert_eq!(last(&s, lu), last(&control, lu));
        assert_eq!(
            loop_iteration(&s, lu),
            loop_iteration(&control, lu),
            "maxIterations is reached exactly when it would have been"
        );
    }

    /// Controller fix 2: a run beneath a `WorkingDirectory` is not marked.
    #[test]
    fn mark_as_refuses_a_run_inside_a_working_directory() {
        let src = flow(&format!(
            "tasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n{}{}",
            leaf("a", 6),
            leaf("b", 6)
        ));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let a = run(&plan, &state, "a");
        assert_eq!(last(&state, a), Some(RunState::Failed));
        assert_eq!(
            mark_as(&plan, &state, a, RunState::Success, NOW),
            Err(RestartRefusal::InsideWorkingDirectory)
        );
        // The block itself is not inside one, and is still markable.
        let block = run(&plan, &state, "block");
        assert!(mark_as(&plan, &state, block, RunState::Success, NOW).is_ok());
    }

    // 15.
    #[test]
    fn mark_as_refuses_a_killed_or_live_execution() {
        let src = flow(&format!("tasks:\n{}", leaf("a", 2)));
        let (plan, state) = driven(&src, &[("a", RunState::Failed)]);
        let a = run(&plan, &state, "a");

        let mut killed = state.clone();
        apply(
            &mut killed,
            &Command::SetExecutionState {
                state: RunState::Killed,
            },
        );
        assert_eq!(
            mark_as(&plan, &killed, a, RunState::Success, NOW),
            Err(RestartRefusal::NotRestartable(RunState::Killed))
        );

        let mut live = state.clone();
        apply(
            &mut live,
            &Command::SetExecutionState {
                state: RunState::Running,
            },
        );
        assert_eq!(
            mark_as(&plan, &live, a, RunState::Success, NOW),
            Err(RestartRefusal::NotRestartable(RunState::Running))
        );
    }

    // `EmptyRun` clears exactly what `RestartLoop` clears on a descendant.
    #[test]
    fn empty_run_clears_what_restart_loop_clears_on_a_descendant() {
        let src = flow(&format!(
            "tasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n{}",
            leaf("a", 6)
        ));
        let (_, mut state) = driven(&src, &[]);
        state.runs[1].rendered = Some("r".into());
        state.runs[1].rendered_run_if = Some("true".into());
        state.runs[1].not_before = Some(Tick(9));
        state.runs[1].working_dir = Some(WorkingDirPhase::Finished);

        let mut by_loop = state.clone();
        apply(&mut by_loop, &Command::RestartLoop { run: 0 });
        let mut by_empty = state.clone();
        apply(&mut by_empty, &Command::EmptyRun { run: 1 });
        assert_eq!(by_empty.runs[1], by_loop.runs[1]);
        assert_eq!(by_empty.runs[0], state.runs[0], "only the named run");
    }
}
