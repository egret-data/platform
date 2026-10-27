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
//! `afterExecution:` at save and at start (D12, plan Task B2).
//! `the_phase_emits_no_execution_state_retry_or_pause` pins it.
//!
//! **A STORED source is not refused at start** (final-review fix,
//! 2026-10-28): an execution running at deploy (D4) or a
//! `CREATE_NEW_EXECUTION` replacement is planned with `plan_stored`, which
//! skips that gate, and on main such a flow published fine. So [`drive`]
//! enforces it in release builds too, in two layers: a run of a refused kind
//! (`Plan::after_execution_refused`) is FAILED before it starts, with the
//! refusal's text as its message ([`refusal_of`]); and a batch that would
//! still write the execution is dropped and the run it was about FAILED.
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
    // Layer 1 (final-review fix, 2026-10-28): a run of a kind D12 refuses
    // here is FAILED before it can start. Only a STORED source reaches this
    // (`build_plan` refuses such a flow at start); see
    // `Plan::after_execution_refused`.
    let refused = fail_refused(plan, state, now);
    if !refused.is_empty() {
        return refused;
    }
    let cmds = match sequential::drive_list(plan, state, &plan.after_execution, None, now) {
        Some(Step::Commands(cmds)) => cmds,
        Some(Step::Waiting | Step::Done(_)) | None => Vec::new(),
    };
    // Layer 2: whatever reaches here, nothing of the phase writes the
    // execution, in release builds too.
    guard(plan, state, cmds, now)
}

/// Layer 1: every live run of the phase whose task
/// [`Plan::after_execution_refused`] names, failed without being run --
/// `FailUnstarted` at `CREATED` (one attempt `[FAILED]`), `SetRunState
/// FAILED` otherwise. The worker writes the refusal's text as the run's
/// message ([`refusal_of`]).
fn fail_refused(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    if plan.after_execution_refused.is_empty() {
        return Vec::new();
    }
    (0..state.runs.len())
        .filter(|&ix| {
            plan.after_execution_refused
                .contains_key(&state.runs[ix].task)
        })
        .filter(|&ix| in_phase(plan, state, ix))
        .filter_map(|ix| fail(state, ix, now))
        .collect()
}

/// The command that fails a live run, or `None` for one that is terminal or
/// discarded.
fn fail(state: &ExecutionState, run: RunIx, now: Tick) -> Option<Command> {
    match state.runs[run].history.last() {
        None => None,
        Some(s) if s.is_terminal() => None,
        Some(RunState::Created) => Some(Command::FailUnstarted { run, at: now }),
        Some(_) => Some(Command::SetRunState {
            run,
            state: RunState::Failed,
        }),
    }
}

/// Layer 2: a batch holding a command that writes the execution
/// ([`writes_the_execution`]) is dropped whole, and the runs it was about
/// are FAILED instead: the run a `PauseRun`/`ScheduleRetry` names, and for a
/// `SetExecutionState` every run of the phase the rest of the batch names --
/// or, if it names none, every live run of the phase with no live child.
/// Layer 1 makes this unreachable for every kind known today; it is the
/// release-build backstop the `debug_assert!` used to be alone.
fn guard(plan: &Plan, state: &ExecutionState, cmds: Vec<Command>, now: Tick) -> Vec<Command> {
    if !cmds.iter().any(writes_the_execution) {
        return cmds;
    }
    let mut runs: Vec<RunIx> = Vec::new();
    for cmd in &cmds {
        match cmd {
            Command::PauseRun { run, .. } | Command::ScheduleRetry { run, .. } => runs.push(*run),
            Command::SetExecutionState { .. } => {}
            other => runs.extend(named_run(other)),
        }
    }
    runs.retain(|&ix| ix < state.runs.len() && in_phase(plan, state, ix));
    if runs.is_empty() {
        runs = (0..state.runs.len())
            .filter(|&ix| in_phase(plan, state, ix))
            .filter(|&ix| fail(state, ix, now).is_some())
            .filter(|&ix| {
                !state
                    .runs
                    .iter()
                    .enumerate()
                    .any(|(c, r)| r.parent == Some(ix) && fail(state, c, now).is_some())
            })
            .collect();
    }
    runs.sort_unstable();
    runs.dedup();
    runs.into_iter()
        .filter_map(|ix| fail(state, ix, now))
        .collect()
}

