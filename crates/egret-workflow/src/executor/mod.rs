//! `advance()` — the pure state machine that decides what an execution does
//! next.
//!
//! # No I/O, no clock
//!
//! Nothing in `executor/` may reach a database, the network or the wall clock
//! (spec §6.5, and the plan's Global Constraints). `now` arrives as a
//! parameter, spelled [`Tick`] rather than `std::time::Instant` because an
//! `Instant` cannot be constructed at an arbitrary value and every retry and
//! timeout test would then need a real clock — in the one module whose whole
//! point is that it has none.
//!
//! # What is implemented
//!
//! W3a Task 3: [`Plan`], [`ExecutionState`], [`Command`], [`build_plan`], and
//! the flowable `sequential` — which is also the flow's top-level `tasks:`,
//! since Kestra runs that list by the same rule. W3a Task 4: `parallel` and
//! `dag`, and the two refusals a `Dag` can earn at plan-build time. Every other
//! task type is planned as an opaque [`TaskKind::Runnable`]; see [`build_plan`]
//! for why that is the safe default rather than an approximation.
//!
//! Tasks 5-8 add `if`/`switch`, `loop` and the attempt lifecycle. They add
//! variants to [`TaskKind`] and [`Command`]; they do not change the signatures
//! here.

mod command;
mod flowable;
mod plan;
mod state;

pub use command::Command;
pub use plan::{
    build_plan, DagNode, Plan, PlanError, PlanTask, RetryBehavior, RetryPolicy, TaskIx, TaskKind,
};
pub use state::{apply, Attempt, ExecutionState, RunIx, RunState, TaskRun, Tick};

use flowable::{sequential, Step};

