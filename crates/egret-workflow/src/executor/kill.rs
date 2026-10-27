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
//! # A leaf nobody killed is killed here (final review C1, 2026-10-18)
//!
//! A leaf (`Runnable`, `Executable`) that is `KILLING` is waited for: a worker
//! holds it and writes its `KILLED`, or the kill-only claim does (spec §4.3).
//! This function never invents that half.
//!
//! **Any other live leaf is a row nothing else will kill**, and until the
//! final review this function left it, on the reading that the route had
//! already killed every leaf. It had not always: an `Exit(KILLED)`'s round
//! could create or start a sibling's run beside the kill (`exit::killed_in`
//! now stops that at the source), and a `CREATED` leaf under a `KILLING`
//! execution is never claimed, so the execution stayed `KILLING` for ever. So
//! it is written as the route writes it (`leaf_kill`): a running `Runnable`
//! (`RUNNING` or `SUBMITTED`) becomes `KILLING` -- its holder, or the
//! kill-only claim, cancels it -- and every other live leaf becomes `KILLED`.
//! It is not resumed, retried or started. A `WorkingDirectory` block is a
//! flowable and keeps the rules below.
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
//! # A `WorkingDirectory` block waits for its own claim (fix round 1)
//!
//! A block in `PREPARING` or `FINISHING` has a claim with the worker, or one
//! a worker is about to take, and no live child. It is not written until the
//! claim has completed: closing it under a leased claim let a late prepare
//! commit onto a `KILLED` block.
//!
//! **A block that has prepared runs its finish claim in DISCARD-ONLY mode**
//! (fix round 2, decided with the user 2026-10-17). Once it is `KILLING` and
//! its children are terminal, a block in `PREPARED` is asked for
//! [`Command::FinishWorkingDirectory`] instead of being closed; the worker
//! takes a `FINISHING` block in `KILLING` (`claim::claim`), deletes its
//! `_wd/{block}/` objects and writes no `outputFiles`, cache or `outputs`
//! (`claim::run_block_claim`), and completes the claim to `FINISHED`. The
//! next round closes it `KILLED`, as a normal finish leaves a block's terminal
//! to the executor. A block that never prepared has nothing to delete and is
//! closed at once.
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
use super::state::{ExecutionState, RunIx, RunState, WorkingDirPhase};

/// One round of closing a `KILLING` execution. Pure, like
/// [`advance`](super::advance), which calls it and returns its result when the
/// execution's last state is [`RunState::Killing`].
///
/// One level per round: a flowable is `KILLED` only when every child run is
/// already terminal, so nested flowables close innermost first, one per round.
pub(crate) fn advance_killing(plan: &Plan, state: &ExecutionState) -> Vec<Command> {
    let cmds = close_runs(plan, state, |_| true);
    if cmds.is_empty() && state.runs.iter().all(is_settled) {
        return vec![Command::SetExecutionState {
            state: RunState::Killed,
        }];
    }
    cmds
}

/// One round of closing a **killed post-terminal phase** (end hooks #2b, the
/// user's answer to Q3, 2026-10-27): [`advance_killing`]'s rules, applied to
/// the runs of the flow's `afterExecution:` only
/// ([`in_phase`](super::after_execution::in_phase)), and never a command that
/// writes the execution, whose state, history and `ended_at` are those it
/// ended with. Nothing is created, started or retried; a flowable of the
/// phase becomes `KILLING`, then `KILLED` once its children are terminal.
pub(crate) fn close_phase(plan: &Plan, state: &ExecutionState) -> Vec<Command> {
    close_runs(plan, state, |ix| {
        super::after_execution::in_phase(plan, state, ix)
    })
}

/// **The kill route's commands for a terminal execution whose
/// `afterExecution:` is still running** (Q3, a security divergence from
/// Kestra, which answers 409 and force-executes the list): every live run
/// of the phase is killed as [`kill_leaves`] kills a leaf -- a running
/// `Runnable` becomes `KILLING` for its holder or the kill-only claim, every
/// other live leaf `KILLED` -- and every live flowable of the phase becomes
/// `KILLING` at once, so the phase reads as killed
/// ([`phase_killed`](super::after_execution::phase_killed)) even when no
/// leaf of it is live. A `WorkingDirectory` block whose prepare or finish
/// claim is out is left `RUNNING`, as [`advance_killing`] leaves it, so the
/// claim can still commit.
///
/// No `SetExecutionState`: the execution is not moved.
pub(crate) fn kill_phase(plan: &Plan, state: &ExecutionState) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        let Some(&last) = run.history.last() else {
            continue;
        };
        if last.is_terminal()
            || last == RunState::Killing
            || !super::after_execution::in_phase(plan, state, ix)
        {
            continue;
        }
        let kind = &plan.tasks[run.task].kind;
        let to = if is_leaf(kind) {
            leaf_kill(kind, last)
        } else if matches!(
            run.working_dir,
            Some(WorkingDirPhase::Preparing | WorkingDirPhase::Finishing)
        ) {
            continue;
        } else {
            RunState::Killing
        };
        cmds.push(Command::SetRunState { run: ix, state: to });
    }
    cmds
}