/// The existing run a command is about, if any.
fn named_run(cmd: &Command) -> Option<RunIx> {
    match cmd {
        Command::StartRun { run, .. }
        | Command::SkipRun { run, .. }
        | Command::FailUnstarted { run, .. }
        | Command::ScheduleRetry { run, .. }
        | Command::PauseRun { run, .. }
        | Command::RestartLoop { run }
        | Command::RecreateRun { run }
        | Command::SetRunState { run, .. }
        | Command::CompleteRun { run, .. }
        | Command::SpawnSubflow { run, .. }
        | Command::BeginWorkingDirectory { run }
        | Command::FinishWorkingDirectory { run }
        | Command::CompleteWorkingDirectoryClaim { run } => Some(*run),
        Command::SpawnExecution { parent_run, .. } => Some(*parent_run),
        Command::CreateRun { parent, .. } => *parent,
        Command::SetExecutionState { .. } => None,
    }
}

/// The message a FAILED run of the phase carries when layer 1 failed it:
/// the D12 refusal of its task (`spec::validate::after_execution_refusal`'s
/// text), or `None`. A refused task's run fails only through layer 1 (it is
/// never started), so the worker can attach the text to any failure of it.
pub fn refusal_of<'a>(plan: &'a Plan, state: &ExecutionState, run: RunIx) -> Option<&'a str> {
    let task = state.runs.get(run)?.task;
    let message = plan.after_execution_refused.get(&task)?;
    in_phase(plan, state, run).then_some(message.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::state::apply;

    /// A terminal `SUCCESS` execution whose one after-task `end` is
    /// `RUNNING`.
    fn running_after() -> (Plan, ExecutionState, RunIx) {
        let plan = crate::executor::plan_stored(
            "id: f\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\nafterExecution:\n  - id: end\n    type: io.kestra.plugin.core.log.Log\n    message: e\n",
        )
        .unwrap();
        let mut state = ExecutionState::new(&plan);
        for s in [RunState::Running, RunState::Success] {
            apply(&mut state, &Command::SetExecutionState { state: s });
        }
        let end = plan.after_execution[0];
        apply(
            &mut state,
            &Command::CreateRun {
                task: end,
                parent: None,
                value: None,
            },
        );
        apply(
            &mut state,
            &Command::StartRun {
                run: 0,
                at: Tick(0),
            },
        );
        (plan, state, 0)
    }

    /// Layer 2, in release builds too: a batch that would write the ended
    /// execution is dropped whole, and the run it was about is FAILED.
    #[test]
    fn the_guard_replaces_an_execution_write_by_a_failed_run() {
        let (plan, state, run) = running_after();
        let fail = vec![Command::SetRunState {
            run,
            state: RunState::Failed,
        }];
        assert_eq!(
            guard(
                &plan,
                &state,
                vec![
                    Command::SetRunState {
                        run,
                        state: RunState::Success
                    },
                    Command::SetExecutionState {
                        state: RunState::Failed
                    },
                ],
                Tick(0)
            ),
            fail
        );
        assert_eq!(
            guard(
                &plan,
                &state,
                vec![Command::ScheduleRetry {
                    run,
                    not_before: Tick(9)
                }],
                Tick(0)
            ),
            fail
        );
        // A bare `SetExecutionState` names no run: the live leaf of the
        // phase is failed.
        assert_eq!(
            guard(
                &plan,
                &state,
                vec![Command::SetExecutionState {
                    state: RunState::Killing
                }],
                Tick(0)
            ),
            fail
        );
        // A batch that writes nothing of the execution passes untouched.
        let ok = vec![Command::SetRunState {
            run,
            state: RunState::Success,
        }];
        assert_eq!(guard(&plan, &state, ok.clone(), Tick(0)), ok);
    }
}