/// Decide what this execution does next.
///
/// Pure: it reads `plan`, `state` and `now`, and returns commands for the
/// caller to apply. It never mutates anything and never reads a clock.
///
/// # The shape of the answer
///
/// The flow's top-level `tasks:` is a `Sequential` with no [`PlanTask`] of its
/// own, so the whole of Task 3 is: drive that list, and translate what it wants
/// into execution-level commands.
///
/// The execution is moved to `RUNNING` on the first round in which it has a
/// run, or on the round it terminates if it never gets one. That is later than
/// the round in which the first run is *created*, and the difference is
/// unobservable: the compared trace holds the execution's history as a
/// sequence, and round numbers are compared only between runs. Emitting it a
/// round earlier would make the plan's own first-round assertion
/// (`a_sequential_runs_its_children_one_at_a_time`) untrue for no gain.
pub fn advance(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    if state.history.last().is_some_and(|s| s.is_terminal()) {
        return Vec::new();
    }

    let work = match sequential::drive(plan, state, &plan.roots, None, now) {
        Step::Commands(cmds) => cmds,
        Step::Waiting => Vec::new(),
        Step::Done(state) => vec![Command::SetExecutionState { state }],
    };

    let not_running_yet = state.history.last() == Some(&RunState::Created);
    let has_a_run = !state.runs.is_empty();
    let terminating = matches!(
        work.first(),
        Some(Command::SetExecutionState { state }) if state.is_terminal()
    );
    if not_running_yet && (has_a_run || terminating) {
        let mut cmds = vec![Command::SetExecutionState {
            state: RunState::Running,
        }];
        cmds.extend(work);
        return cmds;
    }
    work
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tick every test that does not care about time uses.
    const T0: Tick = Tick(0);

    /// Builds a [`Plan`] from a corpus flow by id, via W1's YAML parser.
    ///
    /// Defined ONCE, here. Tasks 4-8 use it and must not fork a second copy.
    /// Both extensions, because `oracle-exec/corpus/valids/` holds 261 `.yaml`
    /// and 56 `.yml` and the id carries neither.
    fn plan_of(corpus_id: &str) -> Plan {
        build_plan(&corpus_source(corpus_id))
            .unwrap_or_else(|e| panic!("build_plan refused {corpus_id}: {e}"))
    }

    fn corpus_source(corpus_id: &str) -> String {
        for ext in ["yaml", "yml"] {
            let path = format!(
                "{}/oracle-exec/corpus/{corpus_id}.{ext}",
                env!("CARGO_MANIFEST_DIR")
            );
            if let Ok(s) = std::fs::read_to_string(&path) {
                return s;
            }
        }
        panic!("no corpus file for {corpus_id} with either extension");
    }

    /// Every corpus flow, as `(id, source)`. Derived from the directory, never
    /// enumerated: a hand-written list is how a table comes to print a count
    /// for a class that no longer exists.
    fn every_corpus_flow() -> Vec<(String, String)> {
        let root = format!("{}/oracle-exec/corpus", env!("CARGO_MANIFEST_DIR"));
        let mut out = Vec::new();
        for dir in ["valids", "invalids"] {
            let entries = std::fs::read_dir(format!("{root}/{dir}"))
                .unwrap_or_else(|e| panic!("cannot read {root}/{dir}: {e}"));
            for entry in entries {
                let path = entry.expect("corpus dir entry").path();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "yaml" && ext != "yml" {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("file stem");
                out.push((
                    format!("{dir}/{stem}"),
                    std::fs::read_to_string(&path).expect("corpus file"),
                ));
            }
        }
        out.sort();
        out
    }

    /// Drive `advance` to a fixed point, completing every `Runnable` that goes
    /// `Running` with `outcome`. A unit-test stand-in for the oracle's
    /// environment: no golden, one outcome for every task.
    fn run_to_fixpoint(plan: &Plan, outcome: RunState) -> ExecutionState {
        let mut state = ExecutionState::new(plan);
        for _ in 0..1000 {
            let cmds = advance(plan, &state, T0);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    apply(&mut state, cmd);
                }
                continue;
            }
            let in_flight = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && state.runs[ix].history.last() == Some(&RunState::Running)
            });
            match in_flight {
                Some(ix) => apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                ),
                None => return state,
            }
        }
        panic!("did not settle in 1000 rounds");
    }

    /// [`run_to_fixpoint`], but the environment completes the **latest**-created
    /// in-flight run first instead of the earliest.
    ///
    /// This is the 4s/2s/1s experiment, in a state machine: completion order is
    /// the exact reverse of creation order. `taskRunList` is creation order —
    /// measured on a flow whose three parallel branches slept 4s, 2s and 1s,
    /// five rounds, one distinct order, `a_slow, b_mid, c_fast` — so the run
    /// list this produces must be unchanged by the reversal.
    fn run_to_fixpoint_finishing_backwards(plan: &Plan, outcome: RunState) -> ExecutionState {
        let mut state = ExecutionState::new(plan);
        for _ in 0..1000 {
            let cmds = advance(plan, &state, T0);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    apply(&mut state, cmd);
                }
                continue;
            }
            let in_flight = (0..state.runs.len()).rev().find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && state.runs[ix].history.last() == Some(&RunState::Running)
            });
            match in_flight {
                Some(ix) => apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                ),
                None => return state,
            }
        }
        panic!("did not settle in 1000 rounds");
    }

    /// Apply one round of `advance` and hand back what it asked for.
    fn round(plan: &Plan, state: &mut ExecutionState) -> Vec<Command> {
        let cmds = advance(plan, state, T0);
        for cmd in &cmds {
            apply(state, cmd);
        }
        cmds
    }

    /// The ids of the runs, in creation order.
    fn run_ids(plan: &Plan, state: &ExecutionState) -> Vec<String> {
        state
            .runs
            .iter()
            .map(|r| plan.tasks[r.task].id.clone())
            .collect()
    }

    fn wire(states: &[RunState]) -> Vec<&'static str> {
        states.iter().map(|s| s.wire()).collect()
    }

    // ----------------------------------------------------------------------
    // the flowable
    // ----------------------------------------------------------------------

    /// The plan's own Task 3 test, verbatim.
    #[test]
    fn a_sequential_runs_its_children_one_at_a_time() {
        let plan = plan_of("valids/sequential");
        let state = ExecutionState::new(&plan);
        let cmds = advance(&plan, &state, T0);
        assert_eq!(
            cmds,
            vec![Command::CreateRun {
                task: 0,
                parent: None,
                value: None
            }]
        );
    }

    /// **The next child is not created until the previous one has terminated.**
    ///
    /// The rule the whole flowable is, stated over commands rather than over a
    /// finished trace: with the first root run created and not yet terminal,
    /// `advance` must never ask for the second. A `drive` that created the
    /// whole list up front passes every trace-shaped assertion about *which*
    /// runs exist and fails this one — and on the real corpus it turns every
    /// `before` pair into `overlapping`.
    #[test]
    fn the_next_child_is_not_created_until_the_previous_one_terminated() {
        let plan = plan_of("valids/logs");
        assert!(
            plan.roots.len() >= 3,
            "this test needs a flow with several root tasks; valids/logs has {}",
            plan.roots.len()
        );

        let mut state = ExecutionState::new(&plan);
        // Round 1: create the first root.
        for cmd in &advance(&plan, &state, T0) {
            apply(&mut state, cmd);
        }
        assert_eq!(state.runs.len(), 1);

        // Round 2: start it. Still no second run.
        for cmd in &advance(&plan, &state, T0) {
            apply(&mut state, cmd);
        }
        assert_eq!(
            state.runs.len(),
            1,
            "the second child must not be created while the first is running"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "SUBMITTED", "RUNNING"]
        );

        // Round 3: it is in flight, so there is nothing to decide.
        assert_eq!(
            advance(&plan, &state, T0),
            vec![],
            "with the only child in flight the executor has nothing to do"
        );

        // The worker lands it; now, and not before, the second is created.
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Success,
            },
        );
        assert_eq!(
            advance(&plan, &state, T0),
            vec![Command::CreateRun {
                task: plan.roots[1],
                parent: None,
                value: None
            }]
        );
    }

    /// **A `Runnable` is `SUBMITTED` before it is `RUNNING`; a flowable is not.**
    ///
    /// Measured on `valids/sequential`, which nests both: the `Sequential` runs
    /// go `CREATED RUNNING SUCCESS` and every `Return` goes `CREATED SUBMITTED
    /// RUNNING SUCCESS`. Deleting the `SUBMITTED` emission leaves a trace that
    /// still has the right runs in the right order and matches nothing.
    #[test]
    fn only_a_runnable_passes_through_submitted() {
        // One flow per flowable this executor drives, so the discriminator is
        // pinned on all three and not only on the one Task 3 shipped.
        let mut flowables = 0;
        let mut runnables = 0;
        for corpus_id in ["valids/sequential", "valids/parallel", "valids/dag"] {
            let plan = plan_of(corpus_id);
            let state = run_to_fixpoint(&plan, RunState::Success);
            for run in &state.runs {
                match plan.tasks[run.task].kind {
                    TaskKind::Sequential { .. }
                    | TaskKind::Parallel { .. }
                    | TaskKind::Dag { .. } => {
                        flowables += 1;
                        assert_eq!(
                            wire(&run.history),
                            ["CREATED", "RUNNING", "SUCCESS"],
                            "{corpus_id}: flowable {} must not pass through SUBMITTED",
                            plan.tasks[run.task].id
                        );
                    }
                    TaskKind::Runnable { .. } => {
                        runnables += 1;
                        assert_eq!(
                            wire(&run.history),
                            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
                            "{corpus_id}: runnable {} must pass through SUBMITTED",
                            plan.tasks[run.task].id
                        );
                    }
                }
                assert_eq!(
                    run.attempts.len(),
                    1,
                    "one attempt each, flowable or not -- Kestra records one for both"
                );
                assert_eq!(
                    wire(&run.attempts[0].history),
                    ["CREATED", "RUNNING", "SUCCESS"],
                    "an attempt never carries SUBMITTED, whatever the run does"
                );
            }
        }
        assert!(
            flowables >= 5 && runnables >= 19,
            "the three flows must exercise both sides; saw {flowables} flowable(s) \
             and {runnables} runnable(s)"
        );
    }

    /// **A child that does not succeed stops the sequence and is what the list
    /// reports.**
    ///
    /// Both halves matter. The remaining children are never created — which is
    /// why a flow whose second of five tasks fails has two runs and not five —
    /// and the execution takes the failure rather than reporting `SUCCESS`
    /// because every run it *did* create is terminal.
    #[test]
    fn a_failing_child_stops_the_sequence_and_the_execution_takes_its_state() {
        let plan = plan_of("valids/logs");
        let n = plan.roots.len();
        let state = run_to_fixpoint(&plan, RunState::Failed);

        assert_eq!(
            state.runs.len(),
            1,
            "the first task failed, so none of the other {} was created",
            n - 1
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "FAILED"]);

        // And the same flow with a succeeding worker runs every task, so the
        // assertion above is about the failure and not about the flow.
        let ok = run_to_fixpoint(&plan, RunState::Success);
        assert_eq!(ok.runs.len(), n);
        assert_eq!(wire(&ok.history), ["CREATED", "RUNNING", "SUCCESS"]);
    }

    /// **A nested `Sequential` terminates only when its own children have, and
    /// its state is theirs.**
    #[test]
    fn a_nested_sequential_takes_its_childrens_outcome() {
        let plan = plan_of("valids/sequential");
        let state = run_to_fixpoint(&plan, RunState::Failed);

        let root_seq = &state.runs[0];
        assert!(
            matches!(plan.tasks[root_seq.task].kind, TaskKind::Sequential { .. }),
            "valids/sequential's first root task is a Sequential"
        );
        assert_eq!(wire(&root_seq.history), ["CREATED", "RUNNING", "FAILED"]);
        assert_eq!(
            state.runs.len(),
            2,
            "the Sequential's first child failed, so nothing after it was created"
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "FAILED"]);
    }

    /// **The execution is never asked for anything once it is terminal.**
    #[test]
    fn a_terminal_execution_is_finished() {
        let plan = plan_of("valids/minimal");
        let state = run_to_fixpoint(&plan, RunState::Success);
        assert_eq!(advance(&plan, &state, T0), vec![]);
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "SUCCESS"]);
    }

    // ----------------------------------------------------------------------
    // parallel
    // ----------------------------------------------------------------------

    /// **Every child is created before any of them is started.**
    ///
    /// The plan's own Task 4 test, and it is the shape Kestra's
    /// `resolveParallelNexts` has: it returns *every* not-yet-created child in
    /// one call, and it returns nothing at all while any child run is still
    /// `CREATED` ("first created, leave").
    ///
    /// Why it matters is measured, not stylistic. Kestra's `taskRunList` is
    /// **creation** order — proven on a flow whose three parallel branches
    /// sleep 4s, 2s and 1s, where completion order is the exact reverse and
    /// five rounds still gave one order, `a_slow, b_mid, c_fast`. A `drive`
    /// that created children lazily, as earlier ones finished, would order that
    /// list by completion and invert it.
    ///
    /// So this asserts both halves: the batch itself, and — through an
    /// environment that finishes the **last**-created run first, which is the
    /// 4s/2s/1s experiment in a clock-free machine — that the resulting list is
    /// unmoved by completion order.
    #[test]
    fn parallel_creates_every_child_before_starting_any() {
        let plan = plan_of("valids/parallel");
        let TaskKind::Parallel { tasks, .. } = &plan.tasks[0].kind else {
            panic!("valids/parallel's first task is a Parallel");
        };
        let children = tasks.clone();
        assert_eq!(children.len(), 6, "valids/parallel has six branches");

        let mut state = ExecutionState::new(&plan);
        // Round 1 creates the Parallel itself; round 2 starts it.
        round(&plan, &mut state);
        round(&plan, &mut state);
        assert_eq!(
            state.runs[0].history.last(),
            Some(&RunState::Running),
            "the Parallel has to be RUNNING before it can create anything"
        );

        // Round 3 is the whole batch, and nothing but the batch.
        let batch = round(&plan, &mut state);
        assert_eq!(
            batch,
            children
                .iter()
                .map(|&task| Command::CreateRun {
                    task,
                    parent: Some(0),
                    value: None
                })
                .collect::<Vec<_>>(),
            "a Parallel creates every child in ONE round, in declaration order, and \
             starts none of them while it does -- a lazy `drive` asks for one \
             CreateRun here and orders taskRunList by completion instead"
        );

        // And the list is what it is whatever order the branches finish in.
        let forwards = run_to_fixpoint(&plan, RunState::Success);
        let backwards = run_to_fixpoint_finishing_backwards(&plan, RunState::Success);
        assert_eq!(
            run_ids(&plan, &forwards),
            ["parent", "t1", "t2", "t3", "t4", "t5", "t6", "last"],
            "creation order, as written"
        );
        assert_eq!(
            run_ids(&plan, &backwards),
            run_ids(&plan, &forwards),
            "the 4s/2s/1s experiment: completion order reversed, taskRunList unmoved"
        );
    }

    /// **A `Parallel` drives every branch in the same round, not the first one
    /// to want something.**
    ///
    /// Measured on `valids/parallel-disabled-tasks`, whose `taskRunList` is
    /// `parallel, sequence1, sequence2, task1, task3, task2, task4`: the two
    /// `Sequential` branches have their first children created adjacently, then
    /// their second children adjacently. A driver that returned the first
    /// branch's work and stopped — which is exactly what `sequential::drive`
    /// does, on purpose — would run `sequence1` to completion before creating
    /// anything under `sequence2`, giving `task1, task2, task3, task4`.
    ///
    /// The flow itself is still baselined (it uses `disabled:`, which no W3a
    /// task claims), so this asserts the property on the same *shape* built
    /// without that key.
    ///
    /// **The mutation that kills this is M26**, which makes rule 3 stop at the
    /// first child that has not terminated — i.e. serialises the branches. It is
    /// **not** M22 (rule 3 breaking after the first child that wants something),
    /// which this test survives: M22 still visits the branches in order on every
    /// round, so branch two catches up as soon as branch one is waiting and the
    /// run list is unchanged. An earlier version of this doc and of the task
    /// report both named M22, and both were wrong — a mutation that a test
    /// survives is not that test's guard.
    #[test]
    fn a_parallel_drives_every_branch_in_the_same_round() {
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: parallel\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: sequence1\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: task1\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n          - id: task2\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n      - id: sequence2\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: task3\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n          - id: task4\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n",
        )
        .expect("plans");
        let state = run_to_fixpoint(&plan, RunState::Success);
        assert_eq!(
            run_ids(&plan, &state),
            [
                "parallel",
                "sequence1",
                "sequence2",
                "task1",
                "task3",
                "task2",
                "task4"
            ],
            "both branches advance in the same round -- this is the taskRunList \
             `valids/parallel-disabled-tasks` produced, minus its disabled task"
        );
    }

    /// **One branch that ends badly fails every sibling still in flight — and
    /// only those.**
    ///
    /// The design doc's sentence is "one bad sibling failed all three", from a
    /// spike whose three parallel branches slept 4s, 2s and 1s and whose
    /// fastest had a validation error: the other two ended `FAILED`. The golden
    /// reproduces the shape — in `valids/parallel-fail-with-flowable` the
    /// branch holding a `Sleep: PT1S` ends `FAILED`, a `Sleep` that succeeds
    /// everywhere else in the corpus including at `PT30S`.
    ///
    /// **The sentence is too wide, and the same golden is what says so.** In
    /// `valids/finally-parallel`, driven with `failed: true`, the `Parallel`
    /// ends `FAILED` while three of its four branches end `SUCCESS`. The
    /// difference is *when*: those three had already terminated. So the rule is
    /// about the runs still going, and both halves are asserted here — the
    /// spike's, and the bound `finally-parallel` puts on it.
    #[test]
    fn one_invalid_sibling_fails_every_sibling() {
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: a_slow\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: b_mid\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: c_fast\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("plans");

        // Create the Parallel, start it, create the batch, start the batch.
        let mut state = ExecutionState::new(&plan);
        for _ in 0..4 {
            round(&plan, &mut state);
        }
        let ids = run_ids(&plan, &state);
        assert_eq!(ids, ["par", "a_slow", "b_mid", "c_fast"]);
        assert!(
            (1..=3).all(|ix| state.runs[ix].history.last() == Some(&RunState::Running)),
            "all three branches must be in flight before one of them fails: {:?}",
            state
                .runs
                .iter()
                .map(|r| r.history.last())
                .collect::<Vec<_>>()
        );

        // c_fast alone fails.
        apply(
            &mut state,
            &Command::SetRunState {
                run: 3,
                state: RunState::Failed,
            },
        );
        let cmds = round(&plan, &mut state);
        assert_eq!(
            cmds,
            vec![
                Command::SetRunState {
                    run: 1,
                    state: RunState::Failed
                },
                Command::SetRunState {
                    run: 2,
                    state: RunState::Failed
                },
            ],
            "a_slow and b_mid were still in flight when c_fast failed, so both end \
             FAILED -- the spike measured exactly this"
        );
        round(&plan, &mut state);
        assert_eq!(
            state.runs[0].history.last(),
            Some(&RunState::Failed),
            "and the Parallel takes the failure"
        );

        // The bound: a sibling that had ALREADY terminated keeps what it ended
        // with. `valids/finally-parallel` ends FAILED with three SUCCESSes.
        let mut state = ExecutionState::new(&plan);
        for _ in 0..4 {
            round(&plan, &mut state);
        }
        apply(
            &mut state,
            &Command::SetRunState {
                run: 1,
                state: RunState::Success,
            },
        );
        apply(
            &mut state,
            &Command::SetRunState {
                run: 3,
                state: RunState::Failed,
            },
        );
        while !advance(&plan, &state, T0).is_empty() {
            round(&plan, &mut state);
        }
        assert_eq!(
            state
                .runs
                .iter()
                .map(|r| r.history.last().copied().unwrap())
                .collect::<Vec<_>>(),
            vec![
                RunState::Failed,
                RunState::Success,
                RunState::Failed,
                RunState::Failed
            ],
            "a_slow had already SUCCEEDED and keeps it; only b_mid, which was still \
             in flight, is failed"
        );
    }

    /// **A `Parallel`'s own state is `KILLED` over `FAILED` over `SUCCESS`.**
    ///
    /// Kestra's `Execution.guessFinalState` priority, narrowed to what
    /// [`RunState`] can hold.
    #[test]
    fn a_parallel_takes_the_worst_of_its_children() {
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("plans");
        for (outcome, expected) in [
            (RunState::Success, RunState::Success),
            (RunState::Failed, RunState::Failed),
            (RunState::Killed, RunState::Killed),
        ] {
            let state = run_to_fixpoint(&plan, outcome);
            assert_eq!(
                state.runs[0].history.last(),
                Some(&expected),
                "children {outcome:?} must give the Parallel {expected:?}"
            );
        }
    }

    /// **`concurrent:` caps how many children a `Parallel` creates at once.**
    ///
    /// Kestra's cap is on the *stream* of next task runs
    /// (`limit(concurrency - runningCount)` in `resolveParallelNexts`), so it
    /// bounds creation and not merely submission; the second batch arrives when
    /// the first has drained. Nothing in the 345-flow corpus exercises this
    /// (`valids/if` and `valids/full` are the only two flows with a
    /// `concurrent:` at all, and both are baselined for other flowables), so
    /// this test is the only thing holding it.
    #[test]
    fn a_parallel_concurrency_cap_bounds_the_batch() {
        let flow = |concurrent: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n{concurrent}    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: c\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            )
        };

        let capped = build_plan(&flow("    concurrent: 2\n")).expect("plans");
        assert_eq!(
            capped.tasks[0].kind,
            TaskKind::Parallel {
                tasks: vec![1, 2, 3],
                concurrency: Some(2)
            }
        );
        let mut state = ExecutionState::new(&capped);
        round(&capped, &mut state);
        round(&capped, &mut state);
        assert_eq!(
            round(&capped, &mut state).len(),
            2,
            "`concurrent: 2` creates two of the three children in the first batch"
        );

        // The control, same YAML one line shorter: Kestra's default is 0, and 0
        // is Kestra's own spelling of "no limit".
        let uncapped = build_plan(&flow("")).expect("plans");
        assert_eq!(
            uncapped.tasks[0].kind,
            TaskKind::Parallel {
                tasks: vec![1, 2, 3],
                concurrency: None
            },
            "an absent `concurrent:` is Kestra's default 0, which is unlimited"
        );
        let mut state = ExecutionState::new(&uncapped);
        round(&uncapped, &mut state);
        round(&uncapped, &mut state);
        assert_eq!(
            round(&uncapped, &mut state).len(),
            3,
            "the control: uncapped, the batch is the whole list"
        );

        // Both flows still finish, capped or not.
        for plan in [&capped, &uncapped] {
            let state = run_to_fixpoint(plan, RunState::Success);
            assert_eq!(run_ids(plan, &state), ["par", "a", "b", "c"]);
        }
    }

    /// **Every `concurrent:` in the corpus is a literal integer**, which is what
    /// makes [`TaskKind::Parallel`]'s "an expression reads as unlimited"
    /// fallback unreachable from anything this oracle compares.
    ///
    /// Derived from the corpus on every run rather than remembered: a flow
    /// added later with `concurrent: "{{ inputs.n }}"` would make the fallback
    /// live, and the fallback is a silent one.
    #[test]
    fn every_corpus_concurrent_is_a_literal_integer() {
        let mut seen = Vec::new();
        for (id, source) in every_corpus_flow() {
            for line in source.lines() {
                if let Some(value) = line.trim().strip_prefix("concurrent:") {
                    let value = value.trim();
                    assert!(
                        value.parse::<u32>().is_ok(),
                        "{id} writes `concurrent: {value}`, which build_plan cannot \
                         read without a RenderContext -- it will silently plan as \
                         UNLIMITED. See TaskKind::Parallel."
                    );
                    seen.push(format!("{id}={value}"));
                }
            }
        }
        assert_eq!(
            seen,
            ["valids/full=3", "valids/if=4"],
            "the corpus's whole population of `concurrent:` values"
        );
    }

    // ----------------------------------------------------------------------
    // dag
    // ----------------------------------------------------------------------

    /// **A node is created only once every task it `dependsOn` has SUCCEEDED,
    /// and the whole eligible batch is created at once.**
    ///
    /// The assertion is `valids/dag`'s own `taskRunList`, verbatim from
    /// `golden.json`: `dag, task1, task2, task3, task4, task6, task5`. That
    /// order is not declaration order — `task5` is declared before `task6` and
    /// created after it — so it can only come from the dependency graph, which
    /// is what makes it the test for the predicate.
    ///
    /// This is the plan's Step-4 mutation target: making `dependsOn` vacuously
    /// satisfied turns the `Dag` into a `Parallel` and creates all six at once.
    #[test]
    fn a_dag_creates_a_node_only_when_every_dependency_has_succeeded() {
        let plan = plan_of("valids/dag");
        let TaskKind::Dag { nodes } = &plan.tasks[0].kind else {
            panic!("valids/dag's first task is a Dag");
        };
        assert_eq!(
            nodes
                .iter()
                .map(|n| (
                    plan.tasks[n.task].id.as_str(),
                    n.depends_on
                        .iter()
                        .map(|&d| plan.tasks[d].id.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            [
                ("task1", vec![]),
                ("task2", vec!["task1"]),
                ("task3", vec!["task1"]),
                ("task4", vec!["task2"]),
                ("task5", vec!["task4", "task3"]),
                ("task6", vec!["task2", "task3"]),
            ],
            "`dependsOn` is resolved to plan indices at build time"
        );

        let state = run_to_fixpoint(&plan, RunState::Success);
        assert_eq!(
            run_ids(&plan, &state),
            ["dag", "task1", "task2", "task3", "task4", "task6", "task5"],
            "Kestra's taskRunList for valids/dag, verbatim -- task5 is DECLARED \
             before task6 and created after it, which only the dependency \
             predicate can produce"
        );
        assert_eq!(state.runs[0].history.last(), Some(&RunState::Success));
    }

    /// **Nothing downstream of a failed dependency is ever created, and the
    /// `Dag` ends FAILED rather than stuck.**
    ///
    /// This is also what makes the predicate's spelling unobservable. Kestra's
    /// `resolveDagNexts` filters on "all dependencies are **terminated**";
    /// [`dag::drive`](crate::executor::flowable::dag::drive) says *succeeded*.
    /// Read alone the two differ on a failed dependency — and no input reaches
    /// the predicate in that state, because the branch has already closed. If a
    /// later task gives a branch a way to survive a failed child (`errors:` is
    /// Task 7's and does exactly that), this is the test that notices.
    #[test]
    fn a_dag_creates_nothing_downstream_of_a_failed_dependency() {
        let plan = plan_of("valids/dag");
        let state = run_to_fixpoint(&plan, RunState::Failed);
        assert_eq!(
            run_ids(&plan, &state),
            ["dag", "task1"],
            "task1 failed, so none of the five nodes downstream of it exists"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED"],
            "and the Dag reports the failure instead of waiting for runs that will \
             never be created"
        );
    }

    /// **A cyclic `Dag` is refused at plan-build time, by name.**
    ///
    /// `invalids/dag-cyclicdependency` is a golden `refused` entry that egret
    /// **over-accepted** before this — over-acceptance being the one divergence
    /// class the programme forbids shipping. Kestra refuses it in
    /// `DagTaskValidator`, at publish time, which is why it has no execution to
    /// compare in the first place.
    ///
    /// The control is the same file with the one edge that closes the cycle
    /// removed: it plans. Without that, this test would pass against a builder
    /// that refused every `Dag`.
    #[test]
    fn a_cyclic_dag_is_refused_by_name() {
        let source = corpus_source("invalids/dag-cyclicdependency");
        // Not `expect_err`: that prints the whole `Plan` and buries the finding.
        // A guard that fires with an unreadable message has been seen fire and
        // not seen work -- Task 3 paid for that once already, on M6.
        let err = match build_plan(&source) {
            Err(e) => e,
            Ok(plan) => panic!(
                "a cyclic dag must be refused; got a plan of {} task(s) rooted at '{}'",
                plan.tasks.len(),
                plan.tasks[0].id
            ),
        };
        assert!(
            err.message.contains("cyclic dependency") && err.message.contains("task1"),
            "the refusal has to name what is cyclic; got {err}"
        );

        // task1 dependsOn task2 and task2 dependsOn task1. Cut the first edge.
        let acyclic = source.replacen("        dependsOn:\n          - task2\n", "", 1);
        assert!(
            build_plan(&acyclic).is_ok(),
            "the control: with the back-edge gone the same flow plans -- otherwise \
             this test would pass against a builder that refused every Dag"
        );
    }

    /// **A `dependsOn` naming a task that is not one of the `Dag`'s own nodes is
    /// refused, by name.**
    ///
    /// Kestra's other `DagTaskValidator` check, and `invalids/dag-notexist-task`
    /// is the golden entry for it — also over-accepted before this. Kestra's
    /// message is "Not existing task id in dependency: taskX".
    ///
    /// The control is the same file with `taskX` corrected to a real node.
    #[test]
    fn a_dag_dependency_on_a_task_that_is_not_a_node_is_refused_by_name() {
        let source = corpus_source("invalids/dag-notexist-task");
        let err = match build_plan(&source) {
            Err(e) => e,
            Ok(plan) => panic!(
                "a dependency on a non-node must be refused; got a plan of {} task(s) \
                 rooted at '{}'",
                plan.tasks.len(),
                plan.tasks[0].id
            ),
        };
        assert!(
            err.message.contains("not nodes of it") && err.message.contains("taskX"),
            "the refusal has to name the id that does not exist; got {err}"
        );
        assert!(
            build_plan(&source.replace("- taskX", "- task1")).is_ok(),
            "the control: point the same dependency at a real node and it plans"
        );
    }

    /// **A `Dag` node with no `task:` is refused.**
    ///
    /// W1's model types it `Option<Task>` because `@NotNull` is bean validation
    /// and not parsing, so the parser hands this builder a node with nothing to
    /// run and no id for a sibling to depend on. There is no such flow in the
    /// corpus, so this refusal banks nothing; it exists because the alternative
    /// is an `unwrap`.
    #[test]
    fn a_dag_node_with_no_task_is_refused() {
        let err = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: d\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task: null\n",
        )
        .expect_err("a node with no task cannot be planned");
        assert!(err.message.contains("node with no task"), "got {err}");
    }

    // ----------------------------------------------------------------------
    // the instant
    // ----------------------------------------------------------------------

    /// **A run's attempt records the `Tick` `advance` held when it started it,
    /// and that instant is recoverable from `ExecutionState` alone.**
    ///
    /// This is the guard for the field that closes cause 2 of the oracle's
    /// elapsed-time hole. Two ticks, not one, so that a `Tick(0)` hard-coded in
    /// either `advance` or `apply` fails rather than passing by coincidence —
    /// and the run whose attempt is checked is the *second*, so reading the
    /// wrong attempt fails too.
    #[test]
    fn an_attempt_records_the_tick_it_was_started_at() {
        let plan = plan_of("valids/logs");
        let mut state = ExecutionState::new(&plan);

        // Create and start the first root at tick 11.
        for _ in 0..2 {
            for cmd in &advance(&plan, &state, Tick(11)) {
                apply(&mut state, cmd);
            }
        }
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Success,
            },
        );
        // Create and start the second at tick 4200.
        for _ in 0..2 {
            for cmd in &advance(&plan, &state, Tick(4200)) {
                apply(&mut state, cmd);
            }
        }

        assert_eq!(state.runs.len(), 2, "two roots started");
        assert_eq!(state.runs[0].started_at(), Some(Tick(11)));
        assert_eq!(state.runs[1].started_at(), Some(Tick(4200)));
        assert_eq!(state.runs[1].attempt_started_at(), Some(Tick(4200)));
        assert_eq!(
            state.runs[1]
                .attempt_started_at()
                .map(|t| Tick(4200).0 - t.0),
            Some(0),
            "elapsed-since-this-attempt-started is what a maxDuration is measured \
             against, and it must be computable from the state alone"
        );

        // A run that has not started has no instant, which is not an error.
        let fresh = ExecutionState::new(&plan);
        assert!(fresh.runs.is_empty());
    }

    /// **The two accessors are different accessors**, and this is the only test
    /// that can tell them apart.
    ///
    /// [`TaskRun::started_at`] is the **first** attempt's instant, because that
    /// is what Kestra measures a `RETRY_FAILED_TASK` retry's `maxDuration` from
    /// (`this.attempts.getFirst().getState().minDate()`,
    /// `core/src/main/java/io/kestra/core/models/executions/TaskRun.java:337`).
    /// [`TaskRun::attempt_started_at`] is the **last**, because that is what a
    /// `timeout` on a `Pause` or a `WaitFor` runs against.
    ///
    /// Without this test they are interchangeable: swapping `.first()` for
    /// `.last()` in *either* one compiles and leaves the whole crate green with
    /// the ratchet unmoved, because **Task 3 never produces a run with more than
    /// one attempt** —
    /// `an_attempt_records_the_tick_it_was_started_at` pins the field but has
    /// only one attempt per run to read it from. Two accessors justified by two
    /// different Kestra lines and distinguishable by nothing is the
    /// guard-that-cannot-fail this package has now paid for three times.
    ///
    /// The second attempt is opened by [`apply`] from a real
    /// [`Command::StartRun`], not by a struct literal, so this pins the writer
    /// too. Only the *decision* to emit that second command is the test's, and
    /// that decision is exactly what Task 7 adds.
    #[test]
    fn the_run_start_and_the_attempt_start_are_different_instants() {
        let plan = plan_of("valids/minimal");
        let mut state = ExecutionState::new(&plan);

        // Attempt 1, at tick 11: created, submitted, started, failed.
        for _ in 0..2 {
            for cmd in &advance(&plan, &state, Tick(11)) {
                apply(&mut state, cmd);
            }
        }
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
        );
        assert_eq!(state.runs[0].attempts.len(), 1);

        // Attempt 2, at tick 4200 -- the command Task 7's retry will emit.
        apply(
            &mut state,
            &Command::StartRun {
                run: 0,
                at: Tick(4200),
            },
        );

        let run = &state.runs[0];
        assert_eq!(
            run.attempts.len(),
            2,
            "this test is worthless with one attempt; that is the whole point of it"
        );
        assert_eq!(
            run.started_at(),
            Some(Tick(11)),
            "started_at() is the FIRST attempt's instant -- Kestra measures a \
             RETRY_FAILED_TASK maxDuration from attempts.getFirst()"
        );
        assert_eq!(
            run.attempt_started_at(),
            Some(Tick(4200)),
            "attempt_started_at() is the LAST attempt's instant -- a Pause or \
             WaitFor timeout runs against the attempt in flight"
        );
        assert_ne!(
            run.started_at(),
            run.attempt_started_at(),
            "if these two ever agree in this test, it has stopped distinguishing \
             the accessors and swapping .first() for .last() passes again"
        );
    }

    // ----------------------------------------------------------------------
    // build_plan
    // ----------------------------------------------------------------------

    /// **`build_plan` never panics and never refuses a corpus flow it can
    /// parse**, over all 345 of them.
    ///
    /// A panic is not covered by the oracle's baseline, so a flow that crashes
    /// the builder fails the ratchet unconditionally; this finds that here,
    /// where the message is readable, rather than there.
    ///
    /// The counts are derived and printed, never asserted against a
    /// hand-written expectation — a summary that lists what its author expected
    /// is not a measurement.
    #[test]
    fn build_plan_survives_every_corpus_flow() {
        let flows = every_corpus_flow();
        assert_eq!(
            flows.len(),
            345,
            "the corpus is 345 flows (317 valids + 28 invalids); if that has moved, \
             oracle-exec/README.md is stale"
        );

        let mut refused: Vec<(String, String)> = Vec::new();
        let mut with_sequential = 0usize;
        for (id, source) in &flows {
            match build_plan(source) {
                Ok(plan) => {
                    if plan
                        .tasks
                        .iter()
                        .any(|t| matches!(t.kind, TaskKind::Sequential { .. }))
                    {
                        with_sequential += 1;
                    }
                }
                Err(e) => refused.push((id.clone(), e.message)),
            }
        }
        println!(
            "build_plan: {} flow(s) planned, {} refused, {with_sequential} carrying a \
             Sequential",
            flows.len() - refused.len(),
            refused.len()
        );
        for (id, msg) in &refused {
            println!("  refused {id}: {msg}");
        }
    }

    /// **The depth bound is derived from a measurement that is re-taken here.**
    ///
    /// `MAX_TASK_DEPTH` is `8 * MEASURED_MAX_CORPUS_DEPTH` in `plan.rs`. This
    /// re-measures the second factor from the committed corpus on every run, so
    /// the constant is a bound over a measurement rather than over a memory of
    /// one. It walks `models::Task::children()`, which sees every flowable —
    /// not only the one Task 3 descends.
    ///
    /// **It is a LOWER bound, not the depth, and the reason is this task's own
    /// finding.** Java's `AbstractBranch.allChildTasks()` concatenates `tasks`,
    /// `errors` and `finally`; `models::Task::children()` returns only the
    /// first, because W1's model **drops** every control-flow type's
    /// `errors`/`finally` fields (`models/task.rs`, `Sequential`'s comment).
    /// A subtree hanging off a task-level `errors:` is therefore invisible to
    /// this walk and to `build_plan` alike. Flow-level `errors:`, `finally:` and
    /// `afterExecution:` **are** in the model and are walked here. So the true
    /// corpus depth is at least what this measures, the 8× headroom is what
    /// absorbs the difference, and **the number may move when Task 7 widens the
    /// model** — at which point this test says so rather than the bound
    /// silently becoming tighter than it reads.
    #[test]
    fn the_depth_bound_is_derived_from_the_deepest_corpus_flow() {
        fn depth(task: &crate::models::Task) -> usize {
            1 + task.children().iter().map(|c| depth(c)).max().unwrap_or(0)
        }

        let mut deepest = 0usize;
        let mut deepest_id = String::new();
        for (id, source) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&source) else {
                continue;
            };
            let d = flow
                .tasks
                .iter()
                .chain(flow.errors.iter())
                .chain(flow.finally_.iter())
                .chain(flow.after_execution.iter())
                .map(depth)
                .max()
                .unwrap_or(0);
            if d > deepest {
                deepest = d;
                deepest_id = id;
            }
        }

        println!("deepest corpus flow: {deepest_id} nests {deepest} level(s)");
        assert_eq!(
            deepest,
            super::plan::MEASURED_MAX_CORPUS_DEPTH,
            "plan.rs's MEASURED_MAX_CORPUS_DEPTH is stale: the deepest corpus flow is \
             now {deepest_id} at {deepest}"
        );
    }

    /// A chain of `levels` nested tasks: `levels - 1` `Sequential`s with one
    /// `Return` at the bottom, so the deepest task sits at depth `levels`.
    fn nested_yaml(levels: usize) -> String {
        assert!(levels >= 1);
        let mut y = String::from("id: deep\nnamespace: io.kestra.tests\ntasks:\n");
        for level in 0..levels - 1 {
            let pad = "  ".repeat(level * 2 + 1);
            y.push_str(&format!(
                "{pad}- id: s{level}\n{pad}  type: io.kestra.plugin.core.flow.Sequential\n{pad}  tasks:\n"
            ));
        }
        let pad = "  ".repeat((levels - 1) * 2 + 1);
        y.push_str(&format!(
            "{pad}- id: leaf\n{pad}  type: io.kestra.plugin.core.debug.Return\n{pad}  format: x\n"
        ));
        y
    }

    /// **The depth bound fires, and says what it refused.**
    ///
    /// A guard nobody has seen fail is a guard nobody has seen, and this one is
    /// unreachable from the corpus by construction — the bound is eight times
    /// the deepest flow in it. So the input is generated.
    #[test]
    fn nesting_past_the_bound_is_refused_by_name() {
        let over = super::plan::MAX_TASK_DEPTH + 1;
        let planned = build_plan(&nested_yaml(over));
        // Not `expect_err`: on the mutation this test exists to kill, that
        // prints the whole 49-task `Plan` and buries the finding.
        assert!(
            planned.is_err(),
            "nesting past the bound must be refused; got a plan of {} task(s)",
            planned.as_ref().map(|p| p.tasks.len()).unwrap_or(0)
        );
        let err = planned.unwrap_err();
        assert!(
            err.message.contains("nests deeper"),
            "the message must say what is wrong; got {err}"
        );
        assert!(
            err.message.contains("id 'leaf'"),
            "the message must name the task it refused; got {err}"
        );

        // And exactly at the bound is accepted, so the assertion above is about
        // the bound and not about deep YAML failing for some other reason.
        let plan = build_plan(&nested_yaml(super::plan::MAX_TASK_DEPTH))
            .expect("exactly at the bound must still be planned");
        assert_eq!(plan.tasks.len(), super::plan::MAX_TASK_DEPTH);
    }

    /// **Only `io.kestra.plugin.core.flow.Sequential` is planned as a
    /// `Sequential`; every other flowable is opaque and its children are not
    /// flattened at all.**
    ///
    /// `AllowFailure` and `WorkingDirectory` both `extend Sequential` in Java
    /// and both stay opaque here, deliberately: see [`build_plan`]. A builder
    /// that mapped them would trade a visible mismatch for a plausible-looking
    /// wrong trace.
    ///
    /// The `Sequential` row is the control: the same YAML, one word different,
    /// comes out the other way. Without it this test would pass against a
    /// builder that flattened nothing at all.
    #[test]
    fn every_other_flowable_stays_opaque() {
        let branch = |ty: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: {ty}\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            )
        };

        for ty in [
            "io.kestra.plugin.core.flow.AllowFailure",
            "io.kestra.plugin.core.flow.WorkingDirectory",
            "io.kestra.plugin.core.flow.Loop",
            "io.kestra.plugin.core.flow.WaitFor",
        ] {
            let plan = build_plan(&branch(ty)).unwrap_or_else(|e| panic!("{ty}: {e}"));
            assert_eq!(
                plan.tasks.len(),
                1,
                "{ty} is opaque, so `inner` must not be in the plan at all"
            );
            assert_eq!(
                plan.tasks[0].kind,
                TaskKind::Runnable {
                    type_id: ty.to_string()
                },
                "{ty} must be planned opaque"
            );
        }

        let plan = build_plan(&branch("io.kestra.plugin.core.flow.Sequential"))
            .expect("the control must plan");
        assert_eq!(
            plan.tasks.len(),
            2,
            "the control: a Sequential DOES flatten its children"
        );
        assert_eq!(plan.tasks[0].kind, TaskKind::Sequential { tasks: vec![1] });

        // Task 4's two controls. `Parallel` moved out of the opaque list above
        // in the same commit that gave it a `TaskKind`, which is the whole
        // point of this test being a list rather than a rule.
        let plan = build_plan(&branch("io.kestra.plugin.core.flow.Parallel"))
            .expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Parallel {
                tasks: vec![1],
                concurrency: None
            },
            "a Parallel flattens its children too, as of Task 4"
        );
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          id: inner\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
        )
        .expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Dag {
                nodes: vec![DagNode {
                    task: 1,
                    depends_on: vec![]
                }]
            },
            "and so does a Dag"
        );
    }

    /// **An OVER-REFUSAL already on `main`, pinned to its one token.**
    ///
    /// `build_plan` refuses `valids/switch-impossible` — a flow this Kestra
    /// published and **ran** — and the cause is not in the executor. W1's
    /// `models::Switch::defaults` is a bare `Vec<Task>`, so an explicit
    /// `defaults: null` is a parse error; Jackson leaves the field null and
    /// Kestra runs the flow. `models/task.rs` applies the
    /// bean-validation-is-not-parsing principle correctly to
    /// `Parallel::concurrent` and every other scalar, and misses it on every
    /// bare `Vec<Task>`.
    ///
    /// This is not an oracle curiosity: `spec::parse` is what `routes.rs:335`
    /// (`create_flow`, `update_flow`) and `routes.rs:640` (`validate_flow`)
    /// call, so egret's HTTP API returns 400 for a flow this Kestra accepts.
    ///
    /// **Committed as a test because it is a defect with a deadline.** Two of
    /// the six `refused` entries Task 3 banked (`invalids/switch-invalid`,
    /// `invalids/invalid-parallel`) are refused by this same rule and are
    /// therefore right *by accident* — when W1 fixes the model this test
    /// flips, and whoever fixes it has to revisit `oracle-exec/baseline.txt`
    /// and Task 8's per-group message assertions in the same commit.
    #[test]
    fn a_null_task_list_is_an_over_refusal_that_reaches_the_http_api() {
        // The refusal, and the single token that causes it.
        let with_null = corpus_source("valids/switch-impossible");
        assert!(
            build_plan(&with_null).is_err(),
            "if this now plans, W1's model has been fixed -- delete this test, \
             re-run the ratchet, and revisit the two accidental refusals it names"
        );
        let without = with_null.replace("defaults: null", "");
        assert!(
            build_plan(&without).is_ok(),
            "`defaults: null` must be the WHOLE cause; if removing it still fails, \
             this test is pinning the wrong token"
        );

        // `invalids/switch-invalid` is refused by the same token, so its
        // refusal says nothing about its empty `cases`.
        let si = corpus_source("invalids/switch-invalid");
        assert!(build_plan(&si).is_err());
        assert!(
            build_plan(&si.replace("defaults: null", "")).is_ok(),
            "invalids/switch-invalid is refused for `defaults: null` and not for \
             anything Kestra judged about it"
        );

        // And in `invalids/invalid-parallel`, only `para4` refuses: an absent
        // `tasks` key and an empty list both parse. So the rule is exactly
        // "explicit null", not "missing or empty".
        let ip = corpus_source("invalids/invalid-parallel");
        assert!(build_plan(&ip).is_err());
        assert!(
            build_plan(&ip.replace("tasks: null", "")).is_ok(),
            "para2 (no `tasks` key) and para3 (`tasks: []`) must both parse -- if \
             they do not, the cause is wider than an explicit null"
        );
    }

    /// **A flow W1's parser cannot read is refused, located.**
    #[test]
    fn unparseable_source_is_refused_with_its_location() {
        let err = build_plan("id: x\nnamespace: y\ntasks: [").expect_err("unparseable");
        assert!(err.message.starts_with("cannot parse flow"), "got {err}");
    }

    /// **`build_plan` flattens in pre-order and children address their parent's
    /// list by index.**
    #[test]
    fn children_are_addressed_by_index_in_pre_order() {
        let plan = plan_of("valids/sequential");
        assert_eq!(plan.tasks[0].id, "1-seq");
        let TaskKind::Sequential { tasks } = &plan.tasks[0].kind else {
            panic!("1-seq is a Sequential");
        };
        assert!(
            tasks.iter().all(|&c| c > 0),
            "a child's index is always greater than its parent's in pre-order"
        );
        assert_eq!(
            tasks
                .iter()
                .map(|&c| plan.tasks[c].id.as_str())
                .collect::<Vec<_>>(),
            ["1-1", "1-2", "1-3_seq", "1-4_end"],
            "declaration order, as written"
        );
        assert_eq!(
            plan.roots
                .iter()
                .map(|&c| plan.tasks[c].id.as_str())
                .collect::<Vec<_>>(),
            ["1-seq", "2_end"]
        );
    }
}
