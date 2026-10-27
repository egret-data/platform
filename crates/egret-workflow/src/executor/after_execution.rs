//! The flow's `afterExecution:` -- the post-terminal phase (package #2b,
//! spec D8, D10, D11).
//!
//! Kestra's `ExecutorService.handleAfterExecution` (`ES:1005-1023`): once the
//! execution's state is terminal, the `afterExecution:` list runs, one task
//! at a time, as ordinary task runs with no parent (`ES:1018`). The
//! execution's own state never moves again: `handleEnd` returns at once on a
//! terminal execution (`ES:1026`), so its state, its history and its
//! `ended_at` are those it ended with, and a failing after-task changes
//! nothing.
//!
//! **What [`drive`] never emits** (spec §4.2): `SetExecutionState` (the
//! execution is over), `ScheduleRetry` (D11: no retry in the phase; the
//! retry machinery would reopen the execution, `retry::scan` writes
//! `SetExecutionState { Retrying }`) and `PauseRun` (D12: a `Pause` would
//! mark an ended execution `PAUSED`). The walk alone produces none of them
//! for the kinds that may run here; `Pause` and `Exit` are refused inside
//! `afterExecution:` at save and at start (D12, plan Task B2). A
//! `debug_assert!` and `the_phase_emits_no_execution_state_retry_or_pause`
//! pin it.
//!
//! **"Terminal" is not "terminated"** (D10, Kestra's
//! `ExecutionService.isTerminated(flow, execution)`, `ExS:1325-1338`): the
//! execution is terminated once it is terminal AND its after-list is done.
//! [`terminated`] is that predicate; [`drives_after`] is "terminal and not
//! terminated", the phase is running.
//!
//! **A kill.** `KILLING` never reaches this module: `advance` hands it to
//! `kill::advance_killing`, so the phase starts once the execution is
//! `KILLED` and never while it is `KILLING` (`ES:192-193`, `ES:1015`).

use super::command::Command;
use super::flowable::{run_of, sequential, Step};
use super::kill;
use super::plan::Plan;
use super::state::{ExecutionState, RunIx, RunState, Tick};

/// Kestra `ExecutorService.handleAfterExecution` (`ES:1005-1023`): once the
/// execution is terminal, run `afterExecution:` one task at a time. The
/// execution's state is never touched (`ES:1026`): no `SetExecutionState`, no
/// retry (spec D11), no pause (D12).
///
/// The caller ([`advance`](super::advance)) reaches it only with a terminal
/// last state, so this function does not ask again. The list is walked with
/// [`sequential`]'s `walk` under no parent: a failed after-task does not stop
/// the next one (`FU:79-109`, the same reading as a `finally:` list), and
/// `errors:`/`finally:` are passed empty because the flow's own lists are
/// over.
pub(crate) fn drive(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    // Q3 (user answer 2026-10-27): a kill STOPS `afterExecution:` (a
    // security divergence from Kestra, which force-executes the list after a
    // kill and answers 409 to a kill of a terminal execution). Once the kill
    // route has killed this phase's live runs ([`kill::kill_phase`]), the
    // phase is over: the walk creates no further after-task, and what is
    // left is closing the phase's flowables ([`kill::close_phase`]).
    if phase_killed(plan, state) {
        let cmds = kill::close_phase(plan, state);
        debug_assert!(
            !cmds.iter().any(writes_the_execution),
            "a killed post-terminal phase emitted a command that writes the execution: {cmds:?}"
        );
        return cmds;
    }
    let cmds = match sequential::drive_list(plan, state, &plan.after_execution, None, now) {
        Some(Step::Commands(cmds)) => cmds,
        Some(Step::Waiting | Step::Done(_)) | None => Vec::new(),
    };
    debug_assert!(
        !cmds.iter().any(writes_the_execution),
        "the post-terminal phase emitted a command that writes the execution: {cmds:?}"
    );
    cmds
}

