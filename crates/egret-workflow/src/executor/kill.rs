//! `advance()` while the execution is `KILLING` — spec
//! `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md` §5.
//!
//! The kill route (§3.2) has already done the part that needs a lease: a leased
//! leaf run is `KILLING` (a worker finishes it, §4), and every other non-terminal
//! leaf run and every `Subflow` run is `KILLED`. What is left is the part only
//! the executor can do, and it is Kestra's `ExecutorService.java:313-335`:
//!
//! * no run is created, started, retried or resumed;
//! * a flowable run that is not terminal becomes `KILLING`, and then `KILLED`
//!   once every one of its child runs is terminal;
//! * the execution becomes `KILLED` once every run is terminal.
//!
//! # A leaf is never written here
//!
//! A leaf (`Runnable`, `Executable`) that is still live under a `KILLING`
//! execution is either `KILLING` — a worker holds it and will write its
//! `KILLED` — or a row the route should have killed and did not. Either way
//! this function writes nothing onto it: it does not invent the worker's half,
//! and it does not resume, retry or start it. The execution then stays
//! `KILLING` until something terminates that run, which is visible rather than
//! a guess.
//!
//! # Why a `PAUSED` flowable IS written, and no `RESTARTED` follows
//!
//! A `Pause` or a `LoopUntil` between iterations is a flowable that may be
//! `PAUSED` when the kill lands. The route leaves flowables to this function,
//! so it moves such a run `KILLING` and then `KILLED` like any other flowable;
//! leaving it would leave the execution `KILLING` for ever. `advance()`'s
//! `pause_mark` — which reads a `SetRunState` onto a `PAUSED` run as "a pause
//! ended" and marks the execution `RESTARTED` — is never applied to this
//! function's output, because `advance` returns it directly.
//!
//! # The attempt ends `KILLED`
//!
//! The commands are plain [`Command::SetRunState`]s, the shape every flowable
//! terminal already takes (`flowable::descend`'s `Step::Done` arm). `apply`
//! pushes each state onto the run's open attempt too, and `KILLING` is not
//! terminal, so a flowable's attempt reads `CREATED RUNNING KILLING KILLED` and
//! is closed by the `KILLED`; `worker::stamps::stamp_attempt_ends` stamps its
//! `ended_at` like any other closed attempt.

use super::command::Command;
use super::plan::{Plan, TaskKind};
use super::state::{ExecutionState, RunIx, RunState};

/// One round of closing a `KILLING` execution. Pure, like
/// [`advance`](super::advance), which calls it and returns its result when the
/// execution's last state is [`RunState::Killing`].
///
/// One level per round: a flowable is `KILLED` only when every child run is
/// already terminal, so nested flowables close innermost first, one per round.
pub(crate) fn advance_killing(plan: &Plan, state: &ExecutionState) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        // An empty history is `Command::RestartLoop`'s "this run does not exist
        // right now"; nothing is owed to it.
        let Some(&last) = run.history.last() else {
            continue;
        };
        if last.is_terminal() || is_leaf(&plan.tasks[run.task].kind) {
            continue;
        }
        if last != RunState::Killing {
            cmds.push(Command::SetRunState {
                run: ix,
                state: RunState::Killing,
            });
        } else if children_terminal(state, ix) {
            cmds.push(Command::SetRunState {
                run: ix,
                state: RunState::Killed,
            });
        }
    }
    if cmds.is_empty() && state.runs.iter().all(is_settled) {
        cmds.push(Command::SetExecutionState {
            state: RunState::Killed,
        });
    }
    cmds
}

/// A task a worker (`Runnable`) or a child execution (`Executable`) ends, not
/// the executor.
fn is_leaf(kind: &TaskKind) -> bool {
    matches!(
        kind,
        TaskKind::Runnable { .. } | TaskKind::Executable { .. }
    )
}

/// Terminal, or emptied by `RestartLoop` and so not a run at the moment.
fn is_settled(run: &super::state::TaskRun) -> bool {
    run.history.last().is_none_or(|s| s.is_terminal())
}

fn children_terminal(state: &ExecutionState, parent: RunIx) -> bool {
    state
        .runs
        .iter()
        .filter(|r| r.parent == Some(parent))
        .all(is_settled)
}

#[cfg(test)]
mod tests {
    use super::super::state::{apply, Attempt, ExecutionMeta, Resume, TaskRun, Tick};
    use super::super::{advance, build_plan, TaskIx};
    use super::*;

    use RunState::{Created, Killed, Killing, Paused, Retrying, Running, Success};

    /// Late enough that every retry and every bounded pause in these fixtures
    /// is due: a guard that let `retry::reopen_due` or `resume_paused` run would
    /// have something to do.
    const LATE: Tick = Tick(1_000_000_000);