/// [`advance_killing`]'s per-run rules over the runs `only` admits.
fn close_runs(plan: &Plan, state: &ExecutionState, only: impl Fn(RunIx) -> bool) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        if !only(ix) {
            continue;
        }
        // An empty history is `Command::RestartLoop`'s "this run does not exist
        // right now"; nothing is owed to it.
        let Some(&last) = run.history.last() else {
            continue;
        };
        if last.is_terminal() {
            continue;
        }
        let kind = &plan.tasks[run.task].kind;
        if is_leaf(kind) {
            // The backstop (final review C1 (b)): see the module doc's
            // "A leaf nobody killed is killed here".
            if last != RunState::Killing {
                cmds.push(Command::SetRunState {
                    run: ix,
                    state: leaf_kill(kind, last),
                });
            }
            continue;
        }
        // A `WorkingDirectory` block whose prepare or finish claim is out --
        // claimable or leased -- is waited for, as a leased leaf is. The block
        // stays `RUNNING` so the claim can still be taken and its commit
        // passes the block-claim fence (`claim::apply_block_claim_on`, which
        // refuses a block that is not `RUNNING`); a finish that commits
        // discards the block's snapshots, as it always has.
        if matches!(
            run.working_dir,
            Some(WorkingDirPhase::Preparing | WorkingDirPhase::Finishing)
        ) {
            continue;
        }
        if last != RunState::Killing {
            cmds.push(Command::SetRunState {
                run: ix,
                state: RunState::Killing,
            });
        } else if children_terminal(state, ix) {
            // A block that has prepared may have `_wd/{block}/` objects, and
            // only its finish claim deletes them: it is asked for in
            // discard-only mode (the claim reads the block's `KILLING`), and
            // the block is closed once that claim has completed (`Finished`).
            // Fix round 2, the user's ruling of 2026-10-17.
            if run.working_dir == Some(WorkingDirPhase::Prepared) {
                cmds.push(Command::FinishWorkingDirectory { run: ix });
            } else {
                cmds.push(Command::SetRunState {
                    run: ix,
                    state: RunState::Killed,
                });
            }
        }
    }
    cmds
}

/// **The kill route's leaf steps** (spec §3.2 steps 1, 2 and 4), as commands
/// for the route to apply to a loaded execution before its `settle`:
///
/// 1. a `Runnable` that is running (`RUNNING` or `SUBMITTED`) becomes
///    `KILLING`, **whether or not a worker holds it**: a holder stops it and
///    writes `KILLED` (spec §4.2), and with no lease the kill-only claim
///    cancels it by run and attempt and writes `KILLED` (spec §4.3);
/// 2. every other live leaf -- `CREATED`, `RETRYING`, `PAUSED`, and every
///    `Subflow` (`Executable`) run -- becomes `KILLED`;
/// 3. a flowable is left alone: `advance_killing` closes it (§5), and so is a
///    leaf that is already `KILLING`;
/// 4. the execution becomes `KILLING` unless it already is.
///
/// **Why no lease is read** (final review I1, 2026-10-18, superseding Task
/// 6's "unleased `RUNNING` → `KILLED`"): a run whose worker died and was
/// reaped is `RUNNING` with `claimed_at` NULL, yet its container may still be
/// running -- W5a re-attaches to it on the next claim. Writing it `KILLED`
/// asked nobody to stop it, and the orphan sweep never removes a
/// `taskRunner.delete: false` container. `KILLING` hands it to the claim,
/// which cancels it as `exit::kill` already does for the same reason.
pub(crate) fn kill_leaves(plan: &Plan, state: &ExecutionState) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        let Some(&last) = run.history.last() else {
            continue;
        };
        let kind = &plan.tasks[run.task].kind;
        if last.is_terminal() || last == RunState::Killing || !is_leaf(kind) {
            continue;
        }
        cmds.push(Command::SetRunState {
            run: ix,
            state: leaf_kill(kind, last),
        });
    }
    if state.history.last() != Some(&RunState::Killing) {
        cmds.push(Command::SetExecutionState {
            state: RunState::Killing,
        });
    }
    cmds
}

