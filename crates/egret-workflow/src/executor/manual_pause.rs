//! Pause by hand and resume by hand -- workflow package 7c (spec
//! `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D1-D3, D5;
//! Kestra `ExecutionService.pause`, `ES:907-916`, and `.resume`,
//! `ES:880-897`).
//!
//! Pure, like `executor::restart`: these functions say what a manual pause
//! and a manual resume write. The worker (`worker::pause`) takes the
//! execution's lock, applies them and settles.

use super::command::Command;
use super::state::{ExecutionState, Resume, RunIx, RunState};

/// **D1: paused by hand** -- the execution's last state is `PAUSED` and no
/// run's last state is (Kestra `ES.resume`'s `findFirstByState(PAUSED)`
/// empty). A `Pause` task or a waiting `LoopUntil` always leaves a `PAUSED`
/// run (`Command::PauseRun` is the only producer of one, and `pause_mark`
/// pauses the execution only in a batch holding one), so the two cannot be
/// confused.
pub fn paused_by_hand(state: &ExecutionState) -> bool {
    paused_by_hand_of(
        state.history.last().copied(),
        state.runs.iter().map(|r| r.history.last().copied()),
    )
}

/// [`paused_by_hand`]'s rule over the last states alone, for a reader that
/// holds rows rather than an `ExecutionState` (`routes::ExecutionDto`). One
/// rule, two shapes of input.
pub fn paused_by_hand_of(
    execution: Option<RunState>,
    mut runs: impl Iterator<Item = Option<RunState>>,
) -> bool {
    execution == Some(RunState::Paused) && !runs.any(|r| r == Some(RunState::Paused))
}

/// Why an execution cannot be paused by hand (D3, Q5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseRefusal {
    /// Not `RUNNING` or `RESTARTED`; the last state.
    NotRunning(Option<RunState>),
    /// A run's last state is `PAUSED`: a `Pause` or a `LoopUntil` is
    /// already waiting (spec Q5, §5 divergence 8). Paused now, the
    /// execution would not be "paused by hand" (D1), and that run's timer
    /// could end the pause.
    TaskWaiting,
}

/// **D3**: a `RUNNING` or `RESTARTED` execution with no waiting run takes
/// `PAUSED`; no task run changes. Every other state is refused with itself
/// -- `KILLING` too, which Kestra's `isRunning()` accepts (spec §5
/// divergence 1): `PAUSED` after `KILLING` would stop `advance_killing`.
pub fn pause_by_hand_commands(state: &ExecutionState) -> Result<Vec<Command>, PauseRefusal> {
    match state.history.last().copied() {
        Some(RunState::Running | RunState::Restarted) => {
            if state
                .runs
                .iter()
                .any(|r| r.history.last() == Some(&RunState::Paused))
            {
                return Err(PauseRefusal::TaskWaiting);
            }
            Ok(vec![Command::SetExecutionState {
                state: RunState::Paused,
            }])
        }
        other => Err(PauseRefusal::NotRunning(other)),
    }
}

/// **D5, paused by hand**: the execution goes back to `RUNNING` (Kestra
/// `ES:880-897`), and the next `advance` drives it. Empty when the
/// execution is not paused by hand.
pub fn resume_by_hand_commands(state: &ExecutionState) -> Vec<Command> {
    if paused_by_hand(state) {
        vec![Command::SetExecutionState {
            state: RunState::Running,
        }]
    } else {
        Vec::new()
    }
}

/// **D5**: the first `PAUSED` `Pause` run in run order -- the one whose
/// `onResume` a resume reads (Kestra `findFirstByState(PAUSED)`). A
/// `LoopUntil`'s wait (`Resume::Iterate`) is skipped: a resume does not end
/// it. The same predicate as `flowable::pause::release_now`.
pub fn first_waiting_pause(state: &ExecutionState) -> Option<RunIx> {
    state.runs.iter().position(|r| {
        r.history.last() == Some(&RunState::Paused)
            && matches!(r.resume_with, Some(Resume::Never | Resume::Terminal { .. }))
    })
}