    fn plan(tasks: &str) -> Plan {
        build_plan(&format!("id: k\nnamespace: t\ntasks:\n{tasks}")).expect("plans")
    }

    fn task(plan: &Plan, id: &str) -> TaskIx {
        plan.tasks
            .iter()
            .position(|t| t.id == id)
            .unwrap_or_else(|| panic!("no task {id}"))
    }

    /// A run with `history`, and one attempt opened at its first `RUNNING`
    /// that holds every state after it, the way `StartRun` + `apply` build one.
    fn run(task: TaskIx, parent: Option<RunIx>, history: &[RunState]) -> TaskRun {
        let attempts = match history.iter().position(|s| *s == Running) {
            Some(at) => {
                let mut h = vec![Created];
                h.extend(history[at..].iter().filter(|s| **s != Paused));
                vec![Attempt {
                    history: h,
                    started_at: Tick(0),
                    ended_at: None,
                }]
            }
            None => Vec::new(),
        };
        TaskRun {
            task,
            parent,
            value: None,
            rendered: None,
            rendered_run_if: None,
            history: history.to_vec(),
            attempts,
            not_before: None,
            resume_with: None,
            working_dir: None,
        }
    }

    fn killing(runs: Vec<TaskRun>) -> ExecutionState {
        ExecutionState {
            runs,
            history: vec![Created, Running, Killing],
            metadata: ExecutionMeta::default(),
        }
    }

    fn set_run(run: RunIx, state: RunState) -> Command {
        Command::SetRunState { run, state }
    }

    fn set_exec(state: RunState) -> Command {
        Command::SetExecutionState { state }
    }

    /// Every round `advance` emits until it emits nothing, applied in order.
    fn rounds(plan: &Plan, state: &mut ExecutionState) -> Vec<Vec<Command>> {
        let mut out = Vec::new();
        for _ in 0..50 {
            let cmds = advance(plan, state, LATE);
            if cmds.is_empty() {
                return out;
            }
            for c in &cmds {
                apply(state, c);
            }
            out.push(cmds);
        }
        panic!("advance did not settle in 50 rounds: {out:?}");
    }

    const LEAF: &str = "  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

    const SEQ3: &str = "  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: a\n      - id: b\n        type: io.kestra.plugin.core.log.Log\n        message: b\n      - id: c\n        type: io.kestra.plugin.core.log.Log\n        message: c\n";

    // Case 1.
    #[test]
    fn a_killing_leaf_is_waited_for() {
        let p = plan(LEAF);
        let s = killing(vec![run(task(&p, "a"), None, &[Created, Running, Killing])]);
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
    }

    // Case 2.
    #[test]
    fn a_killed_leaf_ends_the_execution_killed() {
        let p = plan(LEAF);
        let s = killing(vec![run(
            task(&p, "a"),
            None,
            &[Created, Running, Killing, Killed],
        )]);
        assert_eq!(advance(&p, &s, LATE), vec![set_exec(Killed)]);
    }

    // Case 3.
    #[test]
    fn a_running_parent_over_a_killing_child_becomes_killing_only() {
        let p = plan(SEQ3);
        let s = killing(vec![
            run(task(&p, "seq"), None, &[Created, Running]),
            run(task(&p, "a"), Some(0), &[Created, Running, Success]),
            run(task(&p, "b"), Some(0), &[Created, Running, Killing]),
        ]);
        assert_eq!(advance(&p, &s, LATE), vec![set_run(0, Killing)]);
    }

