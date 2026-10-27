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
use super::plan::Plan;
use super::state::{ExecutionState, RunIx, Tick};

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
    // B3 HOOK (Q3, user answer 2026-10-27): a kill STOPS `afterExecution:`
    // (a security divergence from Kestra, which force-executes the list
    // after a kill and answers 409 to a kill of a terminal execution). Plan
    // Task B3 builds it: once the kill route has stopped this phase's live
    // runs, this walk must create no further after-task. Until B3 nothing
    // records such a kill, so there is nothing to ask here yet.
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
pub fn terminated(plan: &Plan, state: &ExecutionState) -> bool {
    state.history.last().is_some_and(|s| s.is_terminal())
        && plan.after_execution.iter().all(|&task| {
            run_of(state, None, task)
                .and_then(|ix| state.runs[ix].history.last())
                .is_some_and(|s| s.is_terminal())
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