/// What a live, not-yet-`KILLING` leaf becomes under a kill: a `Runnable`
/// that is running (`RUNNING` or `SUBMITTED`) becomes `KILLING`, so whoever
/// may still run it -- the worker that holds it (spec §4.2), or, with no
/// lease, a container a reaped worker left (W5a re-attaches by run and
/// attempt) -- is cancelled by the holder or by the kill-only claim
/// (spec §4.3). Every other live leaf (`CREATED`, `RETRYING`, `PAUSED`, a
/// `Subflow` run) has nothing running and is `KILLED`.
fn leaf_kill(kind: &TaskKind, last: RunState) -> RunState {
    if matches!(kind, TaskKind::Runnable { .. })
        && matches!(last, RunState::Running | RunState::Submitted)
    {
        RunState::Killing
    } else {
        RunState::Killed
    }
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
pub(crate) fn is_settled(run: &super::state::TaskRun) -> bool {
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
        // Final review C1 (b): not reopened, and not left `RETRYING` either --
        // the backstop closes it, and nothing else is asked for.
        assert_eq!(advance(&p, &s, LATE), vec![set_run(0, Killed)]);
    }

    // Item 2: a PAUSED leaf is never resumed, so nothing can read it as a
    // resume; since final review C1 (b) the backstop closes it `KILLED`.
    #[test]
    fn a_paused_leaf_is_killed_and_no_restarted_is_marked() {
        let p = plan(LEAF);
        let mut r = run(task(&p, "a"), None, &[Created, Running, Paused]);
        r.not_before = Some(Tick(0));
        r.resume_with = Some(Resume::Terminal {
            at: Tick(0),
            state: Success,
        });
        let mut s = killing(vec![r]);
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]
        );
        assert!(!s.history.contains(&RunState::Restarted));
        assert_eq!(s.runs[0].history, vec![Created, Running, Paused, Killed]);
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

    // Final review C1 (b), the backstop: a CREATED leaf nobody killed -- the
    // route never saw it, or an `Exit`'s round created it -- is not started
    // and is not left for ever: it is `KILLED`, then the execution.
    #[test]
    fn a_created_leaf_under_a_killing_execution_ends_killed() {
        let p = plan(LEAF);
        let mut s = killing(vec![run(task(&p, "a"), None, &[Created])]);
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]
        );
        assert_eq!(s.runs[0].history, vec![Created, Killed]);
        assert_eq!(s.history, vec![Created, Running, Killing, Killed]);
    }

    // Final review C1 (b): a running `Runnable` nobody moved is `KILLING`, not
    // `KILLED`: a worker may hold it (it stops it, spec §4.2) or the kill-only
    // claim takes it and cancels it by run and attempt (spec §4.3). It is
    // then waited for, and a `Subflow` run is `KILLED` at once.
    #[test]
    fn a_running_leaf_under_a_killing_execution_is_killing_and_a_subflow_killed() {
        let p = plan(
            "  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
             \x20     - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: a\n\
             \x20     - id: b\n        type: io.kestra.plugin.core.log.Log\n        message: b\n\
             \x20     - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
             \x20       flowId: child\n",
        );
        let mut s = killing(vec![
            run(task(&p, "par"), None, &[Created, Running]),
            run(
                task(&p, "a"),
                Some(0),
                &[Created, RunState::Submitted, Running],
            ),
            run(task(&p, "b"), Some(0), &[Created, RunState::Submitted]),
            run(task(&p, "sub"), Some(0), &[Created, Running]),
        ]);
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![
                set_run(0, Killing),
                set_run(1, Killing),
                set_run(2, Killing),
                set_run(3, Killed),
            ]]
        );
        // The worker's half, then the flowable and the execution close.
        apply(&mut s, &set_run(1, Killed));
        apply(&mut s, &set_run(2, Killed));
        assert_eq!(
            rounds(&p, &mut s),
            vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]
        );
    }

    // A KILLING execution with no run at all ends at once.
    #[test]
    fn a_killing_execution_with_no_run_ends_killed() {
        let p = plan(LEAF);
        let s = killing(Vec::new());
        assert_eq!(advance(&p, &s, LATE), vec![set_exec(Killed)]);
    }

    // ---- fix round 1: WorkingDirectory and AllowFailure under a kill --------

    const WD: &str = "  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n";

    fn block(p: &Plan, history: &[RunState], phase: Option<WorkingDirPhase>) -> TaskRun {
        let mut r = run(task(p, "block"), None, history);
        r.working_dir = phase;
        r
    }

    /// No command in any round asks for a prepare or a run. (A finish is
    /// asked for only as the discard-only claim, and each test asserts it.)
    fn no_claim_and_no_run(all: &[Vec<Command>]) {
        for c in all.iter().flatten() {
            assert!(
                !matches!(
                    c,
                    Command::BeginWorkingDirectory { .. }
                        | Command::CreateRun { .. }
                        | Command::StartRun { .. }
                ),
                "a kill asked for {c:?}: {all:?}"
            );
        }
    }

    /// Before the prepare claim was asked for: no claim, nothing to discard,
    /// and the block closes at once.
    #[test]
    fn a_block_killed_before_its_prepare_is_closed_and_asks_for_no_claim() {
        let p = plan(WD);
        let mut s = killing(vec![block(&p, &[Created, Running], None)]);
        let all = rounds(&p, &mut s);
        no_claim_and_no_run(&all);
        assert_eq!(
            all,
            vec![
                vec![set_run(0, Killing)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
    }

    /// A prepare claim that is out is waited for, with the block left
    /// `RUNNING` (so the claim can be taken and its commit passes the fence);
    /// once it completes, the block goes `KILLING`, asks for its discard-only
    /// finish claim (round 2), waits for it, and only then closes. No child is
    /// started.
    #[test]
    fn a_block_whose_prepare_is_out_waits_for_it_then_discards_and_closes() {
        let p = plan(WD);
        let mut s = killing(vec![block(
            &p,
            &[Created, Running],
            Some(WorkingDirPhase::Preparing),
        )]);
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
        // The worker's prepare commits.
        apply(&mut s, &Command::CompleteWorkingDirectoryClaim { run: 0 });
        let first = rounds(&p, &mut s);
        no_claim_and_no_run(&first);
        assert_eq!(
            first,
            vec![
                vec![set_run(0, Killing)],
                vec![Command::FinishWorkingDirectory { run: 0 }],
            ]
        );
        assert_eq!(s.runs[0].working_dir, Some(WorkingDirPhase::Finishing));
        // The worker's discard-only finish commits.
        apply(&mut s, &Command::CompleteWorkingDirectoryClaim { run: 0 });
        let rest = rounds(&p, &mut s);
        no_claim_and_no_run(&rest);
        assert_eq!(rest, vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]);
    }

    /// Children running: the block goes `KILLING`, waits for the killed child,
    /// then asks for its discard-only finish, and closes after it, without
    /// the second child.
    #[test]
    fn a_block_killed_while_a_child_runs_discards_after_it_and_closes() {
        let p = plan(WD);
        let mut s = killing(vec![
            block(&p, &[Created, Running], Some(WorkingDirPhase::Prepared)),
            run(task(&p, "a"), Some(0), &[Created, Running, Killing]),
        ]);
        let first = rounds(&p, &mut s);
        assert_eq!(first, vec![vec![set_run(0, Killing)]]);
        apply(&mut s, &set_run(1, Killed));
        let second = rounds(&p, &mut s);
        assert_eq!(
            second,
            vec![vec![Command::FinishWorkingDirectory { run: 0 }]]
        );
        apply(&mut s, &Command::CompleteWorkingDirectoryClaim { run: 0 });
        let rest = rounds(&p, &mut s);
        for r in [&first, &second, &rest] {
            no_claim_and_no_run(r);
        }
        assert_eq!(rest, vec![vec![set_run(0, Killed)], vec![set_exec(Killed)]]);
        assert_eq!(s.runs.len(), 2, "b was never created");
    }

    /// A finish claim that is out is waited for; its commit (which discards
    /// the snapshots, `claim::run_block_claim`) is what lets the block close.
    #[test]
    fn a_block_whose_finish_is_out_waits_for_it_then_closes() {
        let p = plan(WD);
        let mut s = killing(vec![
            block(&p, &[Created, Running], Some(WorkingDirPhase::Finishing)),
            run(task(&p, "a"), Some(0), &[Created, Running, Success]),
            run(task(&p, "b"), Some(0), &[Created, Running, Killing, Killed]),
        ]);
        assert_eq!(advance(&p, &s, LATE), Vec::<Command>::new());
        apply(&mut s, &Command::CompleteWorkingDirectoryClaim { run: 0 });
        let all = rounds(&p, &mut s);
        no_claim_and_no_run(&all);
        assert_eq!(
            all,
            vec![
                vec![set_run(0, Killing)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
        assert_eq!(s.runs[0].working_dir, Some(WorkingDirPhase::Finished));
    }

    /// IMPORTANT 2: an `AllowFailure` over a KILLED child ends `KILLED`, not
    /// the `WARNING` its own drive would soften a failure to, and its
    /// `finally:` is not run. `advance_killing` never goes through
    /// `allow_failure::drive` or `soften`; this pins that a refactor through
    /// `descend` cannot change it silently.
    #[test]
    fn an_allow_failure_over_a_killed_child_ends_killed_and_runs_no_finally() {
        let p = plan(
            "  - id: af\n    type: io.kestra.plugin.core.flow.AllowFailure\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: ko\n        type: io.kestra.plugin.core.debug.Return\n        format: ko\n    finally:\n      - id: fin\n        type: io.kestra.plugin.core.debug.Return\n        format: f\n",
        );
        let mut s = killing(vec![
            run(task(&p, "af"), None, &[Created, Running]),
            run(task(&p, "a"), Some(0), &[Created, Running, Success]),
            run(
                task(&p, "ko"),
                Some(0),
                &[Created, Running, Killing, Killed],
            ),
        ]);
        let all = rounds(&p, &mut s);
        no_claim_and_no_run(&all);
        assert_eq!(
            all,
            vec![
                vec![set_run(0, Killing)],
                vec![set_run(0, Killed)],
                vec![set_exec(Killed)],
            ]
        );
        assert_eq!(s.runs.len(), 3, "fin was never created");
        // The control: the same block under a RUNNING execution, over a
        // FAILED child, runs its `finally:` -- so "no fin" above is the kill's.
        let mut live = s.clone();
        live.history = vec![Created, Running];
        live.runs.truncate(3);
        live.runs[0].history = vec![Created, Running];
        live.runs[0].attempts[0].history = vec![Created, Running];
        live.runs[2].history = vec![Created, Running, RunState::Failed];
        let fin = task(&p, "fin");
        let created_fin = rounds(&p, &mut live)
            .iter()
            .flatten()
            .any(|c| matches!(c, Command::CreateRun { task, .. } if *task == fin));
        assert!(
            created_fin,
            "the control: a live AllowFailure runs its finally"
        );
    }

    /// **The route's leaf steps** (`kill_leaves`): a running leaf is
    /// `KILLING` whether or not it is held (final review I1), every other
    /// live leaf `KILLED` (`CREATED`, `RETRYING`, a `Subflow`), a flowable and
    /// a terminal or `KILLING` run untouched, and the execution `KILLING`.
    #[test]
    fn the_route_kills_each_live_leaf_by_whether_it_is_running() {
        let p = plan(
            "  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
             \x20     - id: held\n        type: io.kestra.plugin.core.log.Log\n        message: h\n\
             \x20     - id: waiting\n        type: io.kestra.plugin.core.log.Log\n        message: w\n\
             \x20     - id: created\n        type: io.kestra.plugin.core.log.Log\n        message: c\n\
             \x20     - id: retrying\n        type: io.kestra.plugin.core.log.Log\n        message: r\n\
             \x20     - id: done\n        type: io.kestra.plugin.core.log.Log\n        message: d\n\
             \x20     - id: dying\n        type: io.kestra.plugin.core.log.Log\n        message: k\n\
             \x20     - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
             \x20       flowId: child\n",
        );
        let mut s = ExecutionState {
            runs: vec![
                run(task(&p, "par"), None, &[Created, Running]),
                run(task(&p, "held"), Some(0), &[Created, Running]),
                run(task(&p, "waiting"), Some(0), &[Created, Running]),
                run(task(&p, "created"), Some(0), &[Created]),
                run(task(&p, "retrying"), Some(0), &[Created, Running, Retrying]),
                run(task(&p, "done"), Some(0), &[Created, Running, Success]),
                run(task(&p, "dying"), Some(0), &[Created, Running, Killing]),
                run(task(&p, "sub"), Some(0), &[Created, Running]),
            ],
            history: vec![Created, Running],
            metadata: ExecutionMeta::default(),
        };
        // `held` and `waiting` differ only in a lease the route does not read.
        let cmds = kill_leaves(&p, &s);
        assert_eq!(
            cmds,
            vec![
                set_run(1, Killing),
                set_run(2, Killing),
                set_run(3, Killed),
                set_run(4, Killed),
                set_run(7, Killed),
                set_exec(Killing),
            ]
        );
        for c in &cmds {
            apply(&mut s, c);
        }
        // Applied once, a second call adds nothing but the leaves it never
        // owned: the execution is already `KILLING`.
        assert_eq!(kill_leaves(&p, &s), Vec::<Command>::new());
        // And `advance` takes it from there: `par` waits on `held`.
        assert_eq!(advance(&p, &s, LATE), vec![set_run(0, Killing)]);
    }
}