    // Case 4.
    #[test]
    fn a_killing_parent_over_terminal_children_is_killed_then_the_execution() {
        let p = plan(SEQ3);
        let mut s = killing(vec![
            run(task(&p, "seq"), None, &[Created, Running, Killing]),
            run(task(&p, "a"), Some(0), &[Created, Running, Success]),
            run(task(&p, "b"), Some(0), &[Created, Running, Killing, Killed]),
        ]);
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]
        );
        // Item 4: the flowable's attempt is closed by the KILLED.
        assert_eq!(
            s.runs[0].attempts.last().unwrap().history,
            vec![Created, Running, Killing, Killed]
        );
    }

    // Case 5.
    #[test]
    fn nested_flowables_close_innermost_first_one_level_per_round() {
        let p = plan(
            "  - id: outer\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: x\n            type: io.kestra.plugin.core.log.Log\n            message: x\n",
        );
        let mut s = killing(vec![
            run(task(&p, "outer"), None, &[Created, Running]),
            run(task(&p, "inner"), Some(0), &[Created, Running]),
            run(task(&p, "x"), Some(1), &[Created, Running, Killing]),
        ]);
        // Both flowables go KILLING; neither closes while `x` is live.
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![set_run(0, Killing), set_run(1, Killing)]]
        );
        // The worker's half: `x` ends KILLED (spec §4.2).
        apply(&mut s, &set_run(2, Killed));
        assert_eq!(
            rounds(&p, &mut s),
            vec![
                vec![set_run(1, Killed)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
        // Item 4, for both levels: run and attempt end KILLING KILLED.
        for ix in [0, 1] {
            assert_eq!(s.runs[ix].history, vec![Created, Running, Killing, Killed]);
            assert_eq!(
                s.runs[ix].attempts.last().unwrap().history,
                vec![Created, Running, Killing, Killed]
            );
        }
        assert_eq!(s.history, vec![Created, Running, Killing, Killed]);
    }

    // Case 6.
    #[test]
    fn a_task_not_yet_created_is_not_created() {
        let p = plan(SEQ3);
        let mut s = killing(vec![
            run(task(&p, "seq"), None, &[Created, Running]),
            run(task(&p, "a"), Some(0), &[Created, Running, Success]),
        ]);
        assert_eq!(
            rounds(&p, &mut s),
            vec![
                vec![set_run(0, Killing)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
        assert_eq!(s.runs.len(), 2, "no run was created for b or c");
    }

    // Case 7.
    #[test]
    fn a_parallel_starts_no_new_child() {
        let p = plan(
            "  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    concurrent: 2\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: a\n      - id: b\n        type: io.kestra.plugin.core.log.Log\n        message: b\n      - id: c\n        type: io.kestra.plugin.core.log.Log\n        message: c\n",
        );
        let s = killing(vec![
            run(task(&p, "par"), None, &[Created, Running]),
            run(task(&p, "a"), Some(0), &[Created, Running, Killing]),
            run(task(&p, "b"), Some(0), &[Created, Running, Success]),
        ]);
        assert_eq!(advance(&p, &s, LATE), vec![set_run(0, Killing)]);
    }

    // Case 8.
    #[test]
    fn a_due_retry_is_not_reopened() {
        let p = plan(
            "  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 3\n",
        );
        let mut r = run(task(&p, "a"), None, &[Created, Running, Retrying]);
        r.not_before = Some(Tick(0));
        let s = killing(vec![r]);
        // The control: the same state under a RUNNING execution IS reopened, so
        // the empty answer below is the kill's and not a fixture that is never due.
        let mut live = s.clone();
        live.history = vec![Created, Running];
        assert!(
            !advance(&p, &live, LATE).is_empty(),
            "the control: a due retry under a RUNNING execution must be reopened"
        );
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
    }

    // Item 2: a PAUSED leaf is not written, so nothing can read it as a resume.
    #[test]
    fn a_paused_leaf_is_left_alone_and_no_restarted_is_marked() {
        let p = plan(LEAF);
        let mut r = run(task(&p, "a"), None, &[Created, Running, Paused]);
        r.not_before = Some(Tick(0));
        r.resume_with = Some(Resume::Terminal {
            at: Tick(0),
            state: Success,
        });
        let s = killing(vec![r]);
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
    }

    // Item 2, the flowable half: a PAUSED `Pause` is closed, never resumed,
    // and the execution never reads RESTARTED.
    #[test]
    fn a_paused_pause_is_killed_and_no_restarted_is_marked() {
        let p = plan(
            "  - id: wait\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n",
        );
        let mut r = run(task(&p, "wait"), None, &[Created, Running, Paused]);
        r.not_before = Some(Tick(0));
        r.resume_with = Some(Resume::Terminal {
            at: Tick(0),
            state: Success,
        });
        let mut s = killing(vec![r]);
        let all = rounds(&p, &mut s);
        assert_eq!(
            all,
            vec![
                vec![set_run(0, Killing)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
        assert!(!s.history.contains(&RunState::Restarted));
        assert_eq!(
            s.runs[0].history,
            vec![Created, Running, Paused, Killing, Killed]
        );
        assert_eq!(
            s.runs[0].attempts.last().unwrap().history,
            vec![Created, Running, Killing, Killed]
        );
    }

    // Robustness: a CREATED leaf the route missed is not started.
    #[test]
    fn a_created_leaf_is_not_started() {
        let p = plan(LEAF);
        let s = killing(vec![run(task(&p, "a"), None, &[Created])]);
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
    }

    // A KILLING execution with no run at all ends at once.
    #[test]
    fn a_killing_execution_with_no_run_ends_killed() {
        let p = plan(LEAF);
        let s = killing(Vec::new());
        assert_eq!(advance(&p, &s, LATE), vec![set_exec(Killed)]);
    }
}
