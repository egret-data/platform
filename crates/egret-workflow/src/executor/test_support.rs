//! The executor's shared test drive: 7a's helpers from `restart::tests`,
//! shared with `replay::tests` (package 7b).

use crate::executor::{
    advance, apply, build_plan, loop_condition_is_asked, Command, ExecutionState, Plan, RunIx,
    RunState, TaskKind, Tick, WorkingDirPhase,
};

pub(crate) const T0: Tick = Tick(0);
/// The restart's tick: later than anything the drive reached.
pub(crate) const NOW: Tick = Tick(5_000);

pub(crate) const LOG: &str = "io.kestra.plugin.core.log.Log";

pub(crate) fn leaf(id: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}- id: {id}\n{pad}  type: {LOG}\n{pad}  message: {id}\n")
}

pub(crate) fn flow(body: &str) -> String {
    format!("id: f\nnamespace: io.kestra.tests\n{body}")
}

/// Play the executor and the worker until nothing moves: every round's
/// commands are applied; a leaf (`Runnable`, `Subflow`, `Loop`) that is
/// `RUNNING` ends in `outcomes`' state for its task id (default
/// `SUCCESS`); a `WorkingDirectory` claim completes; a `Loop` gets its
/// values rendered; a `LoopUntil` condition is answered `false`.
pub(crate) fn drive(plan: &Plan, state: &mut ExecutionState, outcomes: &[(&str, RunState)]) {
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
pub(crate) fn drive_from(
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

pub(crate) fn driven(src: &str, outcomes: &[(&str, RunState)]) -> (Plan, ExecutionState) {
    let plan = build_plan(src).expect("the fixture flow plans");
    let mut state = ExecutionState::new(&plan);
    drive(&plan, &mut state, outcomes);
    (plan, state)
}

pub(crate) fn last(state: &ExecutionState, ix: RunIx) -> Option<RunState> {
    state.runs[ix].history.last().copied()
}

pub(crate) fn is_terminal(state: &ExecutionState, ix: RunIx) -> bool {
    last(state, ix).is_some_and(RunState::is_terminal)
}

/// The run of the task called `id` (the first, as `run_of` takes it).
pub(crate) fn run(plan: &Plan, state: &ExecutionState, id: &str) -> RunIx {
    (0..state.runs.len())
        .find(|&ix| plan.tasks[state.runs[ix].task].id == id)
        .unwrap_or_else(|| panic!("no run of {id}"))
}