/// The commands the phase must never emit (D8, D11, D12).
pub(crate) fn writes_the_execution(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::SetExecutionState { .. }
            | Command::ScheduleRetry { .. }
            | Command::PauseRun { .. }
    )
}

/// Kestra `ExecutionService.isTerminated(flow, execution)` (`ExS:1325-1338`):
/// the execution is terminal, and every `afterExecution:` task has a run that
/// is terminal -- [`drive`] has nothing left to create or wait for.
///
/// A `Loop` iteration's plan has no `afterExecution:` (D5, `ExS:1331`
/// excludes `LOOP`), so for it "terminated" is "terminal". So is it for any
/// flow without the list.
///
/// **A killed phase** (Q3, user answer 2026-10-27) is terminated once every
/// run of it is terminal: [`drive`] then creates nothing more, so the
/// after-tasks it never reached are not waited for.
pub fn terminated(plan: &Plan, state: &ExecutionState) -> bool {
    if !state.history.last().is_some_and(|s| s.is_terminal()) {
        return false;
    }
    let all_ran = plan.after_execution.iter().all(|&task| {
        run_of(state, None, task)
            .and_then(|ix| state.runs[ix].history.last())
            .is_some_and(|s| s.is_terminal())
    });
    all_ran
        || (phase_killed(plan, state)
            && (0..state.runs.len())
                .filter(|&ix| in_phase(plan, state, ix))
                .all(|ix| kill::is_settled(&state.runs[ix])))
}

/// **The phase has been killed** (Q3): a run of it is `KILLING` or `KILLED`.
///
/// Read off the runs, because the execution's own state must not move and
/// so cannot carry the kill. Before Q3 no run of the phase could end
/// `KILLED`: the phase starts only once a kill of the execution is over
/// (D8), `Exit` is refused in it (D12), and a worker writes `KILLED` only
/// through a kill. One reach is wider than the route: a `Subflow` of the
/// phase with `transmitFailed: true` whose child execution was killed by
/// someone else ends `KILLED` too (`subflow::guess_state`), and that stops
/// the rest of the phase -- the same reading a `KILLED` child gets in the
/// flow's `tasks:`, where it makes its parent `KILLED`
/// (`flowable::aggregate`).
pub fn phase_killed(plan: &Plan, state: &ExecutionState) -> bool {
    !plan.after_execution.is_empty()
        && state.runs.iter().enumerate().any(|(ix, run)| {
            matches!(
                run.history.last(),
                Some(RunState::Killing | RunState::Killed)
            ) && in_phase(plan, state, ix)
        })
}

/// **A kill of this terminal execution has something to stop** (Q3): the
/// execution is terminal and a run of its `afterExecution:` is live. The kill
/// route accepts a kill of a terminal execution only then
/// (`worker::kill`); otherwise it stays 409.
pub fn after_execution_live(plan: &Plan, state: &ExecutionState) -> bool {
    state.history.last().is_some_and(|s| s.is_terminal())
        && state.runs.iter().enumerate().any(|(ix, run)| {
            run.history.last().is_some_and(|s| !s.is_terminal()) && in_phase(plan, state, ix)
        })
}

/// Terminal and not [`terminated`]: the phase is running.
pub fn drives_after(plan: &Plan, state: &ExecutionState) -> bool {
    state.history.last().is_some_and(|s| s.is_terminal()) && !terminated(plan, state)
}

/// True when `run` belongs to the post-terminal phase: its root ancestor (the
/// run with no parent above it) is a task of the flow's `afterExecution:`.
///
/// [`retry::policy_for`](super::retry::policy_for) answers `None` for such a
/// run (D11): nothing in the phase is retried, so no flowable inside it may
/// wait on a retry that will never be scheduled.
pub(crate) fn in_phase(plan: &Plan, state: &ExecutionState, run: RunIx) -> bool {
    if plan.after_execution.is_empty() {
        return false;
    }
    let mut ix = run;
    while let Some(parent) = state.runs[ix].parent {
        ix = parent;
    }
    plan.after_execution.contains(&state.runs[ix].task)
}