/// **D5**: the outputs of a `Pause` run released by hand (`Pause.java:305-330`):
/// `{"onResume": {...}, "resumed": {"by": null, "on": <ISO-8601>, "to":
/// "SUCCESS"}}`. `by` stays null, as in Kestra OSS (`Pause.Resumed.now()`);
/// who resumed is in the audit row. A run with no `onResume` gets `{}` so a
/// template reading it does not fail.
pub fn resumed_outputs(
    on_resume: serde_json::Map<String, serde_json::Value>,
    on: &str,
) -> serde_json::Value {
    serde_json::json!({
        "onResume": on_resume,
        "resumed": { "by": null, "on": on, "to": "SUCCESS" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_support::{driven, flow, last, leaf, run, T0};
    use crate::executor::{advance, apply, build_plan, release_paused_pauses, Plan, Tick};

    /// `advance` alone to a fixed point: no leaf ends (a worker would).
    fn rest(plan: &Plan, state: &mut ExecutionState, now: Tick) {
        for _ in 0..100 {
            let cmds = advance(plan, state, now);
            if cmds.is_empty() {
                return;
            }
            for c in &cmds {
                apply(state, c);
            }
        }
        panic!("advance did not come to rest");
    }

    fn started(src: &str) -> (Plan, ExecutionState) {
        let plan = build_plan(src).expect("the fixture flow plans");
        let mut state = ExecutionState::new(&plan);
        rest(&plan, &mut state, T0);
        (plan, state)
    }

    fn paused(state: &mut ExecutionState) {
        for c in pause_by_hand_commands(state).expect("running") {
            apply(state, &c);
        }
    }

    const PAUSE: &str = "io.kestra.plugin.core.flow.Pause";

    // 1. D1 against each kind of PAUSED.
    #[test]
    fn paused_by_hand_tells_a_manual_pause_from_every_pause_that_leaves_a_paused_run() {
        // Manual.
        let (_, mut state) = started(&flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2))));
        paused(&mut state);
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(paused_by_hand(&state));

        // A `Pause` task.
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: p\n    type: {PAUSE}\n{}",
                leaf("after", 2)
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // A waiting `LoopUntil` (the drive answers its condition `false`).
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    tasks:\n{}",
                leaf("body", 6)
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // Two `Pause`s in a `Parallel`.
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n"
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // The row form agrees.
        assert!(paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Running), Some(RunState::Success)].into_iter()
        ));
        assert!(!paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Paused)].into_iter()
        ));
        assert!(!paused_by_hand_of(
            Some(RunState::Running),
            std::iter::empty()
        ));
    }

    // 2. D2.
    #[test]
    fn drives_forward_is_false_for_a_manual_pause_and_true_for_a_pause_task() {
        let (_, mut state) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        assert!(crate::executor::drives_forward(&state));
        paused(&mut state);
        assert!(!crate::executor::drives_forward(&state));

        let (_, state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")),
            &[],
        );
        assert!(
            crate::executor::drives_forward(&state),
            "a Pause task's PAUSED still drives"
        );
    }

    // 3. D2: a run ending during the pause creates nothing and completes nothing.
    #[test]
    fn a_run_ending_during_a_manual_pause_creates_and_completes_nothing() {
        let src = flow(&format!(
            "tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}{}{}",
            leaf("a", 6),
            leaf("b", 6),
            leaf("c", 2)
        ));
        let (plan, mut state) = started(&src);
        let a = run(&plan, &state, "a");
        assert_eq!(last(&state, a), Some(RunState::Running));
        paused(&mut state);

        apply(
            &mut state,
            &Command::SetRunState {
                run: a,
                state: RunState::Success,
            },
        );
        let before = state.runs.len();
        assert!(
            advance(&plan, &state, Tick(10)).is_empty(),
            "nothing is created or started"
        );
        assert_eq!(state.runs.len(), before, "b has no run");
        let seq = run(&plan, &state, "seq");
        assert_eq!(
            last(&state, seq),
            Some(RunState::Running),
            "the parent is not completed"
        );

        // A run that FAILS during the pause is not retried, failed upward or handled either.
        let src = flow(&format!("tasks:\n{}{}", leaf("x", 2), leaf("y", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let x = run(&plan, &state, "x");
        apply(
            &mut state,
            &Command::SetRunState {
                run: x,
                state: RunState::Failed,
            },
        );
        assert!(advance(&plan, &state, Tick(10)).is_empty());
        assert_eq!(state.history.last(), Some(&RunState::Paused));
    }

    // 4. D5: resume by hand gives RUNNING, and the next advance creates the next run.
    #[test]
    fn resume_by_hand_gives_running_and_the_next_advance_creates_the_next_run() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let a = run(&plan, &state, "a");
        apply(
            &mut state,
            &Command::SetRunState {
                run: a,
                state: RunState::Success,
            },
        );

        let cmds = resume_by_hand_commands(&state);
        assert_eq!(
            cmds,
            vec![Command::SetExecutionState {
                state: RunState::Running
            }]
        );
        for c in &cmds {
            apply(&mut state, c);
        }
        rest(&plan, &mut state, Tick(20));
        let b = run(&plan, &state, "b");
        assert_eq!(last(&state, b), Some(RunState::Running));
        assert_eq!(
            &state.history[state.history.len() - 3..],
            &[RunState::Running, RunState::Paused, RunState::Running],
            "Kestra's ES.resume: PAUSED back to RUNNING, no RESTARTED"
        );

        // Not paused by hand: nothing.
        let (_, state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")),
            &[],
        );
        assert!(resume_by_hand_commands(&state).is_empty());
    }

    // 5. D3: only RUNNING and RESTARTED are paused.
    #[test]
    fn pause_by_hand_refuses_every_state_but_running_and_restarted() {
        let (_, base) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        for s in RunState::ALL {
            let mut state = base.clone();
            state.history.push(*s);
            let got = pause_by_hand_commands(&state);
            match s {
                RunState::Running | RunState::Restarted => assert!(got.is_ok(), "{s:?}"),
                other => assert_eq!(got, Err(PauseRefusal::NotRunning(Some(*other))), "{s:?}"),
            }
        }
    }

    // 5b. Q5 (S3): refused while a run waits, even when the execution is RUNNING.
    #[test]
    fn pause_by_hand_is_refused_while_a_pause_waits() {
        // Two `Pause`s that end at different times: after `quick` ends the
        // execution is RUNNING while `slow` is still PAUSED.
        let plan = build_plan(&flow(&format!(
            "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: quick\n        type: {PAUSE}\n        pauseDuration: PT1S\n      - id: slow\n        type: {PAUSE}\n        pauseDuration: PT1H\n"
        )))
        .expect("plans");
        let mut state = ExecutionState::new(&plan);
        rest(&plan, &mut state, T0);
        rest(&plan, &mut state, Tick(1_000));
        assert_eq!(
            state.history.last(),
            Some(&RunState::Running),
            "{:?}",
            state.history
        );
        assert_eq!(
            last(&state, run(&plan, &state, "slow")),
            Some(RunState::Paused)
        );

        assert_eq!(
            pause_by_hand_commands(&state),
            Err(PauseRefusal::TaskWaiting)
        );
    }

    // 6. D5: the first PAUSED Pause in run order, never a LoopUntil; and the payload.
    #[test]
    fn the_first_waiting_pause_is_a_pause_in_run_order_and_its_payload_is_kestras() {
        let (plan, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: lu\n        type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{{{ false }}}}\"\n        checkFrequency:\n          interval: PT1H\n          maxIterations: 3\n        tasks:\n{}      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n",
                leaf("body", 10)
            )),
            &[],
        );
        let p1 = run(&plan, &state, "p1");
        assert_eq!(first_waiting_pause(&state), Some(p1));

        let mut map = serde_json::Map::new();
        map.insert("ok".into(), serde_json::json!(true));
        assert_eq!(
            resumed_outputs(map, "2026-11-15T10:00:00.000Z"),
            serde_json::json!({
                "onResume": {"ok": true},
                "resumed": {"by": null, "on": "2026-11-15T10:00:00.000Z", "to": "SUCCESS"}
            })
        );
    }

    // 7. D5: a released Pause still ends SUCCESS and the execution goes RESTARTED.
    #[test]
    fn releasing_a_pause_still_restarts_the_execution() {
        let (plan, mut state) = driven(
            &flow(&format!(
                "tasks:\n  - id: p\n    type: {PAUSE}\n{}",
                leaf("after", 2)
            )),
            &[],
        );
        let released = release_paused_pauses(&mut state, Tick(10));
        assert_eq!(released, vec![run(&plan, &state, "p")]);
        let cmds = advance(&plan, &state, Tick(10));
        assert!(
            cmds.contains(&Command::SetExecutionState {
                state: RunState::Restarted
            }),
            "{cmds:?}"
        );
    }
}
