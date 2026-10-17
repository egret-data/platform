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
    if cmds.is_empty() && state.runs.iter().all(is_settled) {
        cmds.push(Command::SetExecutionState {
            state: RunState::Killed,
        });
    }
    cmds
}

/// **The kill route's leaf steps** (spec §3.2 steps 1, 2 and 4), as commands
/// for the route to apply to a loaded execution before its `settle`:
///
/// 1. a `Runnable` that is running (`RUNNING` or `SUBMITTED`) **and** that a
///    worker holds (`leased(run)`) becomes `KILLING`: its worker stops it and
///    writes `KILLED` (spec §4.2);
/// 2. every other live leaf -- a `Runnable` waiting for a worker, `CREATED`,
///    `RETRYING`, and every `Subflow` (`Executable`) run -- becomes `KILLED`;
/// 3. a flowable is left alone: `advance_killing` closes it (§5), and so is a
///    leaf that is already `KILLING`;
/// 4. the execution becomes `KILLING` unless it already is.
///
/// `leased` is the database's answer (`claimed_at IS NOT NULL`), which
/// `advance()` cannot see; it is why this is not [`advance_killing`]'s and why
/// `exit::kill`, which has no lease to read, calls every running `Runnable`
/// held. A run waiting for a worker is therefore `KILLED` here at once rather
/// than left `KILLING` for the claim to finish (spec §4.3): nothing is running
/// it, and nothing will once this commits.
pub(crate) fn kill_leaves(
    plan: &Plan,
    state: &ExecutionState,
    leased: impl Fn(RunIx) -> bool,
) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (ix, run) in state.runs.iter().enumerate() {
        let Some(&last) = run.history.last() else {
            continue;
        };
        let kind = &plan.tasks[run.task].kind;
        if last.is_terminal() || last == RunState::Killing || !is_leaf(kind) {
            continue;
        }
        let held = matches!(kind, TaskKind::Runnable { .. })
            && matches!(last, RunState::Running | RunState::Submitted)
            && leased(ix);
        cmds.push(Command::SetRunState {
            run: ix,
            state: if held {
                RunState::Killing
            } else {
                RunState::Killed
            },
        });
    }
    if state.history.last() != Some(&RunState::Killing) {
        cmds.push(Command::SetExecutionState {
            state: RunState::Killing,
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

    /// **The route's leaf steps** (`kill_leaves`): a held running leaf is
    /// `KILLING`, every other live leaf `KILLED` (waiting, `CREATED`,
    /// `RETRYING`, a `Subflow`), a flowable and a terminal or `KILLING` run
    /// untouched, and the execution `KILLING`.
    #[test]
    fn the_route_kills_each_live_leaf_by_whether_it_is_held() {
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
        // `held`, and also `dying` and `par`: a lease on a row that is not a
        // running leaf changes nothing.
        let leased = |ix: RunIx| matches!(ix, 0 | 1 | 6);
        let cmds = kill_leaves(&p, &s, leased);
        assert_eq!(
            cmds,
            vec![
                set_run(1, Killing),
                set_run(2, Killed),
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
        assert_eq!(kill_leaves(&p, &s, leased), Vec::<Command>::new());
        // And `advance` takes it from there: `par` waits on `held`.
        assert_eq!(advance(&p, &s, LATE), vec![set_run(0, Killing)]);
    }
}
