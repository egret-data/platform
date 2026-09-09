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
//! `dag`, and the two refusals a `Dag` can earn at plan-build time. W3a Task 5:
//! `if` and `switch`, which read the caller's render. W3a Task 6: `loop` and
//! [`Command::SpawnExecution`]. Every other task type is planned as an opaque
//! [`TaskKind::Runnable`]; see [`build_plan`] for why that is the safe default
//! rather than an approximation.
//!
//! Tasks 7-8 add the attempt lifecycle and refusal. They add variants to
//! [`TaskKind`] and [`Command`]; they do not change the signatures here.
//!
//! **W3c adds `runIf:`**, and with it a second obligation on the caller — see
//! [`advance`]'s *What the caller must supply*. It is called out in the module
//! doc because a caller that does not know about it does not get an error: it
//! gets an empty command vector for ever.
//!
//! # What Task 6 emits and does NOT execute
//!
//! A `Loop` does not iterate inside its execution: each value produces a
//! separate **sub-execution**, and [`advance`] asks for one with
//! [`Command::SpawnExecution`]. Nothing in W3a executes that command, and
//! [`ExecutionState`] holds no field for a sub-execution — so `advance` emits
//! the **first batch** (`min(concurrencyLimit, size)`, Kestra's own arithmetic)
//! and cannot schedule a second, and a `Loop` run's terminal state is the
//! caller's to supply. `oracle-exec/README.md` records that hole with its
//! counts; `flowable::loop_` derives why it is a consequence of purity rather
//! than a shortcut.

mod command;
mod flowable;
mod plan;
mod retry;
mod state;

pub use command::Command;
pub use plan::{
    build_plan, DagNode, Plan, PlanError, PlanTask, RetryBehavior, RetryDelay, RetryPolicy, TaskIx,
    TaskKind,
};
pub use state::{apply, Attempt, ExecutionMeta, ExecutionState, RunIx, RunState, TaskRun, Tick};

/// Kestra's `FlowableUtils.resolveValues`, over a rendered `Loop.values`.
///
/// Public because two callers outside this module need the same answer and must
/// not each write their own: `tests/executor.rs` derives what a first batch
/// should be from it, and W3b will need it to schedule the batches after the
/// first. See [`flowable::loop_::iteration_values`] for the rule and for the one
/// place it is knowingly wider than Kestra.
pub use flowable::loop_::iteration_values;

/// Kestra's `TruthUtils.isTruthy`, over an already-rendered string.
///
/// Public for the same reason [`iteration_values`] is: callers outside this
/// module need the same answer and must not each write their own. W3c's oracle
/// arm derives which corpus `runIf:` expressions render falsy and has to ask
/// the question the executor asks, not a transliteration of the Java that
/// agrees with it by luck — the trim alone has 11 measured rows where the
/// obvious transliteration is wrong. See [`flowable::branch::is_truthy`] for
/// the measurement.
pub use flowable::branch::is_truthy;

/// Java's `Duration.parse` over the ISO-8601 subset a flow can write.
///
/// **A visibility change, not a semantic one, and this is the sentence the
/// plan's Global Constraint asks for: nothing here is being fixed, so there
/// is nothing eight tasks of oracle work failed to catch.** `advance()` is
/// byte-unchanged; the only edit to `src/executor/` in W3b Task 3 is
/// `pub(super)` becoming `pub` on this function plus this re-export.
///
/// The reason is [`worker::handlers::sleep`](crate::worker::handlers::sleep):
/// `core.flow.Sleep`'s `duration:` is the same ISO-8601 grammar a `retry:`
/// writes, and a second parser beside this one would be a second table that
/// can disagree with the one it copies — including on the case-insensitivity
/// that `valids/retry-expo`'s `PT0.25s` depends on. One parser, two callers.
pub use retry::parse_duration;

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
///
/// # What the caller must supply, and the silence if it does not
///
/// `advance` is pure and cannot render, so two fields on
/// [`TaskRun`](crate::executor::TaskRun) are the caller's to fill, at two
/// different run states:
///
/// | field | filled when the run is | for |
/// |---|---|---|
/// | [`TaskRun::rendered_run_if`](crate::executor::TaskRun::rendered_run_if) | `CREATED` | the task's `runIf:`, if [`PlanTask::run_if`] is `Some` — W3c |
/// | [`TaskRun::rendered`](crate::executor::TaskRun::rendered) | `CREATED` for a `Loop`, `RUNNING` for an `If`/`Switch` | that flowable's own expression — Tasks 5-6 |
///
/// **An unanswered render is [`Waiting`](flowable::Step::Waiting), not an
/// error, and that is the trap.** A caller that never fills `rendered_run_if`
/// gets an empty `Vec` from every call, for ever, on any flow whose author
/// wrote a `runIf:` — a silent stall rather than a failure. It has to be a wait:
/// reading `None` as falsy would skip every task the caller had not reached, and
/// reading it as truthy would run every task the author had switched off. The
/// oracle's tier 2a is the reference caller; deleting it is mutation M10, and
/// it turns 5 golden entries `STUCK` rather than changing 5 traces.
///
/// **`rendered_run_if` is re-asked per attempt.** A retried run re-enters
/// `CREATED` and [`apply`] clears both slots there, so a caller that answers
/// once and caches is answering only for attempt 1. See `apply`'s rule 2.
pub fn advance(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    if state.history.last().is_some_and(|s| s.is_terminal()) {
        return Vec::new();
    }

    // The execution-delay loop, which in the artifact is a different loop from
    // the one below (`DefaultExecutor.java:561`) whose output is re-submitted
    // for processing rather than folded into the cycle that scheduled it. So it
    // is emitted alone, and the next round does the rest.
    let reopened = retry::reopen_due(plan, state, now);
    if !reopened.is_empty() {
        return reopened;
    }

    // # Kestra's two `isRetrying` guards are NOT here, and deleting them was
    // measured rather than assumed
    //
    // `ExecutorService.handleEnd` returns at once while the execution is
    // `RETRYING`, and its retry pass is gated on the same thing
    // (`!executor.getExecution().getState().isRetrying()`). Both were ported,
    // and both turned out to be **unreachable**: a mutation that deleted each
    // (`Step::Done(_) if retrying => Vec::new()`, and the `if !retrying` around
    // the scan) left the whole crate at 0 failures with the ratchet unmoved, so
    // they were deleted rather than shipped as comments nobody could check —
    // the same answer `parallel::drive_concurrent` records for Kestra's "first
    // created, leave".
    //
    // **What makes them unreachable is a property of [`retry::scan`], not of
    // this function.** `scan` writes `SetRunState { state: Retrying }` and
    // `SetExecutionState { state: Retrying }` in the same round, so no later
    // round can see a `Failed` run while the execution is `RETRYING`:
    // `flowable::current_list` finds no failure, returns `Current::Tasks`, and
    // the walk answers `Waiting` on a `Retrying` child. Both guards are about a
    // state where those two writes have come apart.
    //
    // **So the trigger to watch for is a caller that applies a command batch
    // PARTIALLY** — W3b's message-driven driver is the candidate, exactly as it
    // is for the `Parallel` guard. Whoever gives `advance` one has to bring both
    // guards back, and bring the test that makes them fire with them.
    let mut work = match sequential::drive(plan, state, &plan.roots, &plan.errors, None, now) {
        Step::Commands(cmds) => cmds,
        Step::Waiting => Vec::new(),
        Step::Done(state) => vec![Command::SetExecutionState { state }],
    };

    // `handleFlowableTasks`'s retry pass, which runs after `handleEnd` and is
    // therefore behind whatever state the execution just took. That order is
    // visible: `valids/retry-failed-task-attempts`'s execution reads
    // `… FAILED RETRYING …`, never `… RETRYING FAILED …`.
    work.extend(retry::scan(plan, state, now));

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
    use std::collections::{BTreeMap, BTreeSet};

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

    /// **A `Runnable` is `SUBMITTED` before it is `RUNNING`; a flowable is not,
    /// and neither is a [`TaskKind::Executable`].**
    ///
    /// Measured on `valids/sequential`, which nests the first two: the
    /// `Sequential` runs go `CREATED RUNNING SUCCESS` and every `Return` goes
    /// `CREATED SUBMITTED RUNNING SUCCESS`. Deleting the `SUBMITTED` emission
    /// leaves a trace that still has the right runs in the right order and
    /// matches nothing.
    ///
    /// **The third arm is W6's whole reason for opening `src/executor/`**, and
    /// `valids/subflow-parent` is the flow that pins it: its one `Subflow` run
    /// reads `CREATED RUNNING SUCCESS` in `golden.json`, and it read
    /// `CREATED SUBMITTED RUNNING SUCCESS` here for as long as a `Subflow` was
    /// planned an opaque `Runnable`. The golden side of the same claim is
    /// derived over all 25 declarers by `tests/executor.rs`'s
    /// `no_subflow_run_in_the_golden_passes_through_submitted`; this is the
    /// egret side of it.
    #[test]
    fn only_a_runnable_passes_through_submitted() {
        // One flow per flowable this executor drives, so the discriminator is
        // pinned on all three and not only on the one Task 3 shipped.
        let mut flowables = 0;
        let mut runnables = 0;
        let mut executables = 0;
        for (corpus_id, renders) in [
            ("valids/sequential", &[][..]),
            ("valids/parallel", &[][..]),
            ("valids/dag", &[][..]),
            ("valids/if-condition", &[("if", "true")][..]),
            ("valids/switch", &[("parent-seq", "FIRST")][..]),
            ("valids/loop-serial", &[][..]),
            ("valids/subflow-parent", &[][..]),
        ] {
            let plan = plan_of(corpus_id);
            let state = run_to_fixpoint_rendering(&plan, RunState::Success, renders);
            for run in &state.runs {
                match plan.tasks[run.task].kind {
                    TaskKind::Sequential { .. }
                    | TaskKind::Parallel { .. }
                    | TaskKind::Dag { .. }
                    | TaskKind::If { .. }
                    | TaskKind::Switch { .. }
                    | TaskKind::Loop { .. } => {
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
                    TaskKind::Executable { .. } => {
                        executables += 1;
                        assert_eq!(
                            wire(&run.history),
                            ["CREATED", "RUNNING", "SUCCESS"],
                            "{corpus_id}: executable {} must NOT pass through SUBMITTED -- \
                             no Subflow run in golden.json carries it",
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
            flowables >= 8 && runnables >= 21 && executables >= 1,
            "the seven flows must exercise all three sides, and the two Task 5 \
             added plus the one Task 6 added must each contribute a flowable; \
             saw {flowables} flowable(s), {runnables} runnable(s) and \
             {executables} executable(s)"
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
    // branch: if and switch
    // ----------------------------------------------------------------------

    /// The unrendered expression this task holds, if it holds one.
    ///
    /// The same three kinds `tests/executor.rs`'s tier 2 renders, and the same
    /// function shape, because the two callers of `advance` must not disagree
    /// on which runs need a render.
    fn expression_of(plan: &Plan, task: TaskIx) -> Option<String> {
        match &plan.tasks[task].kind {
            TaskKind::If { condition, .. } => Some(condition.clone()),
            TaskKind::Switch { value, .. } => Some(value.clone()),
            TaskKind::Loop { values, .. } => Some(values.clone()),
            _ => None,
        }
    }

    /// The run state at which this task's expression is rendered.
    ///
    /// **`CREATED` for a `Loop` and `RUNNING` for the other two, measured.** An
    /// `If` is started and then asked; a `Loop` is asked and then started, or
    /// not started at all -- `valids/loop-empty`'s `1_each` reads `CREATED
    /// SUCCESS` with no `RUNNING` and no attempt.
    fn renders_at(plan: &Plan, task: TaskIx) -> Option<RunState> {
        match &plan.tasks[task].kind {
            TaskKind::Loop { .. } => Some(RunState::Created),
            _ => Some(RunState::Running),
        }
    }

    /// [`run_to_fixpoint`], with a caller that also **renders**.
    ///
    /// `advance` is pure and cannot render, so a real caller supplies the
    /// result in `TaskRun::rendered`. This is the smallest caller that does:
    /// whenever the state machine has nothing to do, any planned `If`/`Switch`
    /// run that is `Running` and unrendered gets the value `renders` gives for
    /// its task id, and only then does the worker finish anything.
    ///
    /// Rendering before the worker moves is Kestra's order too: it renders in
    /// the **executor** (`If.isTrue` calls `runContext.render`), not in a
    /// worker, so no worker result can arrive between the executor deciding to
    /// evaluate a condition and the evaluation. It renders **all** of the
    /// unrendered runs in one step for the same reason, and because
    /// `tests/executor.rs`'s tier 2 does: that shape is not free — rendering
    /// one per step changes `valids/if`'s run order and fails a committed
    /// oracle test — so the two callers must not differ on it.
    ///
    /// An id with no entry is a test bug rather than a "not rendered yet", so
    /// it panics: a missing render would otherwise read exactly like the
    /// deliberate `None` case that
    /// [`an_if_with_no_rendered_condition_asks_for_nothing_and_is_not_an_error`]
    /// is about.
    fn run_to_fixpoint_rendering(
        plan: &Plan,
        outcome: RunState,
        renders: &[(&str, &str)],
    ) -> ExecutionState {
        run_to_fixpoint_rendering_run_if(plan, outcome, renders, &[])
    }

    /// [`run_to_fixpoint_rendering`], with the caller also answering `runIf:` --
    /// W3c's second render slot, keyed by task id in its own list because one
    /// task can need both.
    ///
    /// The `runIf:` pass comes FIRST and in its own step, exactly as
    /// `tests/executor.rs`'s tier 2a does, and for the same reason: Kestra
    /// decides `runIf:` before it resolves anything else about the task, so a
    /// `Loop` whose `runIf:` is falsy is `SKIPPED` without its `values:` ever
    /// being looked at. Two callers of `advance` differing on that would be two
    /// answers to the question one of them is the guard for.
    fn run_to_fixpoint_rendering_run_if(
        plan: &Plan,
        outcome: RunState,
        renders: &[(&str, &str)],
        run_ifs: &[(&str, &str)],
    ) -> ExecutionState {
        let mut state = ExecutionState::new(plan);
        for _ in 0..1000 {
            let cmds = advance(plan, &state, T0);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    apply(&mut state, cmd);
                }
                continue;
            }
            // W3c's tier 2a.
            let unrendered_run_if: Vec<RunIx> = (0..state.runs.len())
                .filter(|&ix| {
                    plan.tasks[state.runs[ix].task].run_if.is_some()
                        && state.runs[ix].history.last() == Some(&RunState::Created)
                        && state.runs[ix].rendered_run_if.is_none()
                })
                .collect();
            if !unrendered_run_if.is_empty() {
                for ix in unrendered_run_if {
                    let id = &plan.tasks[state.runs[ix].task].id;
                    let source = plan.tasks[state.runs[ix].task]
                        .run_if
                        .clone()
                        .expect("filtered on Some above");
                    // Same identity short-circuit as below: a `runIf:` with no
                    // `{{` renders to itself, which is how a literal
                    // `runIf: "false"` needs no entry here.
                    let value = if source.contains("{{") {
                        run_ifs
                            .iter()
                            .find(|(k, _)| k == id)
                            .unwrap_or_else(|| panic!("no runIf render supplied for task '{id}'"))
                            .1
                            .to_string()
                    } else {
                        source
                    };
                    state.runs[ix].rendered_run_if = Some(value);
                }
                continue;
            }
            // ALL of the unrendered runs in one step, which is what
            // `tests/executor.rs`'s tier 2 does. The two used to disagree on
            // exactly the shape that turned out not to be free: rendering one
            // per step changes `valids/if`'s run ORDER, and a committed oracle
            // test dies to it. Two callers of `advance` differing on the
            // question one of them is the guard for is how a guard stops
            // guarding.
            let unrendered: Vec<RunIx> = (0..state.runs.len())
                .filter(|&ix| {
                    expression_of(plan, state.runs[ix].task).is_some()
                        && renders_at(plan, state.runs[ix].task)
                            == state.runs[ix].history.last().copied()
                        && state.runs[ix].rendered.is_none()
                })
                .collect();
            if !unrendered.is_empty() {
                for ix in unrendered {
                    let id = &plan.tasks[state.runs[ix].task].id;
                    let source =
                        expression_of(plan, state.runs[ix].task).expect("filtered on Some above");
                    // W2's own identity short-circuit: a source with no `{{`
                    // renders to itself (`VariableRenderer.java:76-79`). So a
                    // literal `values:` needs no entry here, and a test cannot
                    // pass by retyping a list `build_plan` read from the wrong
                    // field.
                    let value = if source.contains("{{") {
                        renders
                            .iter()
                            .find(|(k, _)| k == id)
                            .unwrap_or_else(|| panic!("no render supplied for task '{id}'"))
                            .1
                            .to_string()
                    } else {
                        source
                    };
                    state.runs[ix].rendered = Some(value);
                }
                continue;
            }
            // A `Loop` is in flight here for the same reason a `Runnable` is:
            // the work it waits on -- its sub-executions -- is not simulated by
            // this fixpoint, so its terminal comes from outside. See the
            // oracle's tier 3 and `oracle-exec/README.md`.
            let in_flight = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. } | TaskKind::Loop { .. } | TaskKind::Executable { .. }
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

    /// **A rendered `true` takes `then`, a rendered `false` takes `else`.**
    ///
    /// The plan's own Task 5 test, on Kestra's own flow: `valids/if-condition`
    /// has `then: [when-true]` and `else: [when-false]`, and the golden for it
    /// — captured with `param: true` — is exactly `[if, when-true]`. The
    /// alternative input `param: false` was *measured* on the same artifact by
    /// Task 1's discrimination probe and gave `[if, when-false]`
    /// (`probe.json`, `IfTest.java:76 ifFalsy()`), which is the other half.
    #[test]
    fn an_if_takes_then_when_true_and_else_when_false() {
        let plan = plan_of("valids/if-condition");
        let taken = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", "true")]);
        assert_eq!(
            run_ids(&plan, &taken),
            ["if", "when-true"],
            "a truthy condition takes `then`, and `else` is never created"
        );
        assert_eq!(
            wire(&taken.runs[0].history),
            ["CREATED", "RUNNING", "SUCCESS"]
        );

        let not_taken = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", "false")]);
        assert_eq!(
            run_ids(&plan, &not_taken),
            ["if", "when-false"],
            "a falsy condition takes `else`, and `then` is never created"
        );
    }

    /// **An `If` that has not been rendered yet asks for nothing, and that is
    /// not an error.**
    ///
    /// `advance` is pure and cannot render, so `rendered: None` means "the
    /// caller has not answered yet" and the only correct answer is to emit
    /// nothing. The control is what makes this an assertion rather than a
    /// tautology: the *same* state with a render supplied does emit, so the
    /// emptiness is about the missing render and not about the `If` being
    /// unable to do anything at all.
    #[test]
    fn an_if_with_no_rendered_condition_asks_for_nothing_and_is_not_an_error() {
        let plan = plan_of("valids/if-condition");
        let mut state = ExecutionState::new(&plan);
        round(&plan, &mut state); // create the If
        round(&plan, &mut state); // start it
        assert_eq!(state.runs.len(), 1);
        assert_eq!(state.runs[0].history.last(), Some(&RunState::Running));
        assert_eq!(state.runs[0].rendered, None);

        assert_eq!(
            advance(&plan, &state, T0),
            vec![],
            "an unrendered If asks for nothing -- it must not guess a branch, \
             and it must not fail"
        );

        // The control: the same state, rendered, does ask for something.
        let mut rendered = state.clone();
        rendered.runs[0].rendered = Some("true".to_string());
        assert_eq!(
            advance(&plan, &rendered, T0),
            vec![Command::CreateRun {
                task: 1,
                parent: Some(0),
                value: None
            }],
            "with the render supplied the same If creates its first `then` child"
        );
    }

    /// **Kestra's truthiness is not Rust's, and this is the measurement.**
    ///
    /// `TruthUtils.isTruthy` is `condition != null && !["false", "0", "-0",
    /// ""].contains(condition.trim())`, and the rule was measured against the
    /// running artifact rather than read off that line: flows each holding an
    /// `If` with a **literal** condition (so the rendered value is exactly the
    /// literal and the measurement isolates the predicate from the renderer),
    /// published to `kestra/kestra:v2.0.0-rc13` and executed, recording which
    /// branch got a taskrun. Every row below is one of those executions.
    ///
    /// **The first 20 rows** were the original measurement: `"0"` and `"-0"`
    /// are **falsy** while `"00"`, `"0.0"`, `"FALSE"`, `"False"`, `"null"`,
    /// `"no"`, `"[]"` and `"{}"` are all **truthy** — the list is four exact
    /// strings, not a parse, not a case-insensitive match and not an emptiness
    /// test.
    ///
    /// **The other 18 are the boundary those 20 could not reach**, added in fix
    /// round 1 because every one of the original padded rows pads with an ASCII
    /// **space**, where every candidate trim agrees. Java's `String.trim()`
    /// strips code units `<= U+0020`; Rust's `str::trim` strips Unicode
    /// `White_Space`. The two sets neither contain nor are contained by each
    /// other, and **11 of these rows disagree with `str::trim()` in both
    /// directions** — asserted below by name, so a revert to `.trim()` fails
    /// here rather than taking a silent wrong branch on an NBSP.
    #[test]
    fn kestra_truthiness_is_four_exact_strings_compared_after_trimming() {
        // (rendered condition, branch Kestra took) -- one live execution each.
        let measured = [
            // --- the original 20 ---
            ("true", "when-true"),
            ("false", "when-false"),
            ("0", "when-false"),
            ("-0", "when-false"),
            ("", "when-false"),
            ("  true  ", "when-true"),
            ("  false  ", "when-false"),
            (" 0 ", "when-false"),
            ("   ", "when-false"),
            ("1", "when-true"),
            ("00", "when-true"),
            ("0.0", "when-true"),
            ("abc", "when-true"),
            ("FALSE", "when-true"),
            ("False", "when-true"),
            ("null", "when-true"),
            ("no", "when-true"),
            ("[]", "when-true"),
            ("{}", "when-true"),
            ("true\n", "when-true"),
            // --- the trim boundary, fix round 1 ---
            // In BOTH sets, so both trims agree: these are the control.
            ("\u{20}false\u{20}", "when-false"),
            ("\u{9}false\u{9}", "when-false"),
            ("\u{a}false\u{a}", "when-false"),
            ("\u{d}false\u{d}", "when-false"),
            ("\u{b}false\u{b}", "when-false"),
            ("\u{c}false\u{c}", "when-false"),
            // <= U+0020 but NOT Unicode White_Space: Java trims, Rust does not.
            ("\u{1}false\u{1}", "when-false"),
            ("\u{e}false\u{e}", "when-false"),
            ("\u{1f}false\u{1f}", "when-false"),
            ("\u{1}0\u{1}", "when-false"),
            // Unicode White_Space but > U+0020: Rust trims, Java does not.
            ("\u{a0}false\u{a0}", "when-true"),
            ("\u{85}false\u{85}", "when-true"),
            ("\u{3000}false\u{3000}", "when-true"),
            ("\u{2007}false\u{2007}", "when-true"),
            ("\u{2028}false\u{2028}", "when-true"),
            ("\u{a0}0\u{a0}", "when-true"),
            ("\u{a0}", "when-true"),
            // Neither: the control on the other side.
            ("\u{21}false\u{21}", "when-true"),
        ];
        let plan = plan_of("valids/if-condition");
        let mut falsy = 0;
        for (rendered, branch) in measured {
            let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", rendered)]);
            if branch == "when-false" {
                falsy += 1;
            }
            assert_eq!(
                run_ids(&plan, &state),
                ["if", branch],
                "condition rendered {rendered:?} -- measured live on \
                 kestra/kestra:v2.0.0-rc13, which took `{branch}`"
            );
        }
        assert_eq!(
            (falsy, measured.len() - falsy),
            (17, 21),
            "the measured population: 17 falsy renders and 21 truthy ones, \
             {} rows in total",
            measured.len()
        );

        // **The boundary rows are load-bearing, derived rather than trusted.**
        // A predicate built on `str::trim()` reads identically and is wrong on
        // exactly these, in both directions. Asserting the SET (not a count)
        // means a row that stops discriminating changes an assertion.
        let rust_trim_disagrees: Vec<&str> = measured
            .iter()
            .filter(|(rendered, branch)| {
                let rust_says = if !matches!(rendered.trim(), "false" | "0" | "-0" | "") {
                    "when-true"
                } else {
                    "when-false"
                };
                rust_says != *branch
            })
            .map(|(rendered, _)| *rendered)
            .collect();
        assert_eq!(
            rust_trim_disagrees,
            [
                "\u{1}false\u{1}",
                "\u{e}false\u{e}",
                "\u{1f}false\u{1f}",
                "\u{1}0\u{1}",
                "\u{a0}false\u{a0}",
                "\u{85}false\u{85}",
                "\u{3000}false\u{3000}",
                "\u{2007}false\u{2007}",
                "\u{2028}false\u{2028}",
                "\u{a0}0\u{a0}",
                "\u{a0}",
            ],
            "`str::trim()` (Unicode White_Space) disagrees with Kestra's \
             `String.trim()` (code units <= U+0020) on exactly these 11 of the \
             {} rows, in both directions -- 4 where Java trims and Rust does \
             not, 7 where Rust trims and Java does not. If this list empties, \
             the boundary rows have stopped discriminating and the fix they \
             hold is unguarded",
            measured.len()
        );
        assert_eq!(rust_trim_disagrees.len(), 11, "4 + 7 = 11");
    }

    /// **An `If` whose chosen branch is empty succeeds immediately, and that is
    /// the golden's own answer.**
    ///
    /// `valids/if` is driven with `if1: true, if2: false, if3: true` and its
    /// three `If` tasks have a `then` and no `else`. Kestra's `taskRunList`
    /// carries `if-2` with the history `CREATED RUNNING SUCCESS` and **no child
    /// at all** — `If.resolveState` short-circuits an empty child list into
    /// `guessFinalState(null, ...)`, which is `SUCCESS`.
    ///
    /// It is not a property of `else` being *absent*: on the running artifact an
    /// explicit `else: []` with a falsy condition ended `SUCCESS` with one
    /// taskrun too, which is Java's `ListUtils.emptyOnNull`.
    ///
    /// **A `Switch` does the opposite**, which is why this is asserted here and
    /// not as a rule about branching — see
    /// [`a_switch_with_no_matching_case_and_no_defaults_fails`].
    #[test]
    fn an_if_whose_chosen_branch_is_empty_succeeds_with_no_children() {
        let plan = plan_of("valids/if-without-else");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", "false")]);
        assert_eq!(
            run_ids(&plan, &state),
            ["if"],
            "the empty `else` produces no run at all"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "SUCCESS"]
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "SUCCESS"]);

        // The control: the same flow, rendered truthy, does create a child.
        let taken = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", "true")]);
        assert_eq!(run_ids(&plan, &taken), ["if", "when-true"]);
    }

    /// **A branch is a SEQUENTIAL list: it stops at the first child that does
    /// not succeed, and the `If` takes that child's state.**
    ///
    /// Kestra's `If.resolveNexts` calls `FlowableUtils.resolveSequentialNexts`,
    /// and the shape was measured on the running artifact rather than read off
    /// that call: an `If` with `condition: "true"` and
    /// `then: [a(Log), b(Fail), c(Log)]` produced exactly
    /// `if(FAILED), a(SUCCESS), b(FAILED)` — `c` was never created.
    #[test]
    fn an_if_branch_is_sequential_and_stops_at_the_first_failure() {
        let source = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"true\"\n    then:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n      - id: c\n        type: io.kestra.plugin.core.debug.Return\n        format: c\n";
        let plan = build_plan(source).expect("plans");
        let state = run_to_fixpoint_rendering(&plan, RunState::Failed, &[("if", "true")]);
        assert_eq!(
            run_ids(&plan, &state),
            ["if", "a"],
            "the first child failed, so `b` and `c` were never created"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED"]
        );

        let ok = run_to_fixpoint_rendering(&plan, RunState::Success, &[("if", "true")]);
        assert_eq!(
            run_ids(&plan, &ok),
            ["if", "a", "b", "c"],
            "the control: with a succeeding worker the whole branch runs, one at a time"
        );
    }

    /// **Three `If`s inside one `Parallel` produce Kestra's own `taskRunList`,
    /// verbatim — which is what makes the render tier's batching load-bearing.**
    ///
    /// `valids/if` is a `Parallel` with `concurrent: 4` over nine branches,
    /// three of them `If`s, driven with `if1: true, if2: false, if3: true`. The
    /// assertion is the golden's own run order for it: the three `If`s and one
    /// `Log` are created in the first batch, the two taken branches' children
    /// next, and the remaining `Log`s as slots free up. `if-2` takes an absent
    /// `else`, so it terminates with no child at all — the empty-branch rule,
    /// on the corpus rather than on a synthetic flow.
    ///
    /// **A caller that rendered one run per round could not produce this.** It
    /// would resolve `if-1`, create `if-1-log`, and only then look at `if-3`,
    /// which reorders `taskRunList`. That shape was measured to fail a
    /// committed oracle test too (`valids/if` joins its `runs_differ` list), and
    /// this is the unit-level statement of the same rule — so the two callers of
    /// `advance()` cannot drift apart on it.
    ///
    /// The entry itself is still baselined, on three ordered pairs no clockless
    /// machine can reach; its runs, which are what this asserts, are exact.
    #[test]
    fn three_ifs_in_a_parallel_produce_kestras_own_run_order() {
        let plan = plan_of("valids/if");
        let state = run_to_fixpoint_rendering(
            &plan,
            RunState::Success,
            &[("if-1", "true"), ("if-2", "false"), ("if-3", "true")],
        );
        assert_eq!(
            run_ids(&plan, &state),
            [
                "parallel",
                "if-1",
                "if-2",
                "if-3",
                "log-parallel-1",
                "if-1-log",
                "if-3-log",
                "log-parallel-2",
                "log-parallel-3",
                "log-parallel-4",
                "log-parallel-5",
                "log-parallel-6",
            ],
            "valids/if's taskRunList from golden.json, verbatim"
        );
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "RUNNING", "SUCCESS"],
            "if-2 rendered falsy and has no `else`, so it succeeds with no child"
        );
        assert!(
            state.runs.iter().all(|r| r.parent != Some(2)),
            "and nothing at all was created under it"
        );
    }

    /// **A `Switch` picks the case whose key EQUALS the rendered value.**
    ///
    /// `valids/switch` has four cases and a `defaults`, and both halves are
    /// measured on the artifact: the golden was captured with `string: FIRST`
    /// and is `[parent-seq, t1]`, and the discrimination probe re-ran it with
    /// `string: SECOND` and observed `[parent-seq, t2, t2_sub]`
    /// (`probe.json`, `SwitchTest.java:57 switchSecond()`). The second case is
    /// what makes this a test of the key equality rather than of "the first
    /// case wins".
    #[test]
    fn a_switch_picks_the_case_whose_key_equals_the_rendered_value() {
        let plan = plan_of("valids/switch");
        let first = run_to_fixpoint_rendering(&plan, RunState::Success, &[("parent-seq", "FIRST")]);
        assert_eq!(run_ids(&plan, &first), ["parent-seq", "t1"]);

        let second =
            run_to_fixpoint_rendering(&plan, RunState::Success, &[("parent-seq", "SECOND")]);
        assert_eq!(
            run_ids(&plan, &second),
            ["parent-seq", "t2", "t2_sub"],
            "a second key, driven sequentially -- so this is the equality and \
             not `cases.first()`"
        );

        let default =
            run_to_fixpoint_rendering(&plan, RunState::Success, &[("parent-seq", "DEFAULT")]);
        assert_eq!(
            run_ids(&plan, &default),
            ["parent-seq", "default"],
            "no key equals DEFAULT, so `defaults` runs"
        );
    }

    /// **A `Switch` with no matching case and no `defaults` FAILS — where an
    /// `If` with an empty branch succeeds.**
    ///
    /// Measured, on `valids/fail-on-switch` driven with `param: fail`: the
    /// golden is one run, `switch`, `CREATED RUNNING FAILED`, and the execution
    /// `FAILED` with it. `valids/switch-impossible` is the same shape.
    ///
    /// The two flowables differ because `Switch` has no override of
    /// `resolveState`: its `childTasks` returns Java `null`, and
    /// `FlowableUtils.resolveState` logs "No task found on flow" and answers
    /// `FAILED` for a null list. `If.resolveState` wraps the same list in
    /// `ListUtils.emptyOnNull` first and answers `SUCCESS`.
    #[test]
    fn a_switch_with_no_matching_case_and_no_defaults_fails() {
        let plan = plan_of("valids/fail-on-switch");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[("switch", "fail")]);
        assert_eq!(
            run_ids(&plan, &state),
            ["switch"],
            "no case matched and there is no `defaults`, so nothing was created"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED"],
            "a Switch with nothing to run FAILS -- an If in the same position \
             SUCCEEDS, see an_if_whose_chosen_branch_is_empty_succeeds_with_no_children"
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "FAILED"]);

        // The control: a value that DOES match runs its case and succeeds.
        let ok = run_to_fixpoint_rendering(&plan, RunState::Success, &[("switch", "case1")]);
        assert_eq!(run_ids(&plan, &ok), ["switch", "case1"]);
        assert_eq!(wire(&ok.history), ["CREATED", "RUNNING", "SUCCESS"]);
    }

    /// **A `Switch` whose matched case is an EMPTY list never progresses, and
    /// that is Kestra's behaviour rather than an omission here.**
    ///
    /// Measured live: a flow with `cases: {MATCH: []}` and a non-empty
    /// `defaults`, executed with `value: "MATCH"`, was still `RUNNING` after
    /// 45 s with only the `sw` taskrun. It does **not** fall through to
    /// `defaults` — `Switch.childTasks` is
    /// `filter(key.equals(value)).findFirst().orElse(defaults)`, and an empty
    /// matched list is a `findFirst()` hit — and it does not fail either,
    /// because an empty (non-null) list reaches `resolveState`'s last branch
    /// and returns `Optional.empty()`.
    ///
    /// Reproducing it means emitting nothing for ever, which in the oracle's
    /// environment is a `STUCK` — so this can only be a unit test, and no
    /// corpus flow reaches it.
    #[test]
    fn a_switch_whose_matched_case_is_empty_asks_for_nothing_for_ever() {
        let source = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: sw\n    type: io.kestra.plugin.core.flow.Switch\n    value: MATCH\n    cases:\n      MATCH: []\n    defaults:\n      - id: fallback\n        type: io.kestra.plugin.core.debug.Return\n        format: f\n";
        let plan = build_plan(source).expect("plans");
        let mut state = ExecutionState::new(&plan);
        round(&plan, &mut state); // create
        round(&plan, &mut state); // start
        state.runs[0].rendered = Some("MATCH".to_string());
        assert_eq!(
            advance(&plan, &state, T0),
            vec![],
            "an empty matched case neither runs `defaults` nor fails"
        );
        assert_eq!(state.runs.len(), 1);

        // The control: the same flow with a value that matches nothing DOES
        // reach `defaults`, so the emptiness above is about the empty case and
        // not about this Switch being unable to do anything.
        let mut other = state.clone();
        other.runs[0].rendered = Some("NOPE".to_string());
        assert_eq!(
            advance(&plan, &other, T0),
            vec![Command::CreateRun {
                task: 1,
                parent: Some(0),
                value: None
            }]
        );
    }

    /// **An `If` with no `condition` cannot be planned, and is refused by
    /// name.**
    ///
    /// `condition` is `Option<String>` in W1's model because `@NotNull` is bean
    /// validation and not parsing; an `If` with none has nothing to branch on.
    /// This Kestra refuses it at publish — `invalids/if-without-condition`'s
    /// recorded rejection is `422 tasks[if].condition: must not be null` — so
    /// this is a refusal about the flow, the same class as the two `Dag` checks
    /// Task 4 added and not the widening `build_plan`'s doc argues against.
    #[test]
    fn an_if_with_no_condition_is_refused_by_name() {
        // Not `expect_err`: it prints the whole `Plan` and buries the finding,
        // which is Task 3's M6 lesson and Task 4's M18/M19 arriving a third
        // time. A mutation here has to say what it produced, not dump it.
        let planned = build_plan(&corpus_source("invalids/if-without-condition"));
        let err = match planned {
            Err(e) => e,
            Ok(p) => panic!(
                "an If with no condition must be refused; got a plan of {} task(s) \
                 rooted at '{}'",
                p.tasks.len(),
                p.tasks.first().map_or("?", |t| t.id.as_str())
            ),
        };
        assert!(
            err.message.contains("has no condition"),
            "refused for the wrong reason: {err}"
        );

        // The control: the same flow with a condition plans.
        let with = corpus_source("invalids/if-without-condition").replace(
            "    type: io.kestra.plugin.core.flow.If",
            "    type: io.kestra.plugin.core.flow.If\n    condition: \"true\"",
        );
        let plan = build_plan(&with).expect("the control must plan");
        assert!(matches!(plan.tasks[0].kind, TaskKind::If { .. }));
    }

    /// **A `Switch` with no `value` cannot be planned, and is refused by
    /// name.** The same rule and the same reason as
    /// [`an_if_with_no_condition_is_refused_by_name`]; `Switch::value` is
    /// `@NotNull` in Java too. No corpus flow is this shape, so the flow is
    /// written here — and the control below is what stops the assertion passing
    /// for any other reason.
    #[test]
    fn a_switch_with_no_value_is_refused_by_name() {
        let without = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: sw\n    type: io.kestra.plugin.core.flow.Switch\n    cases:\n      A:\n        - id: a\n          type: io.kestra.plugin.core.debug.Return\n          format: a\n";
        let err = match build_plan(without) {
            Err(e) => e,
            Ok(p) => panic!(
                "a Switch with no value must be refused; got a plan of {} task(s) \
                 rooted at '{}'",
                p.tasks.len(),
                p.tasks.first().map_or("?", |t| t.id.as_str())
            ),
        };
        assert!(
            err.message.contains("has no value"),
            "refused for the wrong reason: {err}"
        );

        let with = without.replace(
            "    type: io.kestra.plugin.core.flow.Switch",
            "    type: io.kestra.plugin.core.flow.Switch\n    value: A",
        );
        let plan = build_plan(&with).expect("the control must plan");
        assert!(matches!(plan.tasks[0].kind, TaskKind::Switch { .. }));
    }

    /// **The unrendered expression reaches the plan verbatim, and it is the
    /// only thing about the condition the plan holds.**
    ///
    /// A `Plan` that stored a rendered `bool` would be a plan that could only
    /// be built by something that can render, which `build_plan` is documented
    /// not to be. Asserting the source text is what pins that.
    #[test]
    fn the_plan_holds_the_unrendered_expression() {
        let plan = plan_of("valids/if-condition");
        let TaskKind::If {
            condition,
            then_,
            else_,
        } = &plan.tasks[0].kind
        else {
            panic!("valids/if-condition's first task is an If");
        };
        assert_eq!(condition, "{{inputs.param}}");
        assert_eq!(
            (
                then_
                    .iter()
                    .map(|&t| plan.tasks[t].id.as_str())
                    .collect::<Vec<_>>(),
                else_
                    .iter()
                    .map(|&t| plan.tasks[t].id.as_str())
                    .collect::<Vec<_>>()
            ),
            (vec!["when-true"], vec!["when-false"])
        );

        let plan = plan_of("valids/switch");
        let TaskKind::Switch {
            value,
            cases,
            default,
        } = &plan.tasks[0].kind
        else {
            panic!("valids/switch's first task is a Switch");
        };
        assert_eq!(value, "{{inputs.string}}");
        assert_eq!(
            cases.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["FIRST", "SECOND", "THIRD"]
        );
        assert_eq!(default.len(), 1, "valids/switch has one `defaults` task");
    }

    // ----------------------------------------------------------------------
    // the loop
    // ----------------------------------------------------------------------

    /// Drive `plan` until the run of `loop_id` exists, then hand the caller's
    /// render of that loop's `values` in and ask `advance` once more.
    ///
    /// The render is the loop's **own literal `values` source**, which is what
    /// W2's renderer returns for a source with no `{{` (Kestra's own
    /// short-circuit, `VariableRenderer.java:76-79`). Taking it from the plan
    /// rather than writing it out again is deliberate: a test that retyped the
    /// list would still pass against a `build_plan` that read the wrong field.
    fn spawn_round(plan: &Plan, loop_id: &str) -> (ExecutionState, Vec<Command>) {
        let mut state = ExecutionState::new(plan);
        for _ in 0..100 {
            let ix =
                (0..state.runs.len()).find(|&ix| plan.tasks[state.runs[ix].task].id == loop_id);
            if let Some(ix) = ix {
                let TaskKind::Loop { values, .. } = &plan.tasks[state.runs[ix].task].kind else {
                    panic!("'{loop_id}' is not planned as a Loop");
                };
                assert!(
                    !values.contains("{{"),
                    "spawn_round only serves loops whose `values` is a literal; \
                     '{loop_id}' holds {values:?}"
                );
                state.runs[ix].rendered = Some(values.clone());
                return (state.clone(), advance(plan, &state, T0));
            }
            let cmds = round(plan, &mut state);
            assert!(!cmds.is_empty(), "'{loop_id}' never got a run");
        }
        panic!("'{loop_id}' never got a run in 100 rounds");
    }

    /// **A `Loop` spawns one execution per value and creates no run for its
    /// body**, which is the plan's own Task 6 test.
    ///
    /// Measured, and it is the whole shape of the flowable: a `Loop` does not
    /// iterate inside its execution. Each value produces a separate
    /// **sub-execution** carrying `parentId`, and the body's task never appears
    /// in the parent's `taskRunList` — over the whole golden, 41 entries carry
    /// 142 sub-executions between them and not one loop body task has a run in
    /// its parent (`no_run_is_ever_created_for_a_task_inside_a_loop` derives
    /// that over the corpus; this pins the one flow).
    ///
    /// `valids/loop-parallel-less` is the flow because it is the one that can
    /// tell the two halves apart: **four** values with `concurrencyLimit: 2`,
    /// so a machine that spawned one per value would emit 4 and one that
    /// ignored the limit would emit 4 as well. Two is the number only the limit
    /// produces.
    #[test]
    fn a_loop_spawns_one_execution_per_value_and_no_child_run() {
        let plan = plan_of("valids/loop-parallel-less");
        let (_, cmds) = spawn_round(&plan, "loop");

        assert!(
            cmds.iter()
                .all(|c| !matches!(c, Command::CreateRun { task, .. } if plan.is_loop_body(*task))),
            "a loop body never gets a run in the parent execution: {cmds:?}"
        );
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(c, Command::SpawnExecution { .. }))
                .count(),
            2,
            "four values, `concurrencyLimit: 2` -- Kestra's own first batch is \
             `min(limit, size)` (Loop.java:422): {cmds:?}"
        );
    }

    /// **An empty `Loop` SUCCEEDS without ever running, and a `values` that
    /// cannot be resolved FAILS the same way.**
    ///
    /// Both measured on the artifact, and both are why a `Loop` is asked for
    /// its values at `CREATED` rather than at `RUNNING`. `valids/loop-empty`'s
    /// `1_each` reads `CREATED SUCCESS` with **no `RUNNING` and no attempt**,
    /// and the flow's next task runs; `valids/loop-null`'s reads `CREATED
    /// FAILED`, also with no attempt, and its next task is never created.
    ///
    /// A machine that started the loop and then noticed produces `CREATED
    /// RUNNING SUCCESS` with one attempt, which is a different trace on both --
    /// so the two histories, not just the terminal states, are what is asserted.
    /// These are the only two `Loop` entries in the whole golden whose
    /// sub-execution list is empty, and they are exactly the two Task 6 banks.
    #[test]
    fn an_empty_loop_succeeds_without_running_and_an_unresolvable_one_fails() {
        let plan = plan_of("valids/loop-empty");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(
            run_ids(&plan, &state),
            ["1_each", "2_end"],
            "an empty loop does not stop the sequence"
        );
        assert_eq!(
            (wire(&state.runs[0].history), state.runs[0].attempts.len()),
            (vec!["CREATED", "SUCCESS"], 0),
            "no RUNNING and no attempt: an empty loop never starts"
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "SUCCESS"]);

        let plan = plan_of("valids/loop-null");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(
            run_ids(&plan, &state),
            ["1_each"],
            "the failure stops the sequence, so `2_end` is never created"
        );
        assert_eq!(
            (wire(&state.runs[0].history), state.runs[0].attempts.len()),
            (vec!["CREATED", "FAILED"], 0),
            "a null inside the iteration values fails the loop before it starts"
        );
        assert_eq!(wire(&state.history), ["CREATED", "RUNNING", "FAILED"]);
    }

    /// **`concurrencyLimit` bounds the first batch: `0` is unlimited, `1` is
    /// full serialisation, `n` is up to `n`.**
    ///
    /// Four corpus flows, chosen because between them they pin every arm of
    /// `Loop.java:422` and the two directions a wrong reading goes: `0` read as
    /// a cap of zero spawns nothing and hangs, and a default read as `0` spawns
    /// everything. The values come from each flow's own `values:` via
    /// [`spawn_round`], so a flow whose list changes cannot silently agree.
    #[test]
    fn the_concurrency_limit_bounds_the_first_batch_and_zero_means_unlimited() {
        for (corpus_id, loop_id, values, expected) in [
            // no `concurrencyLimit:` -- Kestra's default of 1, three values
            ("valids/loop-serial", "loop", 3, 1),
            // `concurrencyLimit: 2` over four values
            ("valids/loop-parallel-less", "loop", 4, 2),
            // `concurrencyLimit: 5` over three -- the cap is above the size
            ("valids/loop-parallel-more", "loop", 3, 3),
            // `concurrencyLimit: 0` -- unlimited, NOT a cap of zero
            ("valids/loop-parallel-unlimited", "loop", 3, 3),
        ] {
            let plan = plan_of(corpus_id);
            let ix = plan
                .tasks
                .iter()
                .position(|t| t.id == loop_id)
                .expect("the loop is planned");
            let TaskKind::Loop { values: src, .. } = &plan.tasks[ix].kind else {
                panic!("{corpus_id}: `{loop_id}` is not a Loop");
            };
            assert_eq!(
                iteration_values(src).expect("literal values resolve").len(),
                values,
                "{corpus_id}: the flow's own value count"
            );

            let (_, cmds) = spawn_round(&plan, loop_id);
            let spawned: Vec<&str> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::SpawnExecution { value, .. } => Some(value.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                spawned.len(),
                expected,
                "{corpus_id}: {values} value(s), first batch of {expected}: {cmds:?}"
            );
            assert!(
                cmds.iter()
                    .any(|c| matches!(c, Command::StartRun { run, .. } if *run == 0)),
                "{corpus_id}: the loop starts in the same round it spawns"
            );
            assert!(
                !cmds
                    .iter()
                    .any(|c| matches!(c, Command::SetRunState { state, .. } if *state == RunState::Submitted)),
                "{corpus_id}: a Loop is a flowable and never passes through SUBMITTED"
            );
        }
    }

    /// **Kestra's `resolveValues`, row by row** — including the two rules a
    /// reader would not write.
    ///
    /// The shapes are the ones the corpus actually holds, plus the two
    /// boundaries nothing in the corpus reaches. `distinct()` is on the array
    /// arm and NOT on the map arm, which is `FlowableUtils.java:439` against
    /// `:463-468`, and a map iterates its VALUES rather than its keys or its
    /// pairs — measured on `valids/loop-map`, whose three sub-executions carry
    /// `"value 1"`, `"value 2"`, `"value 3"` and not `key1`, `key2`, `key3`.
    #[test]
    fn kestra_deduplicates_a_list_of_values_and_not_a_map_of_them() {
        let cases: &[(&str, Result<Vec<&str>, ()>)] = &[
            // corpus shapes
            (r#"["value 1", "value 2"]"#, Ok(vec!["value 1", "value 2"])),
            (r#"[1, 2, 3]"#, Ok(vec!["1", "2", "3"])),
            (r#"[]"#, Ok(vec![])),
            (
                r#"{"key1": "value 1", "key2": "value 2"}"#,
                Ok(vec!["value 1", "value 2"]),
            ),
            (
                r#"["value 1", {"key": "my-key", "value": "my-value"}]"#,
                Ok(vec!["value 1", r#"{"key":"my-key","value":"my-value"}"#]),
            ),
            // a nested object keeps INSERTION order, not alphabetical --
            // `valids/loop-object`'s golden value is
            // `{"key":"my-complex","value":{"sub":1,"bool":true}}`, and `bool`
            // sorts before `sub`.
            (
                r#"[{"key": "my-complex", "value": {"sub": 1, "bool": true}}]"#,
                Ok(vec![
                    r#"{"key":"my-complex","value":{"sub":1,"bool":true}}"#,
                ]),
            ),
            // the two rules
            (r#"["a", "b", "a"]"#, Ok(vec!["a", "b"])),
            (r#"{"k1": "a", "k2": "a"}"#, Ok(vec!["a", "a"])),
            // the refusals
            (r#"[1, null]"#, Err(())),
            (r#"{"k": null}"#, Err(())),
            (r#"{"k": true}"#, Err(())),
            (r#""just a string""#, Err(())),
            (r#"7"#, Err(())),
            (r#"not json at all"#, Err(())),
        ];
        for (source, expected) in cases {
            let got = iteration_values(source);
            match (expected, &got) {
                (Ok(want), Ok(have)) => assert_eq!(
                    have,
                    &want.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
                    "values {source}"
                ),
                (Err(()), Err(_)) => {}
                _ => panic!("values {source}: expected {expected:?}, got {got:?}"),
            }
        }
    }

    /// **An explicit `concurrencyLimit: null` is now REFUSED; a negative one
    /// still takes Kestra's class default of 1, and is still an
    /// OVER-ACCEPTANCE.**
    ///
    /// `Loop.java:220-233` is `@PositiveOrZero @NotNull @Builder.Default private
    /// Integer concurrencyLimit = 1;`, so this Kestra refuses both spellings at
    /// publish. This test used to record both as over-accepted with "Task 8 owns
    /// that" beside them. **Task 8 closed half of it**: `build_plan` now runs
    /// W1's `validate`, which carries `loop_concurrency_limit_required` for the
    /// `@NotNull`, so the `null` row is a refusal with a message.
    ///
    /// `@PositiveOrZero` has no rule in `validate`, so `-1` is still planned as
    /// 1 — and that row is now the ONLY thing in the crate that reaches the
    /// `_ => 1` fallback in `literal_concurrency_limit`, since the `null` path
    /// no longer gets past the gate. It is load-bearing for the mutation below,
    /// not decoration.
    ///
    /// What it must not do is pick silently. The mutation that changes the
    /// fallback from `1` to `0` — the difference between full serialisation and
    /// spawning every value at once — killed **nothing at all** on the first
    /// pass, because
    /// `every_corpus_loop_declares_its_values_and_a_readable_limit` proves no
    /// corpus flow writes either spelling and so nothing else in the crate can
    /// reach the arm. That is the "guard nobody has seen fail" pattern this
    /// package has already shipped three times; this test is what closes it.
    #[test]
    fn a_null_or_negative_concurrency_limit_takes_kestras_class_default() {
        let flow = |limit: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\", \"b\", \"c\"]'\n    concurrencyLimit: {limit}\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            )
        };
        // The `@NotNull` half, closed by Task 8. Asserted on the CODE, not on
        // "it errored": every other spelling below must still plan, so a
        // refusal that widened to catch them all would pass a bare `is_err`.
        let refused = build_plan(&flow("null"))
            .expect_err("`concurrencyLimit: null` is @NotNull in Kestra and must be refused");
        assert!(
            refused
                .message
                .starts_with("loop_concurrency_limit_required:"),
            "refused for the wrong reason: {refused}"
        );

        for (written, expected) in [("-1", 1u32), ("0", 0), ("2", 2)] {
            let plan = build_plan(&flow(written))
                .unwrap_or_else(|e| panic!("`concurrencyLimit: {written}` must plan: {e}"));
            let TaskKind::Loop {
                concurrency_limit, ..
            } = plan.tasks[0].kind
            else {
                panic!("`concurrencyLimit: {written}`: not planned as a Loop");
            };
            assert_eq!(
                concurrency_limit, expected,
                "`concurrencyLimit: {written}` must plan as {expected}"
            );

            // And the behaviour it decides, so the number is not merely stored:
            // three values, so the first batch separates 1 from 0.
            let (_, cmds) = spawn_round(&plan, "each");
            let spawns = cmds
                .iter()
                .filter(|c| matches!(c, Command::SpawnExecution { .. }))
                .count();
            assert_eq!(
                spawns,
                if expected == 0 { 3 } else { expected as usize },
                "`concurrencyLimit: {written}` over three values"
            );
        }
    }

    /// **A `Loop` with no `values:` cannot be planned**, the same shape and the
    /// same reason as an `If` with no `condition` — `@NotNull` in Java, and
    /// nothing to iterate here.
    ///
    /// Unlike that one, no corpus flow is this shape
    /// ([`every_corpus_loop_declares_its_values_and_a_readable_limit`] derives
    /// that), so this test is the whole of what holds the refusal. It prints the
    /// task count and root id rather than the `Plan`, for Task 3's M6 reason.
    #[test]
    fn a_loop_with_no_values_is_refused_by_name() {
        let source = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n";
        match build_plan(source) {
            Err(e) => assert_eq!(e.message, "loop 'each' has no values to iterate"),
            Ok(plan) => panic!(
                "a Loop with no values must be refused; got a plan of {} task(s) rooted at {:?}",
                plan.tasks.len(),
                plan.roots
                    .first()
                    .map(|&ix| plan.tasks[ix].id.as_str())
                    .unwrap_or("<none>")
            ),
        }

        // The control: the same flow with `values:` plans.
        let with_values = source.replace(
            "    type: io.kestra.plugin.core.flow.Loop\n",
            "    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\"]'\n",
        );
        assert!(
            build_plan(&with_values).is_ok(),
            "the control: only the missing `values:` may cause the refusal"
        );
    }

    /// **Every corpus `Loop` declares `values:` and a limit this builder can
    /// read**, derived from all 345 flows on every run.
    ///
    /// Three things it pins, each of which would otherwise be an assumption in
    /// [`build_plan`]: that the `values:` refusal is unreachable from the corpus
    /// (so the unit test above is what holds it), that no `concurrencyLimit:` is
    /// an expression or a negative number (so `literal_concurrency_limit`'s
    /// fallbacks are unreachable too), and how many loops there are to begin
    /// with.
    #[test]
    fn every_corpus_loop_declares_its_values_and_a_readable_limit() {
        // `(flow id, task id, values declared?, concurrencyLimit)` -- the two
        // fields `build_plan` reads, kept as scalars because `models::Loop` is
        // not exported from `models`.
        fn walk(task: &crate::models::Task, out: &mut Vec<(String, bool, Option<i32>)>) {
            if let crate::models::TaskKind::Loop(l) = &task.kind {
                out.push((task.id.clone(), l.values.is_some(), l.concurrency_limit));
            }
            for child in task.children() {
                walk(child, out);
            }
        }

        let mut loops: Vec<(String, String, bool, Option<i32>)> = Vec::new();
        for (id, source) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&source) else {
                continue;
            };
            let mut here = Vec::new();
            for task in flow.tasks.iter().chain(&flow.errors) {
                walk(task, &mut here);
            }
            for (task_id, has_values, limit) in here {
                loops.push((id.clone(), task_id, has_values, limit));
            }
        }

        let missing: Vec<&str> = loops
            .iter()
            .filter(|(_, _, has_values, _)| !has_values)
            .map(|(id, _, _, _)| id.as_str())
            .collect();
        assert_eq!(
            missing,
            Vec::<&str>::new(),
            "no corpus flow omits `values:`, so `a_loop_with_no_values_is_refused_by_name` \
             is the only thing holding that refusal"
        );

        let unreadable: Vec<&str> = loops
            .iter()
            .filter(|(_, _, _, limit)| !matches!(limit, Some(n) if *n >= 0))
            .map(|(id, _, _, _)| id.as_str())
            .collect();
        assert_eq!(
            unreadable,
            Vec::<&str>::new(),
            "every `concurrencyLimit:` in the corpus is a non-negative literal, so \
             `literal_concurrency_limit`'s fallback to the class default is never \
             reached by anything this oracle compares"
        );

        let non_default = loops
            .iter()
            .filter(|(_, _, _, limit)| *limit != Some(1))
            .count();
        assert_eq!(
            (loops.len(), non_default),
            (52, 14),
            "52 `Loop` tasks across the 345 corpus flows, of which 14 write a \
             `concurrencyLimit:` other than the default 1. Fifteen flows write \
             the property at all -- `valids/if-in-parallel` writes \
             `concurrencyLimit: 1`, which IS the default, and that is the \
             cross-check on the number"
        );
    }

    /// **"Inside a loop" means the whole subtree, and the two readings differ.**
    ///
    /// [`Plan::is_loop_body`] answers for a task anywhere under a `Loop`, not
    /// only for its direct children, because an `If` nested in a `Sequential` in
    /// a loop body is just as absent from the parent's `taskRunList`. Nothing
    /// forces that from the type: a narrower reading compiles and passes every
    /// other test in this crate, because `advance()` never consults the method
    /// at all.
    ///
    /// So the difference is measured. Over all 345 corpus flows the wide
    /// reading names **89** tasks and the narrow one **64**; the 25 between them
    /// are the ones a narrow reading would have called reachable, and the oracle
    /// would then have tried to render their expressions against a context with
    /// no `item`. (Restricted to the comparable entries the oracle actually
    /// drives, the wide count is 82 —
    /// `tests/executor.rs::no_run_is_ever_created_for_a_task_inside_a_loop`.)
    #[test]
    fn the_two_readings_of_inside_a_loop_differ_by_a_measured_amount() {
        let (mut wide, mut narrow) = (0usize, 0usize);
        for (id, source) in every_corpus_flow() {
            let Ok(plan) = build_plan(&source) else {
                continue;
            };
            let direct: Vec<TaskIx> = plan
                .tasks
                .iter()
                .filter_map(|t| match &t.kind {
                    TaskKind::Loop { tasks, .. } => Some(tasks.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            narrow += direct.len();
            let here = (0..plan.tasks.len())
                .filter(|&ix| plan.is_loop_body(ix))
                .count();
            wide += here;
            assert!(
                here >= direct.len(),
                "{id}: the wide reading must contain the narrow one"
            );
        }
        assert_eq!(
            (wide, narrow),
            (88, 63),
            "88 tasks are inside a loop by the shipped reading and 63 by the \
             direct-children one. The 25 between them are why this is a test: \
             `advance()` never calls `is_loop_body`, so nothing else in the crate \
             can tell the two apart. Both were one higher until the final \
             review implemented `disabled:`, which deletes \
             `valids/disable-flowable`'s `1-each-2` -- a direct child of a \
             `Loop` -- from the plan; the gap of 25 is unchanged, because the \
             task left both readings at once"
        );
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
        /// The model-side reading: `Task::children()`, which is
        /// `spec::validate`'s walk and excludes every task-level `errors:`.
        fn depth(task: &crate::models::Task) -> usize {
            1 + task.children().iter().map(|c| depth(c)).max().unwrap_or(0)
        }

        /// The **builder's own** reading: the depth of the tree `flatten_one`
        /// actually built, `errors:` subtrees included. `Plan::children_of` is
        /// `errors` + the flowable's children, which is exactly the set
        /// `flatten_one` recurses into at `depth + 1`, and the roots are at
        /// depth 1 — so this is the largest `depth` the recursion ever held.
        fn plan_depth(plan: &Plan, ix: TaskIx) -> usize {
            1 + plan
                .children_of(ix)
                .into_iter()
                .map(|c| plan_depth(plan, c))
                .max()
                .unwrap_or(0)
        }

        let mut deepest = 0usize;
        let mut deepest_id = String::new();
        let mut deepest_planned = 0usize;
        let mut deepest_planned_id = String::new();
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
                deepest_id = id.clone();
            }
            if let Ok(plan) = build_plan(&source) {
                let p = plan
                    .roots
                    .iter()
                    .chain(plan.errors.iter())
                    .map(|&ix| plan_depth(&plan, ix))
                    .max()
                    .unwrap_or(0);
                if p > deepest_planned {
                    deepest_planned = p;
                    deepest_planned_id = id;
                }
            }
        }

        println!(
            "deepest corpus flow: {deepest_id} nests {deepest} level(s) by \
             `Task::children()`; deepest PLAN: {deepest_planned_id} at \
             {deepest_planned} level(s) by the builder's own walk"
        );
        assert_eq!(
            deepest,
            super::plan::MEASURED_MAX_CORPUS_DEPTH,
            "plan.rs's MEASURED_MAX_CORPUS_DEPTH is stale: the deepest corpus flow is \
             now {deepest_id} at {deepest}"
        );
        assert_eq!(
            deepest_planned,
            super::plan::MEASURED_MAX_CORPUS_DEPTH,
            "the BUILDER's own walk is what MAX_TASK_DEPTH bounds, and since Task 7 \
             it is wider than `Task::children()`: `flatten_one` descends into a \
             task-level `errors:` and `children()` does not. Through Task 8 only \
             the model-side reading was derived, so the constant was measured \
             against a walk that is not the one it guards. The two agree at \
             {deepest_planned}; if they ever stop agreeing, this is the one to \
             believe -- the deepest PLAN is {deepest_planned_id}"
        );
    }

    /// A chain of `levels` nested tasks: `levels - 1` `Sequential`s with one
    /// `Return` at the bottom, so the deepest task sits at depth `levels`.
    fn nested_yaml(levels: usize) -> String {
        nested_yaml_spelled(levels, "io.kestra.plugin.core.flow.Sequential")
    }

    /// The same chain, with the `Sequential`s written in whichever of the four
    /// spellings W1 accepts the caller names. See
    /// `every_spelling_of_a_flowable_reaches_the_same_plan`.
    fn nested_yaml_spelled(levels: usize, sequential: &str) -> String {
        assert!(levels >= 1);
        let mut y = String::from("id: deep\nnamespace: io.kestra.tests\ntasks:\n");
        for level in 0..levels - 1 {
            let pad = "  ".repeat(level * 2 + 1);
            y.push_str(&format!(
                "{pad}- id: s{level}\n{pad}  type: {sequential}\n{pad}  tasks:\n"
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

    /// **No corpus `Dag` node is disabled** — the derivation behind the one
    /// position `flatten_list`'s `disabled:` rule deliberately does not reach.
    ///
    /// A `Dag`'s nodes go through `flatten_dag`, not `flatten_list`, so a
    /// disabled `Dag` node is still planned. That is a gap, and it is a gap on
    /// purpose: Kestra removes such a node from `currentTasks` like any other
    /// child (`findTaskDependingFlowState` -> `removeDisabled`) but leaves it in
    /// `taskDependencies` (`FlowableUtils.resolveDagNexts`), so every node that
    /// `dependsOn` it waits for a run that is never created and the `Dag` never
    /// terminates. Each of the three ways out is worse than the gap: copying it
    /// reproduces a hang, dropping the node *and* its incoming edges invents a
    /// semantics Kestra does not have, and dropping the node alone makes
    /// `flatten_dag` refuse the flow for a dangling `dependsOn` — an
    /// over-refusal, the direction that must not ship.
    ///
    /// So the question is left open, and this is what says the corpus never
    /// asks it. If a flow ever does, this fails and somebody has to decide
    /// rather than inherit the guess.
    #[test]
    fn no_corpus_dag_node_is_disabled() {
        // A `Dag`'s node is `{ task: {...}, dependsOn: [...] }`, so a disabled
        // node is a `disabled: true` on the object under a `task:` key that is
        // itself an element of a `Dag`'s `tasks:`. Walked over the raw YAML for
        // the same reason `every_corpus_errors_list_is_on_a_flowable_egret_plans`
        // is: a construct the model drops would score zero by construction.
        fn walk(node: &serde_json::Value, out: &mut Vec<String>) {
            if let serde_json::Value::Object(map) = node {
                let is_dag = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .and_then(crate::models::canonical_flow_type)
                    .is_some_and(|c| c == "dag");
                if is_dag {
                    for entry in map
                        .get("tasks")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                    {
                        let disabled = entry
                            .get("task")
                            .and_then(|t| t.get("disabled"))
                            .and_then(|d| d.as_bool())
                            .unwrap_or(false);
                        if disabled {
                            out.push(
                                entry["task"]["id"]
                                    .as_str()
                                    .unwrap_or("<no id>")
                                    .to_string(),
                            );
                        }
                    }
                }
            }
            match node {
                serde_json::Value::Object(map) => {
                    for v in map.values() {
                        walk(v, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for v in items {
                        walk(v, out);
                    }
                }
                _ => {}
            }
        }

        // The positive control, first: this walker must be able to SEE a
        // disabled `Dag` node, or the empty result below is a property of the
        // walker rather than of the corpus. Written in the alias spelling too,
        // since `canonical_flow_type` is what decides whether a node is a
        // `Dag`'s.
        for ty in ["io.kestra.plugin.core.flow.Dag", "core.dag"] {
            let probe: serde_json::Value = serde_norway::from_str(&format!(
                "id: probe\nnamespace: io.kestra.tests\ntasks:\n  - id: d\n    type: {ty}\n    \
                 tasks:\n      - task:\n          id: off\n          type: io.kestra.plugin.core.debug.Return\n          \
                 format: x\n          disabled: true\n      - task:\n          id: on\n          \
                 type: io.kestra.plugin.core.debug.Return\n          format: x\n"
            ))
            .expect("the probe parses");
            let mut found = Vec::new();
            walk(&probe, &mut found);
            assert_eq!(
                found,
                ["off".to_string()],
                "{ty}: the walker must find a disabled Dag node and must not \
                 report the enabled one"
            );
        }

        let mut dags = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        for (id, source) in every_corpus_flow() {
            let Ok(doc) = serde_norway::from_str::<serde_json::Value>(&source) else {
                continue;
            };
            if source.contains(".flow.Dag") || source.contains("core.dag") {
                dags += 1;
            }
            let mut here = Vec::new();
            walk(&doc, &mut here);
            offenders.extend(here.into_iter().map(|node| format!("{id}/{node}")));
        }

        println!("corpus flows declaring a Dag: {dags}");
        assert!(
            dags > 0,
            "the control: if no corpus flow declares a `Dag` at all, this test \
             proves nothing about `Dag` nodes"
        );
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "a corpus `Dag` node is disabled, so the gap `flatten_list`'s doc \
             names is now reachable. `flatten_dag` still plans it, which is an \
             over-acceptance; Kestra would hang on it. Somebody has to decide \
             what egret does, and the three obvious answers are each worse than \
             the gap -- see `flatten_list`"
        );
    }

    /// **Every spelling W1 accepts for a flowable reaches the same plan** —
    /// the W1/W3a dispatch seam, asserted rather than assumed.
    ///
    /// W1's `models::canonical_flow_type` reaches `ModelKind::Sequential` from
    /// **four** spellings: the FQCN, the `core.` short form, and either of them
    /// in any case, because it lowercases. `flatten_one` used to match the
    /// parsed variant against the FQCN *literal*, and carried a comment saying
    /// "`models::TaskKind::Sequential` is reached by that type string and by no
    /// other". That was false the moment W1 gained the short form, and the
    /// price is not a cosmetic one: a `core.sequential` planned as an opaque
    /// `Runnable` has no children in the plan at all, so a chain of them is
    /// **one** task however deep it is written and [`MAX_TASK_DEPTH`] never
    /// fires.
    ///
    /// Measured at the fix's parent commit, on a two-level chain:
    ///
    /// ```text
    /// io.kestra.plugin.core.flow.Sequential -> 2 tasks, Sequential { tasks: [1] }
    /// core.sequential                       -> 1 task,  Runnable { type_id: "core.sequential" }
    /// core.Sequential                       -> 1 task,  Runnable { .. }
    /// io.kestra.plugin.core.flow.sequential -> 1 task,  Runnable { .. }
    /// ```
    ///
    /// It was latent rather than live — no corpus flow writes an alias, which
    /// is why every ratchet entry was green through it — and latent is the
    /// only reason this is a test and not a bank.
    ///
    /// The second half is the one that matters, and it is the one a
    /// shape-equality assertion alone would miss: the recursion bound must fire
    /// on the alias too. A guard that only fires on one spelling of the input
    /// it guards is not a bound.
    #[test]
    fn every_spelling_of_a_flowable_reaches_the_same_plan() {
        const SPELLINGS: &[&str] = &[
            "io.kestra.plugin.core.flow.Sequential",
            "core.sequential",
            "core.Sequential",
            "io.kestra.plugin.core.flow.sequential",
        ];

        let canonical =
            build_plan(&nested_yaml_spelled(2, SPELLINGS[0])).expect("the FQCN spelling plans");
        assert_eq!(canonical.tasks.len(), 2, "the control: two tasks, nested");
        assert!(
            matches!(canonical.tasks[0].kind, TaskKind::Sequential { .. }),
            "the control must be a Sequential, or the rows below prove nothing"
        );

        for spelling in SPELLINGS {
            let plan = build_plan(&nested_yaml_spelled(2, spelling))
                .unwrap_or_else(|e| panic!("{spelling} must plan: {e}"));
            assert_eq!(
                plan.tasks.len(),
                canonical.tasks.len(),
                "{spelling}: W1 parses this to the same `ModelKind` as the FQCN,                  so it must reach the same plan. A different task count means                  the builder fell through to an opaque `Runnable` and the whole                  subtree is missing"
            );
            assert_eq!(
                plan.tasks[0].kind, canonical.tasks[0].kind,
                "{spelling}: same variant, not merely the same size"
            );

            // And the bound fires on it. This is the half that is not implied
            // by the shape assertions: an opaque `Runnable` chain is one task
            // deep whatever it says on the page.
            let over = super::plan::MAX_TASK_DEPTH + 1;
            let refused = build_plan(&nested_yaml_spelled(over, spelling));
            assert!(
                refused.is_err(),
                "{spelling}: nesting {over} levels deep must be refused; got a                  plan of {} task(s)",
                refused.as_ref().map(|p| p.tasks.len()).unwrap_or(0)
            );
            let err = refused.unwrap_err();
            assert!(
                err.message.contains("nests deeper"),
                "{spelling}: refused for the wrong reason: {err}"
            );
        }
    }

    /// **Only a `Sequential` is planned as a `Sequential`; every other
    /// flowable is opaque and its children are not flattened at all.**
    ///
    /// "A `Sequential`" and not "`io.kestra.plugin.core.flow.Sequential`":
    /// since the final review the builder dispatches through
    /// `canonical_flow_type`, so all four spellings of it reach this arm. That
    /// is `every_spelling_of_a_flowable_reaches_the_same_plan`'s subject; what
    /// this test is about is the OTHER direction, which the change does not
    /// touch — a type that canonicalises to something else stays opaque.
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
        // `condition:` is written for every row, not only for the two that
        // need it. `LoopUntil`/`WaitFor` declare it `@NotNull`
        // (`LoopUntil.java`), so since Task 8 put W1's `validate` behind
        // `build_plan` a probe flow without one is REFUSED -- correctly, and
        // for a reason that has nothing to do with opacity. A probe that
        // cannot be planned cannot say whether its children were flattened,
        // and the four rows are kept byte-identical so that no row is a
        // different experiment from the others. `AllowFailure` and
        // `WorkingDirectory` ignore the extra key (it lands in their opaque
        // remainder), which the `Sequential` control below confirms.
        let branch = |ty: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: {ty}\n    condition: \"{{{{ true }}}}\"\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            )
        };

        for ty in [
            "io.kestra.plugin.core.flow.AllowFailure",
            "io.kestra.plugin.core.flow.WorkingDirectory",
            "io.kestra.plugin.core.flow.WaitFor",
            "io.kestra.plugin.core.flow.LoopUntil",
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

        // Task 6's, and the one whose children are flattened and then never
        // run: a `Loop` body gets a `PlanTask` so that W3b can execute the
        // sub-execution, and no `CreateRun` in THIS execution ever names it.
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\"]'\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Loop {
                values: "[\"a\"]".to_string(),
                tasks: vec![1],
                concurrency_limit: 1
            },
            "a Loop flattens its body and takes Kestra's default limit of 1, as \
             of Task 6"
        );
        assert!(
            plan.is_loop_body(1) && !plan.is_loop_body(0),
            "the body is inside the loop and the loop is not inside itself"
        );

        // Task 5's two. Both flatten, through the same `canonical_flow_type`
        // dispatch the three above use -- which reaches each of them from four
        // spellings, not from the FQCN alone. This comment said "the FQCN
        // only" and was true of the builder until the final review.
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{ inputs.p }}\"\n    then:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::If {
                condition: "{{ inputs.p }}".to_string(),
                then_: vec![1],
                else_: vec![]
            },
            "an If flattens both branches, as of Task 5"
        );
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"{{ inputs.p }}\"\n    cases:\n      A:\n        - id: inner\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
        )
        .expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Switch {
                value: "{{ inputs.p }}".to_string(),
                cases: vec![("A".to_string(), vec![1])],
                default: vec![]
            },
            "and so does a Switch"
        );
    }

    /// **The over-refusal that reached the HTTP API, and the record of its
    /// absence.**
    ///
    /// `build_plan` used to refuse `valids/switch-impossible` — a flow this
    /// pinned Kestra publishes and **runs** — and the cause was not in the
    /// executor. W1's `models::Switch::defaults` was a bare `Vec<Task>`, so an
    /// explicit `defaults: null` was a parse error; Jackson does not error on
    /// one, it leaves the field null and Kestra runs the flow. `models/task.rs`
    /// applied the bean-validation-is-not-parsing principle correctly to
    /// `Parallel::concurrent` and every other scalar, and missed it on every
    /// bare list.
    ///
    /// It was never an oracle curiosity: `spec::parse` is what `routes.rs`'s
    /// `create_flow`, `update_flow` and `validate_flow` call, so egret's HTTP
    /// API returned 400 for a flow this Kestra accepts. Over-refusal is the
    /// mirror of the divergence class this programme forbids shipping.
    ///
    /// `wire::null_is_absent`, on every list field in `models/task.rs`, closes
    /// it. This test is its predecessor inverted: what that one *rehearsed* by
    /// deleting the offending token from each of the three flows, this one
    /// asserts with the token in place.
    ///
    /// **The two accidental refusals are the reason this is a test and not a
    /// one-line diff.** `invalids/switch-invalid` and `invalids/invalid-parallel`
    /// were refused BY the null rule, so their refusals said nothing about what
    /// Kestra judged them for, and a fix that accepted the token could have
    /// accepted the flows with it. It does not, and the reason is the same
    /// principle: `@NotEmpty` on `AbstractBranch.tasks` is bean validation,
    /// evaluated when Kestra *validates* a flow rather than when Jackson
    /// *parses* one, and `spec::validate` — which `build_plan` runs as its third
    /// gate — is egret's equivalent. Both are still refused, each for the cause
    /// Kestra itself names, and that is asserted on egret's own code rather than
    /// on `is_err()`: every one of these flows is wrong in more than one way, so
    /// a bare `expect_err` would pass for the wrong reason.
    #[test]
    fn a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own() {
        // The flow that was refused, with the token that refused it still in it.
        let with_null = corpus_source("valids/switch-impossible");
        assert!(
            with_null.contains("defaults: null"),
            "this test is about an explicit `null` in the corpus source; the flow \
             no longer carries one, so nothing below is asserting anything"
        );
        let planned = build_plan(&with_null)
            .expect("a flow this Kestra published with 200 and RAN must plan");

        // `null` and "key absent" are now the same input, which is Jackson's
        // behaviour and the whole content of the fix — asserted as an equality
        // rather than as two `is_ok()`s, so a fix that merely stopped erroring
        // while producing something else would fail here.
        assert_eq!(
            planned,
            build_plan(&with_null.replace("defaults: null", "")).expect("without the null"),
            "an explicit `null` list must parse to exactly what an absent key \
             parses to"
        );

        // `invalids/switch-invalid` used to be refused by the same token, so its
        // refusal said nothing about its empty `cases`. It is refused by
        // `switch_no_branches` now — "No task defined, neither cases or default
        // have any tasks" is Kestra's own wording for this flow.
        let si = build_plan(&corpus_source("invalids/switch-invalid"))
            .expect_err("its empty `cases:` is a refusal in its own right");
        assert!(
            si.message.starts_with("switch_no_branches:"),
            "with the null accepted, invalids/switch-invalid must be refused for \
             what Kestra judged about it, not for something else: {si}"
        );

        // And `invalids/invalid-parallel`, whose `para4` carried `tasks: null`
        // while `para2` (no `tasks` key) and `para3` (`tasks: []`) always
        // parsed: the parse rule was exactly "explicit null", and what catches
        // all three now is `parallel_tasks_required`, which is
        // `AbstractBranch`'s `@NotEmpty` and again Kestra's own reason.
        let ip = build_plan(&corpus_source("invalids/invalid-parallel"))
            .expect_err("para2, para3 and para4 are empty flowables");
        assert!(
            ip.message.starts_with("invalid_namespace:")
                || ip.message.starts_with("parallel_tasks_required:"),
            "with the null accepted, invalids/invalid-parallel must be refused \
             for one of the things Kestra named (its namespace, its id, or the \
             three empty `Parallel`s): {ip}"
        );
    }

    /// A flow this pinned Kestra published with **200** and `build_plan`
    /// refuses anyway: an OVER-REFUSAL, with the cause and the code that says
    /// egret refuses it for *that* cause.
    ///
    /// Written as an exception table rather than as a suppression list, the
    /// same way `STILL_OVER_ACCEPTED` is: each row is asserted to be **still
    /// refused**, so fixing one fails this test by name and forces the record
    /// to be corrected rather than letting the row rot.
    struct PublishedButRefused {
        id: &'static str,
        /// Why egret refuses it, and whose rule that is.
        cause: &'static str,
        /// egret's own codes. Any one of them must open the message, so a
        /// refusal that becomes a *different* refusal is a failure and not a
        /// silent pass.
        codes: &'static [&'static str],
    }

    /// **The two, and the sweep below asserts there is no third.**
    ///
    /// There were three. `valids/switch-impossible` was the first row and the
    /// only one of them with a golden entry — refused over a single
    /// `defaults: null` by a bare `Vec<Task>` in W1's model, which is now
    /// `wire::null_is_absent`. Its row is deleted here rather than edited, and
    /// the sweep below is what forced the deletion: it panicked by name the
    /// moment the flow started planning. See
    /// `a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own`.
    const PUBLISHED_BUT_REFUSED: &[PublishedButRefused] = &[
        PublishedButRefused {
            id: "valids/task-flow-dynamic",
            cause: "`namespace: \"{{inputs.namespace}}\"` on a `Subflow`.                     `subflow_namespace_invalid` checks the grammar against the                     UNRENDERED source, so every dynamic subflow reference is                     refused. The check exists because `publish::subflow_ref_name`                     encodes the pair as `\"{namespace}/{flowId}\"` and a `/` in                     either half decodes back into a different pair -- skipping it                     when the value holds `{{` would move the collision to render                     time, where nothing looks for it. The fix is to the encoding,                     and it is W1's",
            codes: &["subflow_namespace_invalid", "subflow_flow_id_invalid"],
        },
        PublishedButRefused {
            id: "valids/trigger-flow-listener-namespace-condition",
            cause: "a `Flow` trigger `dependsOn:` entry carrying only a                     `namespace:`, and another carrying only `states:`. At                     checkout `96893b589`, `Flow.java:364-380`'s `Dependency`                     declares `namespace`, `flowId`, `states` and `labels` as                     plain `@PluginProperty` fields with NO `@NotNull` on any of                     them -- a namespace-only dependency is a documented Kestra                     pattern. So these two codes are a `@NotNull` egret invented,                     for the same encoding reason as the row above. W1's",
            codes: &[
                "dependency_flow_id_required",
                "dependency_namespace_required",
            ],
        },
    ];

    /// **Every flow this Kestra published with 200 must plan, except two
    /// named over-refusals — swept over the whole corpus, not pinned by
    /// name.**
    ///
    /// Over-refusal is the mirror of the divergence class the programme
    /// forbids shipping, and until the final review nothing in this package
    /// swept for it. The two `validate` brought in at Task 8 were pinned *by
    /// name*, which guards those two and nothing else: they live in the 26
    /// `not-standalone` flows, the population the coverage statement says is
    /// never compared, so the ratchet could not have found them and cannot
    /// find a third.
    ///
    /// **Measured, not argued.** An invented `validate` rule refusing three
    /// more flows Kestra published with 200 (`kv`, `pause_no_tasks`,
    /// `trigger-flow-listener`, all `not-standalone`) left this crate green at
    /// 536 passed. That is the gap this test closes.
    ///
    /// The population is derived from `corpus.json`'s own `publish_status`,
    /// not from the `invalids/` directory name and not from the golden:
    /// **295** of the 345 flows. That is deliberately wider than the golden's
    /// 266 `runs` entries — it includes every `not-standalone` flow, which is
    /// exactly where the two known over-refusals hide, and one flow
    /// (`invalids/inputs-with-multiple-constraint-violations`) that published
    /// with 200 and was refused later, when the execution's *input values*
    /// were read. That one must still plan: `build_plan(&str)` never sees an
    /// input value, so refusing it here would be refusing it for a reason
    /// egret cannot have.
    ///
    /// The exception table is inverted the way `STILL_OVER_ACCEPTED` is: each
    /// row is asserted to be **still refused, for its own code**. A suppression
    /// list would let a fixed row rot; this one fails when a row is fixed and
    /// says which — which is not a claim about how it was written but a
    /// measurement: the table was three rows until the null-list fix landed, and
    /// this test is what refused to let the third one be deleted quietly. It
    /// panicked at `valids/switch-impossible` by name, with the row's own cause
    /// in the message, before anything had been edited.
    #[test]
    fn every_flow_kestra_published_plans_except_two_named_over_refusals() {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../../oracle-exec/corpus.json"))
                .expect("corpus.json parses");
        let entries = corpus.as_array().expect("corpus.json is an array");

        let published: BTreeSet<String> = entries
            .iter()
            .filter(|e| e["publish_status"].as_u64() == Some(200))
            .map(|e| {
                e["id"]
                    .as_str()
                    .expect("every corpus entry has an id")
                    .to_string()
            })
            .collect();
        assert_eq!(
            (published.len(), entries.len()),
            (295, 345),
            "the population, derived from corpus.json's own `publish_status`. \
             If it moves, the corpus or the harvest has changed and this \
             sweep's exception table must be re-measured"
        );

        let expected: BTreeSet<String> = PUBLISHED_BUT_REFUSED
            .iter()
            .map(|r| r.id.to_string())
            .collect();

        let mut refused: BTreeMap<String, String> = BTreeMap::new();
        for id in &published {
            if let Err(err) = build_plan(&corpus_source(id)) {
                refused.insert(id.clone(), err.message);
            }
        }

        let found: BTreeSet<String> = refused.keys().cloned().collect();
        let extra: Vec<&String> = found.difference(&expected).collect();
        assert!(
            extra.is_empty(),
            "OVER-REFUSAL: {} flow(s) this Kestra published with 200 are refused \
             by `build_plan` and are not in PUBLISHED_BUT_REFUSED. Over-refusal \
             is the mirror of the class this programme forbids shipping; fix the \
             rule, or add the row with its cause and its owner:\n{}",
            extra.len(),
            extra
                .iter()
                .map(|id| format!("  {id}: {}", refused[*id]))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // The other direction: every named row must still BE refused, and for
        // its own code. This is what stops the table becoming a list of things
        // that used to be true.
        for row in PUBLISHED_BUT_REFUSED {
            assert!(
                published.contains(row.id),
                "{}: PUBLISHED_BUT_REFUSED names a flow this Kestra did NOT \
                 publish with 200, so the row is about nothing",
                row.id
            );
            let Some(message) = refused.get(row.id) else {
                panic!(
                    "{}: this Kestra published it with 200 and it now PLANS -- \
                     one of {:?} has been fixed. Delete the row (and if this \
                     leaves the table empty, say so in oracle-exec/README.md's \
                     coverage statement, which quotes the count). Its cause was: \
                     {}",
                    row.id, row.codes, row.cause
                )
            };
            assert!(
                row.codes.iter().any(|c| message.starts_with(c)),
                "{} is refused, but not by the rule this row is about ({:?}): \
                 {message}",
                row.id,
                row.codes
            );
        }

        println!(
            "over-refusal sweep: {} flow(s) published with 200, {} refused, all \
             {} named",
            published.len(),
            refused.len(),
            PUBLISHED_BUT_REFUSED.len()
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

    // ----------------------------------------------------------------------
    // Task 7: retry, and errors:
    // ----------------------------------------------------------------------

    /// [`run_to_fixpoint`] with the oracle's clock tier: when nothing else can
    /// happen, `now` jumps to the earliest `not_before` still ahead of it.
    ///
    /// Time moves only when nothing else can, and only to a deadline something
    /// is already waiting for — the same rule `tests/executor.rs` tier 4 uses,
    /// written here too because a `retry:` is the first thing in this crate for
    /// which a lib test needs one at all. `outcome` is per attempt: a
    /// `Vec<RunState>` indexed by attempt number, with the last value repeated,
    /// so a run that succeeds on its fourth try is expressible.
    fn run_to_fixpoint_over_time(plan: &Plan, outcomes: &[RunState]) -> (ExecutionState, Tick) {
        let mut state = ExecutionState::new(plan);
        let mut now = Tick(0);
        for _ in 0..1000 {
            let cmds = advance(plan, &state, now);
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
            if let Some(ix) = in_flight {
                let attempt = state.runs[ix].attempts.len().saturating_sub(1);
                let outcome = *outcomes
                    .get(attempt)
                    .or(outcomes.last())
                    .expect("an outcome");
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                );
                continue;
            }
            match state
                .runs
                .iter()
                .filter_map(|r| r.not_before)
                .filter(|t| *t > now)
                .min()
            {
                Some(t) => now = t,
                None => return (state, now),
            }
        }
        panic!("did not settle in 1000 rounds");
    }

    /// The plan's own Task 7 test.
    ///
    /// Measured on `valids/retry-failed-task-attempts` (`maxAttempts: 4`,
    /// `interval: PT1S`): the execution's own history reads
    /// `CREATED RUNNING FAILED RETRYING RUNNING FAILED RETRYING RUNNING FAILED
    /// RETRYING RUNNING FAILED`, the run's reads the same four attempts
    /// separated by `RETRYING CREATED SUBMITTED`, and each of the four attempts
    /// reads exactly `CREATED RUNNING FAILED`.
    ///
    /// # The double entry, decided on purpose
    ///
    /// The spec recorded that Kestra's record ALSO showed the retried task as
    /// **two `taskRunList` entries**, and asked Task 7 to determine whether that
    /// is a semantic fact or a serialisation artefact before implementing.
    /// **It is neither: it does not reproduce.** Measured two ways —
    ///
    /// - over `oracle-exec/traces.json`, all 2050 captured execution records
    ///   (268 flows × 5 rounds, plus every sub-execution) holding 4215
    ///   taskruns: **zero** have two entries with the same taskrun id, and zero
    ///   have two with the same `(taskId, value, parentTaskRunId)`.
    ///   `no_captured_execution_has_a_duplicate_taskrun` re-derives that on
    ///   every run;
    /// - from the artifact, the path is closed by construction:
    ///   `ExecutorService.java:611` marks the run `RETRYING` in place, and
    ///   `ExecutionService.retryTask` (`ExecutionService.java:135`) installs the
    ///   new attempt through `Execution.withTaskRun`, whose body is
    ///   `Collections.replaceAll(newTaskRunList, findTaskRunByTaskRunId(id),
    ///   taskRun)` followed by `throw new IllegalStateException` when the
    ///   replace finds nothing. It replaces or it throws; it has no append.
    ///
    /// So egret reproduces one entry with several attempts, which is what the
    /// spec asked for as the *semantics* — and this comment records that the
    /// measured record agrees rather than that the shapes were traded off.
    #[test]
    fn a_retried_task_has_two_attempts_and_the_execution_passes_through_retrying() {
        let plan = plan_of("valids/retry-failed-task-attempts");
        let (state, now) = run_to_fixpoint_over_time(&plan, &[RunState::Failed]);

        assert_eq!(run_ids(&plan, &state), ["fail"]);
        assert_eq!(
            state.runs[0].attempts.len(),
            4,
            "`maxAttempts: 4` is four attempts in total, not four RETRIES: \
             Kestra's guard is `attemptNumber() >= maxAttempts`"
        );
        for (i, attempt) in state.runs[0].attempts.iter().enumerate() {
            assert_eq!(
                wire(&attempt.history),
                ["CREATED", "RUNNING", "FAILED"],
                "attempt {i} must not carry the RETRYING that follows it"
            );
        }
        assert_eq!(
            wire(&state.runs[0].history),
            [
                "CREATED",
                "SUBMITTED",
                "RUNNING",
                "FAILED",
                "RETRYING", //
                "CREATED",
                "SUBMITTED",
                "RUNNING",
                "FAILED",
                "RETRYING", //
                "CREATED",
                "SUBMITTED",
                "RUNNING",
                "FAILED",
                "RETRYING", //
                "CREATED",
                "SUBMITTED",
                "RUNNING",
                "FAILED",
            ]
        );
        assert_eq!(
            wire(&state.history),
            [
                "CREATED", "RUNNING", //
                "FAILED", "RETRYING", "RUNNING", //
                "FAILED", "RETRYING", "RUNNING", //
                "FAILED", "RETRYING", "RUNNING", //
                "FAILED",
            ],
            "the execution takes FAILED on every round a retry is pending, because \
             `handleEnd`'s `isTerminated(currentTasks)` is 0 == 0 for the empty \
             list `findTaskDependingFlowState` returns while a retry is pending"
        );
        assert_eq!(
            now,
            Tick(3_000),
            "three retries at `interval: PT1S`, and the clock moved only to the \
             deadlines the retries asked for"
        );
    }

    /// The retried run appears ONCE, and Kestra's own record does too.
    ///
    /// Derived from every captured execution rather than from the eight
    /// `retry-*` flows, because the claim under test is about the record shape
    /// and not about retry: if any flow in the corpus could produce a duplicate
    /// `taskRunList` entry, this is what would find it.
    #[test]
    fn no_captured_execution_has_a_duplicate_taskrun() {
        let raw = std::fs::read_to_string(format!(
            "{}/oracle-exec/traces.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("traces.json");
        let traces: serde_json::Value = serde_json::from_str(&raw).expect("traces.json is JSON");
        let flows = traces["flows"].as_object().expect("flows is an object");

        let (mut docs, mut runs, mut dup_id, mut dup_key) = (0usize, 0usize, 0usize, 0usize);
        // **The zero has to be non-vacuous.** "No taskrun appears twice" is
        // trivially true of a corpus in which nothing ever retried, so the
        // retried population is counted beside it: a run with two or more
        // attempts is a run Kestra retried, and it is those runs the spec's
        // observation was about.
        let (mut retried_runs, mut retried_flows) = (0usize, BTreeSet::new());
        for (flow_id, rounds) in flows {
            for round in rounds.as_array().into_iter().flatten() {
                let mut every: Vec<&serde_json::Value> = Vec::new();
                if round["root"].is_object() {
                    every.push(&round["root"]);
                }
                every.extend(round["descendants"].as_array().into_iter().flatten());
                for doc in every {
                    docs += 1;
                    let list = doc["runs"].as_array().expect("runs is an array");
                    runs += list.len();
                    let ids: BTreeSet<&str> =
                        list.iter().filter_map(|r| r["id"].as_str()).collect();
                    if ids.len() != list.len() {
                        dup_id += 1;
                    }
                    let keys: BTreeSet<String> = list
                        .iter()
                        .map(|r| format!("{}|{}|{}", r["taskId"], r["value"], r["parentTaskRunId"]))
                        .collect();
                    if keys.len() != list.len() {
                        dup_key += 1;
                    }
                    for run in list {
                        if run["attempts"].as_array().is_some_and(|a| a.len() >= 2) {
                            retried_runs += 1;
                            retried_flows.insert(flow_id.clone());
                        }
                    }
                }
            }
        }

        assert_eq!(
            (docs, runs),
            (2050, 4215),
            "the captured population: 2050 execution records holding 4215 \
             taskruns. 4215 is the same number `oracle-exec/README.md` quotes \
             for the happens-before endpoint measurement, by a different route"
        );
        assert_eq!(
            (retried_runs, retried_flows.len()),
            (105, 19),
            "the zero below is about RETRIED runs, so the population that makes \
             it non-vacuous is asserted first: 105 runs across 19 flows carry \
             two or more attempts. Without this, `(0, 0)` would also be the \
             answer for a corpus in which nothing ever retried"
        );
        assert_eq!(
            (dup_id, dup_key),
            (0, 0),
            "a retried task appears ONCE in Kestra's `taskRunList` on this \
             artifact, with several attempts. The spec recorded a double entry \
             and asked Task 7 to explain it; the answer is that it does not \
             reproduce -- and `Execution.withTaskRun` replaces or throws, so \
             there is no append path that could produce one"
        );
    }

    /// `maxAttempts` and `maxDuration` are two independent bounds, and either
    /// one alone stops the retry.
    ///
    /// `valids/retry-failed-flow-duration` is the corpus flow with **no**
    /// `maxAttempts` at all — `maxDuration: PT7S`, `interval: PT2S` — which is
    /// why [`RetryPolicy::max_attempts`] is an `Option` and not a `u32` with a
    /// zero default. Kestra's bound is `nextDate.isAfter(first attempt start +
    /// maxDuration)`, strictly after, measured from the FIRST attempt.
    #[test]
    fn a_retry_is_bounded_by_max_attempts_and_separately_by_max_duration() {
        let by_attempts = plan_of("valids/retry-failed-task-attempts");
        let (state, _) = run_to_fixpoint_over_time(&by_attempts, &[RunState::Failed]);
        assert_eq!(state.runs[0].attempts.len(), 4);
        assert_eq!(
            by_attempts.tasks[0]
                .retry
                .as_ref()
                .map(|r| (r.max_attempts, r.max_duration)),
            Some((Some(4), None))
        );

        let by_duration = plan_of("valids/retry-failed-flow-duration");
        assert_eq!(
            by_duration
                .retry
                .as_ref()
                .map(|r| (r.max_attempts, r.max_duration)),
            Some((None, Some(std::time::Duration::from_secs(7)))),
            "a flow-level retry with no `maxAttempts` is bounded only by \
             `maxDuration`; a `0` default would stop it before its first retry"
        );
        let (state, now) = run_to_fixpoint_over_time(&by_duration, &[RunState::Failed]);
        // 0, 2000, 4000, 6000 -- and the fifth would be 8000, which IS after
        // 0 + 7000, so the policy is exhausted. Kestra recorded THREE attempts
        // for this flow rather than four, and the difference is the hole
        // `oracle-exec/README.md` names: every attempt here costs zero elapsed
        // time, so more of them fit inside the same window. That is why this
        // entry stays baselined and is asserted here rather than banked.
        assert_eq!((state.runs[0].attempts.len(), now), (4, Tick(6_000)));
    }

    /// A flowable is never retried; its child is, on the flowable's policy.
    ///
    /// `ExecutorService.java:572` gates the whole retry pass on `task instanceof
    /// RunnableTask<?> || task instanceof Subflow`, and
    /// `searchForParentTaskWithRetry` is what gives the child a policy it does
    /// not own. `valids/retry-flowable` writes `retry:` on a `Sequential` and
    /// Kestra's golden has the `Sequential` with **one** attempt and its child
    /// with three.
    #[test]
    fn a_flowable_is_never_retried_and_its_child_inherits_the_policy() {
        let plan = plan_of("valids/retry-flowable");
        assert!(
            plan.tasks[0].retry.is_some() && plan.tasks[1].retry.is_none(),
            "the policy is written on the Sequential, not on the child"
        );
        let (state, _) = run_to_fixpoint_over_time(&plan, &[RunState::Failed]);
        assert_eq!(run_ids(&plan, &state), ["flowable", "child"]);
        assert_eq!(
            (state.runs[0].attempts.len(), state.runs[1].attempts.len()),
            (1, 3)
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED"]
        );
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "RETRYING", "RUNNING", "RETRYING", "RUNNING", "FAILED"],
            "no FAILED before the last one: the root list's only task is the \
             Sequential, which never terminated while its child was being \
             retried, so `handleEnd` never fired"
        );
    }

    /// `errors:` runs on the failure path, and the list's own state is the
    /// `tasks:` list's — not the `errors:` list's.
    ///
    /// Both halves are measured on `valids/errors`, whose two error tasks both
    /// SUCCEED and whose execution ends **FAILED**. Java passes `tasks` to
    /// `guessFinalState` even when `currentTasks` is `errors`
    /// (`FlowableUtils.resolveState`, `ExecutorService.onEnd`), and an
    /// aggregate over the driven list would get every `errors:` entry in the
    /// corpus exactly backwards.
    #[test]
    fn an_error_list_runs_on_failure_and_the_state_stays_the_task_lists_own() {
        let plan = plan_of("valids/errors");
        let mut state = ExecutionState::new(&plan);
        // The first root task fails; every error task then succeeds.
        for _ in 0..200 {
            let cmds = advance(&plan, &state, T0);
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
            let Some(ix) = in_flight else { break };
            let outcome = if plan.tasks[state.runs[ix].task].id == "failed" {
                RunState::Failed
            } else {
                RunState::Success
            };
            apply(
                &mut state,
                &Command::SetRunState {
                    run: ix,
                    state: outcome,
                },
            );
        }
        assert_eq!(
            run_ids(&plan, &state),
            [
                "failed",
                "t2",
                "t3",
                "t3-t1",
                "t3-t1-t1",
                "t3-t1-t1-t1",
                "t3-t1-t1-t1-last"
            ],
            "the error tasks are created at the ROOT level (`parent: null`), in \
             declaration order, only after the failure"
        );
        assert!(state.runs[1..]
            .iter()
            .all(|r| r.history.last() == Some(&RunState::Success)));
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "FAILED"],
            "the execution's state is `guessFinalState(flow.getTasks())` -- over \
             `tasks:`, never over the `errors:` that actually ran"
        );
    }

    /// A task-level `errors:` runs **under the flowable that owns it**, and the
    /// flow's own `errors:` runs at the root.
    ///
    /// The two corpus flows are the same flow written twice —
    /// `valids/sequential-with-local-errors` and
    /// `valids/sequential-with-global-errors` differ only in where the two
    /// `error-t*` tasks are declared — and Kestra's records differ in exactly
    /// one field: the error runs' `parent`.
    #[test]
    fn an_errors_list_belongs_to_whoever_declared_it() {
        let local = plan_of("valids/sequential-with-local-errors");
        let global = plan_of("valids/sequential-with-global-errors");
        assert!(global.errors.len() == 2 && global.tasks.iter().all(|t| t.errors.is_empty()));
        assert!(local.errors.is_empty() && local.tasks.iter().any(|t| t.errors.len() == 2));

        for (plan, want_parent) in [(&local, Some(2usize)), (&global, None)] {
            let mut state = ExecutionState::new(plan);
            for _ in 0..200 {
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
                let Some(ix) = in_flight else { break };
                let outcome = if plan.tasks[state.runs[ix].task].id == "t2-t1" {
                    RunState::Failed
                } else {
                    RunState::Success
                };
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                );
            }
            assert_eq!(
                run_ids(plan, &state),
                [
                    "parent-seq",
                    "t1",
                    "t2_seq",
                    "t2-t1",
                    "error-t1",
                    "error-t2"
                ]
            );
            let parents: Vec<Option<usize>> = state.runs[4..].iter().map(|r| r.parent).collect();
            assert_eq!(
                parents,
                vec![want_parent, want_parent],
                "a local `errors:` hangs off the `Sequential` that declared it \
                 (run 2, `t2_seq`); a flow-level one hangs off nothing"
            );
        }
    }

    /// A terminated [`Attempt`] takes no further state, and the run's own
    /// history takes every state.
    ///
    /// The rule lives in [`apply`], where it has no plan to consult, so this
    /// drives it directly rather than through `advance`.
    #[test]
    fn a_terminated_attempt_takes_no_further_state() {
        let plan = plan_of("valids/retry-failed-task-attempts");
        let mut state = ExecutionState::new(&plan);
        for cmd in [
            Command::CreateRun {
                task: 0,
                parent: None,
                value: None,
            },
            Command::StartRun {
                run: 0,
                at: Tick(5),
            },
            Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
            Command::SetRunState {
                run: 0,
                state: RunState::Retrying,
            },
            Command::SetRunState {
                run: 0,
                state: RunState::Created,
            },
        ] {
            apply(&mut state, &cmd);
        }
        assert_eq!(
            wire(&state.runs[0].attempts[0].history),
            ["CREATED", "RUNNING", "FAILED"],
            "the attempt ended at FAILED and takes neither the RETRYING nor the \
             re-CREATED that follow it"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED", "RETRYING", "CREATED"]
        );
    }

    /// `CREATE_NEW_EXECUTION` ends the execution `RETRIED`, marks every
    /// ancestor run with it, **and schedules the replay**.
    ///
    /// `markWithTaskRunAs(execution, taskRunId, RETRIED, true)` — the `true` is
    /// `markParents`, which recurses up `getParentTaskRunId()` and then sets the
    /// execution too, where `RETRY_FAILED_TASK` passes `false` and leaves every
    /// ancestor alone. `RETRIED` is terminal, so this is where this EXECUTION
    /// stops and Kestra replays the whole flow as a **different execution**.
    ///
    /// **The last assertion used to read `now == Tick(0)`, under "no
    /// `ScheduleRetry`: the delay belongs to the replay, not to this run", and
    /// W6 Task 4 measured that the artifact says the opposite.** Both arms of
    /// `ExecutorService.java:603-608` build one `ExecutionDelay` on THIS taskrun at
    /// `nextRetryDate`; only its `delayType` differs. The delay is how the
    /// replay knows when to happen, and `maxDuration` is measured against the
    /// instant it names, so a `CREATE_NEW_EXECUTION` arm that emitted none left
    /// the replay with no clock at all.
    #[test]
    fn create_new_execution_ends_the_execution_retried() {
        let plan = plan_of("valids/retry-new-execution-task-attempts");
        assert_eq!(
            plan.tasks[0].retry.as_ref().map(|r| r.behavior),
            Some(RetryBehavior::CreateNewExecution)
        );
        let (state, now) = run_to_fixpoint_over_time(&plan, &[RunState::Failed]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "SUBMITTED", "RUNNING", "FAILED", "RETRIED"]
        );
        assert_eq!(
            wire(&state.runs[0].attempts[0].history),
            ["CREATED", "RUNNING", "FAILED"],
            "one attempt: the next one belongs to an execution egret never creates"
        );
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "FAILED", "RETRIED"]
        );
        assert_eq!(
            now,
            Tick(1_000),
            "the replay's own `ExecutionDelay`: `interval: PT1S` past the \
             instant the run failed, which this harness advances to. It is the \
             only deadline in this execution, and `reopen_due` never consumes \
             it -- the run it names is `RETRIED`, not `RETRYING`"
        );
        assert_eq!(
            state.runs[0].not_before,
            Some(Tick(1_000)),
            "and it is written on the run that failed, which is where the \
             artifact's `ExecutionDelay.taskRunId` puts it"
        );
    }

    /// The two behaviours read different clocks, and both are already modelled.
    ///
    /// `TaskRun.java:337` measures a `RETRY_FAILED_TASK` `maxDuration` from
    /// `attempts.getFirst().getState().minDate()`; `TaskRun.java:316` measures a
    /// `CREATE_NEW_EXECUTION` one from
    /// `execution.getMetadata().getOriginalCreatedDate()`, which is [`Tick`]'s
    /// own origin. This drives a run whose first attempt starts **late**, so the
    /// two origins are 5 seconds apart and the same policy answers differently.
    #[test]
    fn the_two_retry_behaviours_measure_max_duration_from_different_instants() {
        let plan = plan_of("valids/retry-failed-task-attempts");
        let mut state = ExecutionState::new(&plan);
        apply(
            &mut state,
            &Command::CreateRun {
                task: 0,
                parent: None,
                value: None,
            },
        );
        apply(
            &mut state,
            &Command::StartRun {
                run: 0,
                at: Tick(5_000),
            },
        );
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
        );
        assert_eq!(state.runs[0].started_at(), Some(Tick(5_000)));

        let policy = |behavior| RetryPolicy {
            max_attempts: None,
            delay: RetryDelay::Constant {
                interval: std::time::Duration::from_secs(1),
            },
            max_duration: Some(std::time::Duration::from_secs(3)),
            behavior,
            warning_on_retry: false,
        };
        let at = Tick(5_000);
        assert_eq!(
            retry::next_retry_at(&policy(RetryBehavior::RetryFailedTask), &state, 0, at),
            Some(Tick(6_000)),
            "5000 + 1000 <= 5000 (the first attempt) + 3000"
        );
        assert_eq!(
            retry::next_retry_at(&policy(RetryBehavior::CreateNewExecution), &state, 0, at),
            None,
            "5000 + 1000 IS after 0 (the execution's creation) + 3000"
        );
    }

    /// **The two `ExecutionMeta` fields decide where a replay chain ends, and
    /// each is asserted against the constants it replaced.**
    ///
    /// W6 Task 4. `next_retry_at`'s `CREATE_NEW_EXECUTION` arm read
    /// `(1, Tick(0))` — literally "attempt 1, and this execution's own
    /// creation" — which is true of the first execution of a chain and false of
    /// every replay. Both halves are driven here on the SAME state, so the
    /// difference between them is the metadata and nothing else:
    ///
    /// - `attempt` against `maxAttempts`: `attemptNumber >= maxAttempts` is
    ///   `Integer >=` in Kestra (`TaskRun.java:311`), so attempt 3 of 3 is the
    ///   last and refuses;
    /// - `origin` against `maxDuration`: a replay's origin is a NEGATIVE tick,
    ///   the original execution's creation measured back from its own, so the
    ///   same policy at the same instant answers differently at different
    ///   depths of one chain.
    ///
    /// **Split into two assertions per field rather than one per behaviour**,
    /// because a single "it stops" would pass against an arm that stopped
    /// always: each pair moves ONE field across the boundary and asserts both
    /// sides of it.
    #[test]
    fn the_execution_metadata_is_what_ends_a_create_new_execution_chain() {
        let plan = plan_of("valids/retry-new-execution-task-attempts");
        let mut state = ExecutionState::new(&plan);
        apply(
            &mut state,
            &Command::CreateRun {
                task: 0,
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
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
        );

        let attempts = RetryPolicy {
            max_attempts: Some(3),
            delay: RetryDelay::Constant {
                interval: std::time::Duration::from_secs(1),
            },
            max_duration: None,
            behavior: RetryBehavior::CreateNewExecution,
            warning_on_retry: false,
        };
        state.metadata = ExecutionMeta {
            attempt: 2,
            origin: Tick(0),
        };
        assert_eq!(
            retry::next_retry_at(&attempts, &state, 0, Tick(10)),
            Some(Tick(1_010)),
            "attempt 2 of 3: one more replay is due, an interval away"
        );
        state.metadata.attempt = 3;
        assert_eq!(
            retry::next_retry_at(&attempts, &state, 0, Tick(10)),
            None,
            "attempt 3 of 3 exhausts the policy. With the constant `1` this \
             arm used to read, `1 >= 3` is false at every depth and the chain \
             never ends -- which the two `retry-new-execution-*-attempts` \
             entries would have measured as an unbounded driver run"
        );

        let duration = RetryPolicy {
            max_attempts: None,
            delay: RetryDelay::Constant {
                interval: std::time::Duration::from_secs(2),
            },
            max_duration: Some(std::time::Duration::from_secs(6)),
            behavior: RetryBehavior::CreateNewExecution,
            warning_on_retry: false,
        };
        state.metadata = ExecutionMeta {
            attempt: 2,
            origin: Tick(-2_000),
        };
        assert_eq!(
            retry::next_retry_at(&duration, &state, 0, Tick(10)),
            Some(Tick(2_010)),
            "the second execution of the chain: 2010 is not after -2000 + 6000"
        );
        state.metadata.origin = Tick(-4_010);
        assert_eq!(
            retry::next_retry_at(&duration, &state, 0, Tick(10)),
            None,
            "the third: 2010 IS after -4010 + 6000 = 1990. With the constant \
             `Tick(0)` this arm used to read, the bound is 6000 at every depth \
             and no elapsed time ever accumulates"
        );
    }

    /// `exponential` doubles by default and stops at `maxInterval`;
    /// `random` takes a delay inside Kestra's own bounds.
    ///
    /// `Exponential.nextRetryDate` is `interval * factor^(attemptCount - 1)`
    /// with `factor` defaulting to `2d` and the result capped at `maxInterval`
    /// by a `>=`. `Random.nextRetryDate` is
    /// `random.nextLong(minInterval, maxInterval)`, which `advance` cannot be
    /// and does not pretend to be — see [`RetryDelay::Random`] for what taking
    /// the lower bound can and cannot change.
    #[test]
    fn the_exponential_delay_doubles_and_the_random_one_stays_inside_its_bounds() {
        let expo = plan_of("valids/retry-expo");
        let Some(RetryDelay::Exponential {
            interval,
            max_interval,
            delay_factor,
        }) = expo.tasks[0].retry.as_ref().map(|r| r.delay.clone())
        else {
            panic!("valids/retry-expo declares `type: exponential`");
        };
        assert_eq!(
            (interval, max_interval, delay_factor),
            (
                // `PT0.25s`, with a LOWER-CASE `s`. `Duration.parse` is
                // case-insensitive and so is `retry::parse_duration`; a
                // case-sensitive one drops this whole policy silently.
                std::time::Duration::from_millis(250),
                std::time::Duration::from_secs(5),
                None
            )
        );
        let (state, now) = run_to_fixpoint_over_time(&expo, &[RunState::Failed]);
        assert_eq!(
            (state.runs[0].attempts.len(), now),
            (3, Tick(750)),
            "250 then 500: `interval * 2^(attemptCount - 1)`, and `maxAttempts: 3` \
             stops it before the cap is reached"
        );

        let random = plan_of("valids/retry-random");
        let Some(RetryDelay::Random {
            min_interval,
            max_interval,
        }) = random.tasks[0].retry.as_ref().map(|r| r.delay.clone())
        else {
            panic!("valids/retry-random declares `type: random`");
        };
        assert_eq!(
            (min_interval, max_interval),
            (
                std::time::Duration::from_millis(200),
                std::time::Duration::from_millis(500)
            )
        );
        let (state, now) = run_to_fixpoint_over_time(&random, &[RunState::Failed]);
        assert_eq!(state.runs[0].attempts.len(), 3);
        for (i, attempt) in state.runs[0].attempts.iter().enumerate().skip(1) {
            let waited = attempt.started_at.0 - state.runs[0].attempts[i - 1].started_at.0;
            assert!(
                (200..500).contains(&waited),
                "attempt {i} waited {waited}ms, outside Kestra's own \
                 [minInterval, maxInterval) -- a real RNG has to land here too"
            );
        }
        assert_eq!(now, Tick(400), "the lower bound, twice");
    }

    /// Java's `Duration.parse` grammar, over the shapes a `retry:` can be
    /// written in — and over the shapes it refuses.
    ///
    /// The case-insensitive rows are the load-bearing ones: `valids/retry-expo`
    /// writes `PT0.25s`, and a parser that reads only `S` drops that policy
    /// with no error at all, because a policy that cannot be read is a task
    /// that does not retry.
    #[test]
    fn the_retry_duration_grammar_is_javas_and_is_case_insensitive() {
        use std::time::Duration as D;
        let accepts: &[(&str, D)] = &[
            ("PT1S", D::from_secs(1)),
            ("PT0.25s", D::from_millis(250)),
            ("pt0.250S", D::from_millis(250)),
            ("PT7.5S", D::from_millis(7_500)),
            ("PT15M", D::from_secs(900)),
            ("PT1H", D::from_secs(3_600)),
            ("PT1H30M", D::from_secs(5_400)),
            ("P1D", D::from_secs(86_400)),
            ("P1DT2H", D::from_secs(93_600)),
            ("PT0S", D::ZERO),
        ];
        for (text, want) in accepts {
            assert_eq!(
                retry::parse_duration(text),
                Some(*want),
                "{text} must parse as Java's Duration.parse reads it"
            );
        }
        // Refused, each for its own reason: no designator, an empty time part,
        // a unit outside the grammar, trailing junk, a negative span, and an
        // expression a `Property` could hold but a literal parser cannot read.
        for text in [
            "1S",
            "PT",
            "P1W",
            "PT1S2",
            "-PT1S",
            "{{ inputs.wait }}",
            "",
            "P",
            "PTS",
        ] {
            assert_eq!(
                retry::parse_duration(text),
                None,
                "{text:?} is not a duration this builder may guess at"
            );
        }
    }

    /// Every `retry:` in the corpus is written in literals, so the one thing a
    /// pure builder cannot read is the one thing nobody writes.
    ///
    /// [`PlanTask::retry`] is dropped whole when any of its durations is not a
    /// literal, which is the safe direction — a policy this builder cannot
    /// evaluate must not become a policy it guesses at. This measures that the
    /// direction is never taken: every flow in the 345-flow corpus that
    /// declares a `retry:` anywhere produces a `RetryPolicy` for it.
    #[test]
    fn every_corpus_retry_is_written_in_literals() {
        let (mut declared, mut planned, mut flows) = (0usize, 0usize, 0usize);
        let mut dropped: Vec<String> = Vec::new();
        for (id, source) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&source) else {
                continue;
            };
            let Ok(plan) = build_plan(&source) else {
                continue;
            };
            let mut here = 0usize;
            if flow.retry.is_some() {
                here += 1;
                if plan.retry.is_some() {
                    planned += 1;
                } else {
                    dropped.push(format!("{id} (flow-level)"));
                }
            }
            // The plan is flat and the model is a tree, so this counts the
            // TASKS the plan holds -- which is every task `build_plan`
            // descended into. A `retry:` inside an unimplemented flowable is
            // invisible to both sides and is counted by neither.
            let mut stack: Vec<&crate::models::Task> = flow.tasks.iter().collect();
            stack.extend(flow.errors.iter());
            while let Some(task) = stack.pop() {
                if task.retry.is_some() {
                    here += 1;
                    match plan.tasks.iter().find(|p| p.id == task.id) {
                        Some(p) if p.retry.is_some() => planned += 1,
                        Some(_) => dropped.push(format!("{id}/{}", task.id)),
                        // Not planned at all: inside a flowable `build_plan`
                        // keeps opaque, so there is no `PlanTask` to carry it.
                        None => here -= 1,
                    }
                }
                stack.extend(task.children());
            }
            declared += here;
            if here > 0 {
                flows += 1;
            }
        }
        assert_eq!(
            dropped,
            Vec::<String>::new(),
            "a `retry:` whose durations are not literals is dropped whole; none is"
        );
        assert_eq!(
            (flows, declared, planned),
            (25, 26, 26),
            "25 corpus flows declare a `retry:` on a task `build_plan` plans (or \
             on the flow), 26 policies between them, and every one of them is \
             readable as literals. If this moves, the corpus moved -- OR a flow \
             left the population by becoming refusable, which is what took this \
             from (26, 27, 27) at Task 7: `invalids/duplicate-parallel` declares \
             a `retry:` and is now refused for `duplicate_task_id`, which is \
             what Kestra refuses it for"
        );
    }

    /// **Every `errors:` the corpus writes, by the type that owns it — and
    /// which of them `build_plan` can actually read.**
    ///
    /// # This walks the RAW document, and the round-0 version could not fail
    ///
    /// Round 0 matched on W1's parsed [`crate::models::TaskKind`]
    /// (`Sequential | Parallel | If | Switch | Dag => …errors.len(), _ => 0`),
    /// which is a guard that cannot fail for the thing it exists to detect: a
    /// type whose model has **no** `errors` field contributes zero by
    /// construction, so the derived map could only ever hold the five types
    /// that already had one. It reported "9 tasks on three of the five types"
    /// and was hiding a fourth.
    ///
    /// This reads the YAML itself, so a type W1 does not model still counts,
    /// and then asks the **plan** whether the list survived — which is the
    /// question the title is about.
    ///
    /// # What it surfaces: `Loop`
    ///
    /// `AllowFailure`, `Pause` and `WorkingDirectory` write an `errors:` that
    /// `build_plan` cannot read, and that is not a gap: all three are planned
    /// [`TaskKind::Runnable`], so their whole subtree is opaque and a field on
    /// them would be unreadable by construction.
    ///
    /// **`Loop` is a gap.** It is planned as a [`TaskKind::Loop`] — `advance()`
    /// drives it and spawns for it — and `models::Loop` carries no `errors`
    /// field, so `plan.rs`'s flattening has no `Loop` arm and **four** corpus
    /// flows' lists are invisible to the executor. All four are baselined, so
    /// nothing banked on it; the debt is recorded here, on
    /// [`PlanTask::errors`], and in `oracle-exec/README.md`'s unclaimed table.
    ///
    /// A `Loop`'s `errors:` is also not the same construct as its body: Kestra
    /// resolves it in the **sub-execution** (`Loop.java:272`, inside
    /// `resolveState` for `ExecutionKind.LOOP`), which W3a runs none of. So
    /// closing this needs the sub-execution hole closed first, and it is filed
    /// beside `Subflow` and the `CREATE_NEW_EXECUTION` replay rather than as a
    /// missing `match` arm.
    #[test]
    fn every_corpus_errors_list_is_on_a_flowable_egret_plans() {
        // Every object carrying an `id`, a `type` and a non-empty `errors`,
        // as `(owner id, owner type, error task ids)`.
        fn walk(node: &serde_json::Value, out: &mut Vec<(String, String, Vec<String>)>) {
            match node {
                serde_json::Value::Object(map) => {
                    if let (Some(id), Some(ty), Some(errors)) = (
                        map.get("id").and_then(|v| v.as_str()),
                        map.get("type").and_then(|v| v.as_str()),
                        map.get("errors").and_then(|v| v.as_array()),
                    ) {
                        if !errors.is_empty() {
                            out.push((
                                id.to_string(),
                                ty.to_string(),
                                errors
                                    .iter()
                                    .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                                    .collect(),
                            ));
                        }
                    }
                    for value in map.values() {
                        walk(value, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, out);
                    }
                }
                _ => {}
            }
        }

        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        // The owner is a task `advance()` DRIVES and its `errors:` did not
        // reach the plan. That is the gap this test exists to find.
        let mut gaps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut opaque: BTreeMap<String, usize> = BTreeMap::new();
        let mut read: BTreeMap<String, usize> = BTreeMap::new();
        let mut flow_level = 0usize;

        for (flow_id, source) in every_corpus_flow() {
            let Ok(doc) = serde_norway::from_str::<serde_json::Value>(&source) else {
                continue;
            };
            // The flow's own `errors:`: the root object has an `id` and no
            // `type`, so `walk` never counts it.
            if doc
                .get("errors")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                flow_level += 1;
            }
            let mut found = Vec::new();
            walk(&doc, &mut found);
            let plan = build_plan(&source).ok();
            for (owner, ty, error_ids) in found {
                *by_type.entry(ty.clone()).or_default() += 1;
                let Some(plan) = plan.as_ref() else { continue };
                match plan.tasks.iter().find(|t| t.id == owner) {
                    // Not planned at all, or planned opaque: its children get
                    // no run either way, so an unread `errors:` costs nothing.
                    None => *opaque.entry(ty).or_default() += 1,
                    Some(t) if matches!(t.kind, TaskKind::Runnable { .. }) => {
                        *opaque.entry(ty).or_default() += 1
                    }
                    // Driven by `advance()`. Its `errors:` must have reached
                    // the plan, or this is a gap with a name.
                    Some(t) if t.errors.is_empty() => {
                        gaps.entry(ty).or_default().insert(flow_id.clone());
                    }
                    Some(t) => {
                        assert!(
                            error_ids
                                .iter()
                                .all(|want| t.errors.iter().any(|&e| plan.tasks[e].id == *want)),
                            "{flow_id}: {owner}'s `errors:` reached the plan \
                             partially, which no arm can produce"
                        );
                        *read.entry(ty).or_default() += 1;
                    }
                }
            }
        }

        println!(
            "task-level errors: {by_type:?}; read {read:?}; opaque {opaque:?}; \
             DRIVEN BUT UNREAD {gaps:?}"
        );
        assert_eq!(
            (flow_level, by_type.clone()),
            (
                17,
                BTreeMap::from([
                    ("io.kestra.plugin.core.flow.AllowFailure".to_string(), 4),
                    ("io.kestra.plugin.core.flow.Dag".to_string(), 1),
                    ("io.kestra.plugin.core.flow.Loop".to_string(), 4),
                    ("io.kestra.plugin.core.flow.Parallel".to_string(), 1),
                    ("io.kestra.plugin.core.flow.Pause".to_string(), 1),
                    ("io.kestra.plugin.core.flow.Sequential".to_string(), 7),
                    ("io.kestra.plugin.core.flow.WorkingDirectory".to_string(), 1),
                ])
            ),
            "17 flows write a flow-level `errors:` and 18 TASKS write one, on \
             SEVEN types -- derived from the YAML, so a type W1 does not model \
             still appears here. Round 0 matched on W1's parsed kind and could \
             only ever report the five types that already had the field.\n\n\
             The denominator is all **345** corpus flows, `invalids/` included, \
             and that is worth two tasks rather than a footnote. Counting only \
             `valids/` gives `Loop` 3 and `Sequential` 6: the missing `Loop` is \
             `invalids/loop-switch-failed`, which Task 1 measured is NOT an \
             invalid flow at all -- Kestra's own `RunContextTest` runs it and \
             `golden.json` holds it with verdict `runs` -- and the missing \
             `Sequential` is `invalids/duplicate`, which egret over-accepts \
             today. So the `valids/`-only reading drops one flow this oracle \
             genuinely compares and one that exposes a known over-acceptance, \
             which is why `every_corpus_flow()` is the population everywhere in \
             this crate"
        );
        assert_eq!(
            gaps,
            BTreeMap::from([(
                "io.kestra.plugin.core.flow.Loop".to_string(),
                BTreeSet::from([
                    "invalids/loop-switch-failed".to_string(),
                    "valids/all-flowable".to_string(),
                    "valids/finally-loop".to_string(),
                    "valids/finally-loop-parallel".to_string(),
                ])
            )]),
            "the ONE type `advance()` drives whose `errors:` `build_plan` cannot \
             read. `models::Loop` has no `errors` field, so `plan.rs` has no \
             `Loop` arm. All FOUR flows are baselined, so nothing banks on it -- \
             and `invalids/loop-switch-failed` is one of them, a `runs` entry \
             despite its directory. It is debt, it is named in \
             `oracle-exec/README.md`'s unclaimed table, and this is what fails \
             if a fifth appears or if someone closes it without deleting this \
             line"
        );
    }

    /// **Two retries falling due in the same round put ONE `RUNNING` on the
    /// execution, not one each.**
    ///
    /// `advance` is a pure function of the state it is handed and any caller may
    /// hand it any state, so this hands it one the corpus cannot reach: two
    /// sibling `Runnable`s of a `Parallel`, both `RETRYING`, both past their
    /// `not_before`. The state is hand-built for the same reason
    /// `parallel::drive_concurrent`'s doc gives for the guard it deleted — the
    /// difference between "unreachable under today's caller" and "unreachable"
    /// is the whole distinction, and here the rule is kept because the wrong
    /// shape is a real defect: `ExecutionService.retryTask` is
    /// `withState(RUNNING)` on the **execution**, which has one state, and a
    /// per-run emission would push a duplicate `RUNNING` into a history the
    /// golden never shows one in.
    ///
    /// Without this test the rule was a guard nobody had seen fail: the
    /// mutation that emits per run killed **nothing** across the whole crate.
    #[test]
    fn two_retries_due_in_one_round_move_the_execution_to_running_once() {
        let plan = plan_of("valids/parallel");
        let mut state = ExecutionState::new(&plan);
        // Drive far enough that the `Parallel` and its children exist.
        for _ in 0..10 {
            let cmds = advance(&plan, &state, Tick(0));
            if cmds.is_empty() {
                break;
            }
            for cmd in &cmds {
                apply(&mut state, cmd);
            }
        }
        let children: Vec<RunIx> = (0..state.runs.len())
            .filter(|&ix| {
                state.runs[ix].parent.is_some()
                    && matches!(
                        plan.tasks[state.runs[ix].task].kind,
                        TaskKind::Runnable { .. }
                    )
            })
            .take(2)
            .collect();
        assert_eq!(children.len(), 2, "valids/parallel has several children");
        for &ix in &children {
            for cmd in [
                Command::SetRunState {
                    run: ix,
                    state: RunState::Failed,
                },
                Command::SetRunState {
                    run: ix,
                    state: RunState::Retrying,
                },
                Command::ScheduleRetry {
                    run: ix,
                    not_before: Tick(1_000),
                },
            ] {
                apply(&mut state, &cmd);
            }
        }
        apply(
            &mut state,
            &Command::SetExecutionState {
                state: RunState::Retrying,
            },
        );

        let cmds = advance(&plan, &state, Tick(1_000));
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(
                    c,
                    Command::SetExecutionState {
                        state: RunState::Running
                    }
                ))
                .count(),
            1,
            "the execution has ONE state, so two due retries move it to RUNNING \
             once -- got {cmds:?}"
        );
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(
                    c,
                    Command::SetRunState {
                        state: RunState::Created,
                        ..
                    }
                ))
                .count(),
            2,
            "both runs are re-CREATED, though: `retryTask` is per taskRun"
        );
    }

    /// **`markParents` walks the WHOLE ancestor chain, and `false` walks none.**
    ///
    /// `markWithTaskRunAs(execution, id, RETRIED, true)` recurses up
    /// `getParentTaskRunId()` (`ExecutionService.java:668-670`); the
    /// `RETRY_FAILED_TASK` call passes `false` and leaves every ancestor alone.
    /// No corpus flow puts a `CREATE_NEW_EXECUTION` retry on a nested task — all
    /// four `retry-new-execution-*` flows fail a **root** task — so a mutation
    /// that stopped the walk after one level killed nothing. This drives a
    /// nested one.
    #[test]
    fn create_new_execution_marks_every_ancestor_run_retried() {
        let source = "\
id: probe-new-execution-nested
namespace: io.kestra.tests
tasks:
  - id: outer
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: inner
        type: io.kestra.plugin.core.flow.Sequential
        tasks:
          - id: boom
            type: io.kestra.plugin.core.execution.Fail
            retry:
              behavior: CREATE_NEW_EXECUTION
              type: constant
              maxAttempts: 3
              interval: PT1S
";
        let plan = build_plan(source).expect("a plan");
        let (state, _) = run_to_fixpoint_over_time(&plan, &[RunState::Failed]);
        assert_eq!(run_ids(&plan, &state), ["outer", "inner", "boom"]);
        for (ix, id) in ["outer", "inner", "boom"].iter().enumerate() {
            assert_eq!(
                state.runs[ix].history.last(),
                Some(&RunState::Retried),
                "`markParents: true` marks {id} too"
            );
        }
        assert_eq!(state.history.last(), Some(&RunState::Retried));
    }

    /// **What `errors:` adds to `Plan::is_loop_body`, measured — and it is
    /// zero on this corpus.**
    ///
    /// `Plan::children_of` walks `errors` because Java's
    /// `AbstractBranch.allChildTasks()` does, and a task hanging off an
    /// `errors:` inside a loop body is just as absent from the parent's
    /// `taskRunList` as one hanging off `tasks:`. But a mutation that dropped
    /// the `errors` half killed nothing, and this is why: **no corpus loop body
    /// contains a flowable with an `errors:` at all**, so the wide walk and the
    /// walk without it name the same tasks.
    ///
    /// So the rule is kept and its cost is a measurement rather than an
    /// argument. If a later corpus makes this non-zero, the assertion is what
    /// says the walk started mattering.
    #[test]
    fn an_errors_list_inside_a_loop_body_is_a_shape_the_corpus_does_not_reach() {
        let (mut wide, mut without_errors, mut flows) = (0usize, 0usize, 0usize);
        for (_, source) in every_corpus_flow() {
            let Ok(plan) = build_plan(&source) else {
                continue;
            };
            if !plan
                .tasks
                .iter()
                .any(|t| matches!(t.kind, TaskKind::Loop { .. }))
            {
                continue;
            }
            flows += 1;
            let here_wide = (0..plan.tasks.len())
                .filter(|&ix| plan.is_loop_body(ix))
                .count();
            wide += here_wide;
            // The same predicate with every `errors:` list emptied, which is
            // exactly what dropping the `errors` half of `children_of` would
            // reach.
            let mut stripped = plan.clone();
            for task in &mut stripped.tasks {
                task.errors.clear();
            }
            without_errors += (0..stripped.tasks.len())
                .filter(|&ix| stripped.is_loop_body(ix))
                .count();
        }
        assert_eq!(
            (flows, wide),
            (43, 88),
            "the 43 corpus flows holding a `Loop` and the 88 tasks the wide \
             reading names inside one -- the same 88 \
             `the_two_readings_of_inside_a_loop_differ_by_a_measured_amount` \
             derives. 43 flows and not 42: the flow count is over the YAML and \
             `disabled:` does not change it"
        );
        assert_eq!(
            wide - without_errors,
            0,
            "not one of those 89 is reached through an `errors:` list, so \
             walking `errors` in `Plan::children_of` costs and buys nothing \
             here. It is Kestra's own walk and it stays; this is its price"
        );
    }

    // ----------------------------------------------------------------------
    // W3c: `runIf:`
    // ----------------------------------------------------------------------

    /// A three-task flow whose middle task carries `run_if`.
    fn seq_with_run_if(run_if: &str) -> Plan {
        build_plan(&format!(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n\
             \x20 - id: first\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n\
             \x20 - id: middle\n    type: io.kestra.plugin.core.debug.Return\n    format: b\n    runIf: \"{run_if}\"\n\
             \x20 - id: last\n    type: io.kestra.plugin.core.debug.Return\n    format: c\n"
        ))
        .expect("the probe plans")
    }

    /// **A falsy `runIf:` creates the run and then does not execute it**, which
    /// is the whole of W3c and is *not* what `disabled:` does.
    ///
    /// Measured on `valids/task-runif`, whose golden is:
    ///
    /// ```text
    /// output               CREATED SUBMITTED RUNNING SUCCESS
    /// executed             CREATED SUBMITTED RUNNING SUCCESS
    /// notexecuted          CREATED SKIPPED
    /// notexecutedflowable  CREATED SKIPPED
    /// willfailedtheflow    CREATED FAILED
    /// ```
    ///
    /// So a skipped run exists, has no `SUBMITTED` and no `RUNNING`, carries one
    /// attempt reading `CREATED SKIPPED`, and **the sequence keeps going** —
    /// `notexecutedflowable` is created after `notexecuted` was skipped. A
    /// disabled task, by contrast, has no run at all (`flatten_list`).
    #[test]
    fn a_falsy_run_if_creates_the_run_skips_it_and_the_sequence_continues() {
        let plan = seq_with_run_if("false");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(run_ids(&plan, &state), ["first", "middle", "last"]);
        assert_eq!(wire(&state.runs[1].history), ["CREATED", "SKIPPED"]);
        assert_eq!(state.runs[1].attempts.len(), 1);
        assert_eq!(
            wire(&state.runs[1].attempts[0].history),
            ["CREATED", "SKIPPED"]
        );
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "a skipped sibling does not stop the sequence: Kestra creates \
             `notexecutedflowable` after skipping `notexecuted`"
        );
        assert_eq!(
            state.history.last(),
            Some(&RunState::Success),
            "`Execution.guessFinalState` has no arm for SKIPPED, so it leaves the \
             aggregate alone -- `valids/task-runif-executionupdating` ends \
             SUCCESS with two of its five runs SKIPPED"
        );

        // The control, same flow, same driver: a TRUTHY `runIf:` changes
        // nothing at all. Without it, an implementation that skipped
        // unconditionally would pass every assertion above.
        let plan = seq_with_run_if("true");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"]
        );
    }

    /// **The falsy set is Kestra's `TruthUtils.FALSE_VALUES`, and it is the
    /// SAME predicate an `If` uses** — not a second copy of it.
    ///
    /// `branch::is_truthy` is measured against the running artifact over 38
    /// rows, including the 18 that pin Java's `String.trim()` against Rust's
    /// `str::trim` (11 of them disagree, in both directions). Writing a second
    /// predicate here would have been a second chance to get that wrong; this
    /// asserts that `runIf:` goes through the first one, on the rows where a
    /// naive reading differs.
    #[test]
    fn a_run_ifs_truthiness_is_the_same_predicate_an_if_uses() {
        for (rendered, skipped) in [
            ("false", true),
            ("0", true),
            ("-0", true),
            ("", true),
            ("  false  ", true),
            ("true", false),
            ("00", false),
            ("0.0", false),
            ("FALSE", false),
            ("null", false),
            ("no", false),
            // The trim rows: `\u{1}` is <= U+0020 so Java strips it and the
            // value is falsy; `\u{A0}` is not, so Java keeps it and the value
            // is truthy. Rust's `str::trim` answers the opposite on both.
            ("\u{1}false\u{1}", true),
            ("\u{A0}false\u{A0}", false),
        ] {
            let plan = seq_with_run_if("{{ x }}");
            let state = run_to_fixpoint_rendering_run_if(
                &plan,
                RunState::Success,
                &[],
                &[("middle", rendered)],
            );
            assert_eq!(
                state.runs[1].history.last() == Some(&RunState::Skipped),
                skipped,
                "rendered {rendered:?}: expected skipped={skipped}, got {:?}",
                wire(&state.runs[1].history)
            );
        }
    }

    /// **An unrendered `runIf:` asks for nothing and is not an error** — the
    /// same rule `an_if_with_no_rendered_condition_asks_for_nothing_and_is_not_an_error`
    /// holds for a condition, and for the same reason.
    ///
    /// `advance()` is pure and cannot render. Treating `None` as falsy would
    /// skip every task the caller had not reached yet, silently and on every
    /// flow that uses the feature.
    #[test]
    fn an_unrendered_run_if_asks_for_nothing_and_is_not_an_error() {
        let plan = seq_with_run_if("{{ inputs.go }}");
        let mut state = ExecutionState::new(&plan);
        // first: created, submitted, running -> succeed it.
        for _ in 0..4 {
            round(&plan, &mut state);
        }
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Success,
            },
        );
        // Now `middle` is created and its `runIf:` is unrendered.
        round(&plan, &mut state);
        assert_eq!(run_ids(&plan, &state), ["first", "middle"]);
        assert_eq!(wire(&state.runs[1].history), ["CREATED"]);
        assert_eq!(
            advance(&plan, &state, T0),
            Vec::new(),
            "with the `runIf:` unrendered there is nothing to decide: not a \
             skip, not a start, and not an error"
        );

        // And the caller answering unblocks it, so the wait above is a wait and
        // not a deadlock.
        state.runs[1].rendered_run_if = Some("true".to_string());
        assert!(!advance(&plan, &state, T0).is_empty());
    }

    /// **A skipped flowable never creates its children**, which is what makes
    /// `valids/task-runif`'s `notexecutedflowable` one run rather than a
    /// subtree.
    ///
    /// The check is in [`flowable::open`], before the flowable is descended
    /// into at all, so this holds for every kind — it is asserted on a
    /// `Sequential` because a `Subflow` is planned opaque and could not tell
    /// the difference.
    #[test]
    fn a_skipped_flowable_creates_no_children_and_leaves_its_parent_successful() {
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    runIf: \"false\"\n    \
             tasks:\n      - id: inner1\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      \
             - id: inner2\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n",
        )
        .expect("the probe plans");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(run_ids(&plan, &state), ["seq"]);
        assert_eq!(wire(&state.runs[0].history), ["CREATED", "SKIPPED"]);
        assert_eq!(
            state.history.last(),
            Some(&RunState::Success),
            "the flow's own list aggregates a SKIPPED child as benign"
        );
    }

    /// **A `Loop` decides `runIf:` before it resolves its `values:`**, so a
    /// skipped `Loop` spawns nothing.
    ///
    /// This is why the check is at the top of [`flowable::open`] rather than
    /// inside [`start`](flowable::start): a `Loop` never calls `start` —
    /// `loop_::open` answers at `CREATED` — so a check in `start` would let a
    /// skipped `Loop` spawn its whole first batch.
    #[test]
    fn a_skipped_loop_spawns_nothing() {
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: lp\n    type: io.kestra.plugin.core.flow.Loop\n    runIf: \"false\"\n    \
             values: '[\"a\", \"b\"]'\n    tasks:\n      - id: body\n        \
             type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("the probe plans");
        let mut state = ExecutionState::new(&plan);
        let mut spawns = 0usize;
        for _ in 0..20 {
            let cmds = advance(&plan, &state, T0);
            if cmds.is_empty() {
                // Tier 2a, by hand: a literal `runIf:` renders to itself.
                let Some(ix) = (0..state.runs.len()).find(|&ix| {
                    plan.tasks[state.runs[ix].task].run_if.is_some()
                        && state.runs[ix].rendered_run_if.is_none()
                }) else {
                    break;
                };
                state.runs[ix].rendered_run_if = plan.tasks[state.runs[ix].task].run_if.clone();
                continue;
            }
            for cmd in &cmds {
                if matches!(cmd, Command::SpawnExecution { .. }) {
                    spawns += 1;
                }
                apply(&mut state, cmd);
            }
        }
        assert_eq!(run_ids(&plan, &state), ["lp"]);
        assert_eq!(wire(&state.runs[0].history), ["CREATED", "SKIPPED"]);
        assert_eq!(
            spawns, 0,
            "a skipped `Loop` asks for no sub-execution; the values are never \
             resolved because the run never leaves CREATED alive"
        );

        // The control: the same `Loop` with a truthy `runIf:` spawns its batch,
        // so the zero above is the skip and not the probe.
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: lp\n    type: io.kestra.plugin.core.flow.Loop\n    runIf: \"true\"\n    \
             values: '[\"a\", \"b\"]'\n    tasks:\n      - id: body\n        \
             type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("the probe plans");
        let mut state = ExecutionState::new(&plan);
        let mut spawns = 0usize;
        for _ in 0..20 {
            let cmds = advance(&plan, &state, T0);
            if cmds.is_empty() {
                let unrendered = (0..state.runs.len()).find(|&ix| {
                    plan.tasks[state.runs[ix].task].run_if.is_some()
                        && state.runs[ix].rendered_run_if.is_none()
                });
                if let Some(ix) = unrendered {
                    state.runs[ix].rendered_run_if = plan.tasks[state.runs[ix].task].run_if.clone();
                    continue;
                }
                let unrendered = (0..state.runs.len()).find(|&ix| {
                    matches!(plan.tasks[state.runs[ix].task].kind, TaskKind::Loop { .. })
                        && state.runs[ix].rendered.is_none()
                });
                match unrendered {
                    Some(ix) => {
                        state.runs[ix].rendered = Some("[\"a\", \"b\"]".to_string());
                        continue;
                    }
                    None => break,
                }
            }
            for cmd in &cmds {
                if matches!(cmd, Command::SpawnExecution { .. }) {
                    spawns += 1;
                }
                apply(&mut state, cmd);
            }
        }
        assert!(spawns > 0, "the control must spawn");
    }

    /// **A skipped branch does not fail its siblings** — the guard on
    /// [`parallel::drive_concurrent`]'s rule 1.
    ///
    /// Through W3a, "terminal and not `SUCCESS`" and "terminated in error" were
    /// the same set for every state [`RunState`] could hold. `Skipped` is the
    /// first that is not: `State.java:293` lists it in `isTerminatedNoFail()`
    /// beside `SUCCESS`. Without the exclusion, `fail_the_rest` would `FAIL`
    /// every sibling still in flight the moment one branch's `runIf:` came back
    /// falsy — egret failing runs Kestra succeeds.
    #[test]
    fn a_skipped_branch_does_not_fail_its_siblings() {
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
             - id: skipped\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"false\"\n      \
             - id: sibling\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n",
        )
        .expect("the probe plans");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(run_ids(&plan, &state), ["par", "skipped", "sibling"]);
        assert_eq!(wire(&state.runs[1].history), ["CREATED", "SKIPPED"]);
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "the sibling must run to completion, not be failed by rule 1"
        );
        assert_eq!(state.runs[0].history.last(), Some(&RunState::Success));
    }

    /// **A `Dag` node downstream of a skipped dependency still runs.**
    ///
    /// `FlowableUtils.resolveDagNexts`'s filter is "all dependencies are
    /// **terminated**", and `dag::drive` spells it `SUCCESS` because through
    /// W3a every terminated-but-not-successful dependency had already ended the
    /// branch. `SKIPPED` is the state where that argument stops holding, so the
    /// predicate accepts it — otherwise a `runIf: "false"` on one node would
    /// hang the whole `Dag` for ever, on a flow Kestra completes.
    #[test]
    fn a_dag_node_downstream_of_a_skipped_dependency_still_runs() {
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: d\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      \
             - task:\n          id: up\n          type: io.kestra.plugin.core.debug.Return\n          format: a\n          runIf: \"false\"\n      \
             - task:\n          id: down\n          type: io.kestra.plugin.core.debug.Return\n          format: b\n        dependsOn:\n          - up\n",
        )
        .expect("the probe plans");
        let state = run_to_fixpoint_rendering(&plan, RunState::Success, &[]);
        assert_eq!(run_ids(&plan, &state), ["d", "up", "down"]);
        assert_eq!(wire(&state.runs[1].history), ["CREATED", "SKIPPED"]);
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "with a `SUCCESS`-only predicate this run is never created and the \
             `Dag` never terminates -- a hang, on a flow Kestra completes"
        );
        assert_eq!(state.history.last(), Some(&RunState::Success));
    }

    /// **`disabled:` and `runIf:` are the same shape and different rules**, and
    /// this is the pair of traces that says so.
    ///
    /// A disabled task has **no run at all** (`Execution.removeDisabled`, a list
    /// filter). A `runIf:`-false task has a run in `SKIPPED`. The oracle
    /// README credited the golden's `SKIPPED` population to `disabled:` in
    /// three places and the plan said it too; both were false, and this asserts
    /// the difference in one place rather than leaving it to prose.
    #[test]
    fn disabled_removes_the_run_and_run_if_skips_it() {
        let disabled = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n    disabled: true\n",
        )
        .expect("plans");
        let state = run_to_fixpoint(&disabled, RunState::Success);
        assert_eq!(
            run_ids(&disabled, &state),
            Vec::<String>::new(),
            "`disabled:` is a plan-time deletion: no taskrun at all"
        );

        let run_if = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n    runIf: \"false\"\n",
        )
        .expect("plans");
        let state = run_to_fixpoint_rendering(&run_if, RunState::Success, &[]);
        assert_eq!(run_ids(&run_if, &state), ["t"]);
        assert_eq!(wire(&state.runs[0].history), ["CREATED", "SKIPPED"]);
    }

    /// **`runIf:` reaches the plan from every task position `build_plan`
    /// flattens**, derived rather than spot-checked.
    ///
    /// W3c's whole failure mode is a field that parses and is then dropped at
    /// some boundary — which is exactly what happened to `disabled:` through
    /// eight tasks. `flatten_one` writes `run_if` once, before the kind
    /// dispatch, so every position goes through it; this is what says so for
    /// each of the positions a corpus flow can put a task in.
    #[test]
    fn a_run_if_survives_every_position_the_builder_flattens() {
        let probe = "id: probe\nnamespace: io.kestra.tests\n\
             tasks:\n  \
             - id: root\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n    runIf: \"r\"\n  \
             - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      \
             - id: seqchild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"sc\"\n    \
             errors:\n      - id: seqerr\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"se\"\n  \
             - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
             - id: parchild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"pc\"\n  \
             - id: iff\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{ x }}\"\n    then:\n      \
             - id: thenchild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"tc\"\n    \
             else:\n      - id: elsechild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"ec\"\n  \
             - id: sw\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"{{ x }}\"\n    cases:\n      \
             A:\n        - id: casechild\n          type: io.kestra.plugin.core.debug.Return\n          format: a\n          runIf: \"cc\"\n    \
             defaults:\n      - id: defchild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"dc\"\n  \
             - id: dag\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      \
             - task:\n          id: dagchild\n          type: io.kestra.plugin.core.debug.Return\n          format: a\n          runIf: \"gc\"\n  \
             - id: lp\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\"]'\n    tasks:\n      \
             - id: bodychild\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n        runIf: \"bc\"\n\
             errors:\n  - id: flowerr\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n    runIf: \"fe\"\n";
        let plan = build_plan(probe).expect("the probe plans");
        let got: BTreeMap<String, String> = plan
            .tasks
            .iter()
            .filter_map(|t| t.run_if.clone().map(|r| (t.id.clone(), r)))
            .collect();
        assert_eq!(
            got,
            BTreeMap::from(
                [
                    ("root", "r"),
                    ("seqchild", "sc"),
                    ("seqerr", "se"),
                    ("parchild", "pc"),
                    ("thenchild", "tc"),
                    ("elsechild", "ec"),
                    ("casechild", "cc"),
                    ("defchild", "dc"),
                    ("dagchild", "gc"),
                    ("bodychild", "bc"),
                    ("flowerr", "fe"),
                ]
                .map(|(a, b)| (a.to_string(), b.to_string()))
            ),
            "eleven positions, and each one is a place a dropped field would be \
             invisible to every other test in this file"
        );
    }

    /// **The instant on a skip is the `now` the decision was taken at**, and
    /// the plumbing from [`flowable::open`] to the stored [`Attempt`] is
    /// asserted here rather than only at [`apply`].
    ///
    /// `Command::SkipRun`'s doc claims the instant "can only be the `now` the
    /// decision was taken at". **Nothing held that claim.** Replacing
    /// `at: now` with `at: Tick(0)` in `open` left all 561 tests green — the
    /// state test calls `apply` with a literal, so it pins the bookkeeping and
    /// not the seam. The control for the shape already existed one line away:
    /// the same mutation on `StartRun`'s `at: now` kills 3.
    ///
    /// Two ticks, because a single non-zero one would pass against a machine
    /// that stored the first tick it ever saw.
    #[test]
    fn a_skipped_runs_attempt_records_the_tick_the_skip_was_decided_at() {
        for tick in [Tick(11), Tick(4200)] {
            let plan = seq_with_run_if("false");
            let mut state = ExecutionState::new(&plan);
            // `first` is created, started and finished, so that `middle` -- the
            // one carrying the `runIf:` -- is opened at `tick` and nothing else
            // is in flight when it is.
            for _ in 0..3 {
                for cmd in &advance(&plan, &state, Tick(0)) {
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
            // Create `middle`. Its literal `runIf:` renders to itself.
            for cmd in &advance(&plan, &state, tick) {
                apply(&mut state, cmd);
            }
            assert_eq!(run_ids(&plan, &state), ["first", "middle"]);
            state.runs[1].rendered_run_if = Some("false".to_string());

            let cmds = advance(&plan, &state, tick);
            assert_eq!(
                cmds,
                vec![Command::SkipRun { run: 1, at: tick }],
                "`open` must carry the `now` it was called with onto the command"
            );
            for cmd in &cmds {
                apply(&mut state, cmd);
            }
            assert_eq!(
                state.runs[1].attempts[0].started_at, tick,
                "and `apply` must store it, so the instant on a skipped attempt \
                 is the one the decision was taken at end to end"
            );
        }
    }

    /// **No corpus flow puts a `runIf:` on a `Parallel` or `Dag` child**, so
    /// the two rules W3c had to widen for `SKIPPED` are read from Kestra's Java
    /// rather than measured on the artifact — and that is stated here rather
    /// than left for a reader to assume otherwise.
    ///
    /// If a corpus flow ever does, both `a_skipped_branch_does_not_fail_its_siblings`
    /// and `a_dag_node_downstream_of_a_skipped_dependency_still_runs` acquire a
    /// golden entry that can judge them, and this test is what would say so.
    #[test]
    fn no_corpus_parallel_or_dag_child_declares_a_run_if() {
        // Over the raw YAML, for the reason every construct walk in this file
        // is: reading the model would score zero for anything the model drops.
        fn walk(node: &serde_json::Value, out: &mut Vec<String>) {
            if let serde_json::Value::Object(map) = node {
                let canon = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .and_then(crate::models::canonical_flow_type);
                match canon.as_deref() {
                    Some("parallel") => {
                        for child in map
                            .get("tasks")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                        {
                            if child.get("runIf").is_some() {
                                out.push(child["id"].as_str().unwrap_or("<no id>").to_string());
                            }
                        }
                    }
                    Some("dag") => {
                        for entry in map
                            .get("tasks")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                        {
                            let Some(task) = entry.get("task") else {
                                continue;
                            };
                            if task.get("runIf").is_some() {
                                out.push(task["id"].as_str().unwrap_or("<no id>").to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            match node {
                serde_json::Value::Object(map) => {
                    for v in map.values() {
                        walk(v, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for v in items {
                        walk(v, out);
                    }
                }
                _ => {}
            }
        }

        // The positive control, in both spellings and both shapes, so an empty
        // result below is a property of the corpus and not of this walker.
        for (ty, body) in [
            (
                "io.kestra.plugin.core.flow.Parallel",
                "    tasks:\n      - id: on\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n        runIf: \"false\"\n      - id: off\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "core.parallel",
                "    tasks:\n      - id: on\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n        runIf: \"false\"\n      - id: off\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "io.kestra.plugin.core.flow.Dag",
                "    tasks:\n      - task:\n          id: on\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n          runIf: \"false\"\n      - task:\n          id: off\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
            ),
            (
                "core.dag",
                "    tasks:\n      - task:\n          id: on\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n          runIf: \"false\"\n      - task:\n          id: off\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
            ),
        ] {
            let probe: serde_json::Value = serde_norway::from_str(&format!(
                "id: probe\nnamespace: io.kestra.tests\ntasks:\n  - id: p\n    type: {ty}\n{body}"
            ))
            .expect("the probe parses");
            let mut found = Vec::new();
            walk(&probe, &mut found);
            assert_eq!(
                found,
                ["on".to_string()],
                "{ty}: the walker must find the child with a runIf and not the other"
            );
        }

        let mut offenders: Vec<String> = Vec::new();
        let mut with_run_if = 0usize;
        for (id, source) in every_corpus_flow() {
            if source.contains("runIf") {
                with_run_if += 1;
            }
            let Ok(doc) = serde_norway::from_str::<serde_json::Value>(&source) else {
                continue;
            };
            let mut here = Vec::new();
            walk(&doc, &mut here);
            offenders.extend(here.into_iter().map(|t| format!("{id}/{t}")));
        }
        println!("corpus flows declaring a runIf anywhere: {with_run_if}");
        assert_eq!(
            with_run_if, 9,
            "the population this is a statement about; if it moves, re-derive \
             the `runIf:` numbers in oracle-exec/README.md in the same commit"
        );
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "a corpus `Parallel` or `Dag` child declares a `runIf:`, so the two \
             SKIPPED rules W3c widened are now judged by a golden entry rather \
             than by Kestra's Java alone. Re-read `parallel::drive_concurrent` \
             rule 1 and `dag::drive`'s predicate against it"
        );
    }

    /// **No corpus flow writes an explicit `runIf: null`**, which is the one
    /// spelling W1's parser and Kestra might read differently.
    ///
    /// `optional()` maps an explicit null to `None`, i.e. to "no `runIf:`",
    /// which runs the task. Java's `TruthUtils.isTruthy(null)` is `false`,
    /// which would skip it — but whether Jackson ever hands `getRunIf()` a null
    /// past the `@Builder.Default` was not measured, and guessing either way
    /// would be inventing a semantics. Nothing reaches the question, and this
    /// is what says so.
    #[test]
    fn no_corpus_flow_writes_a_null_run_if() {
        fn walk(node: &serde_json::Value, out: &mut Vec<String>) {
            if let serde_json::Value::Object(map) = node {
                if map.get("runIf").is_some_and(|v| v.is_null()) {
                    out.push(map["id"].as_str().unwrap_or("<no id>").to_string());
                }
                for v in map.values() {
                    walk(v, out);
                }
            } else if let serde_json::Value::Array(items) = node {
                for v in items {
                    walk(v, out);
                }
            }
        }

        // The control.
        let probe: serde_json::Value = serde_norway::from_str(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  - id: n\n    \
             type: io.kestra.plugin.core.debug.Return\n    format: x\n    runIf: null\n  \
             - id: y\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n    runIf: \"false\"\n",
        )
        .expect("the probe parses");
        let mut found = Vec::new();
        walk(&probe, &mut found);
        assert_eq!(found, ["n".to_string()]);

        let mut offenders: Vec<String> = Vec::new();
        for (id, source) in every_corpus_flow() {
            let Ok(doc) = serde_norway::from_str::<serde_json::Value>(&source) else {
                continue;
            };
            let mut here = Vec::new();
            walk(&doc, &mut here);
            offenders.extend(here.into_iter().map(|t| format!("{id}/{t}")));
        }
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "a corpus flow writes `runIf: null`, so the divergence named on \
             `models::Task::run_if` is now reachable and has to be measured \
             against the artifact rather than left named"
        );
    }

    /// **A `runIf:` is re-asked on every attempt, and a retry is what makes
    /// that observable.**
    ///
    /// Kestra renders `runIf:` at the point it emits a worker task
    /// (`ExecutionEventMessageHandler.java:269`), and a retried task is emitted
    /// again — so attempt 2 is decided by attempt 2's render, not attempt 1's.
    ///
    /// [`retry::reopen_due`] returns a run to `CREATED`, which re-enters
    /// [`flowable::open`] and therefore `run_if_verdict`. Without clearing the
    /// slot, `TaskRun::rendered_run_if` still holds attempt 1's answer and
    /// `advance` never asks again — a narrower instance of exactly the
    /// over-acceptance W3c exists to close, because a `runIf:` that has become
    /// falsy would still run.
    ///
    /// **`rendered_run_if` is the first render slot that can sit on a retried
    /// run**, which is why this is W3c's and not pre-existing:
    /// [`retry::scan`] only retries a [`TaskKind::Runnable`], and a `Runnable`
    /// never carried the older `rendered` slot (that is an `If`/`Switch`/`Loop`
    /// field).
    ///
    /// No corpus flow has both a `retry:` and a `runIf:`
    /// (`no_corpus_flow_has_both_a_retry_and_a_run_if` derives it), so the
    /// ratchet is blind to this and the test is what holds it.
    #[test]
    fn a_run_if_is_re_asked_on_every_attempt() {
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: a\n    \
             runIf: \"{{ go }}\"\n    retry:\n      type: constant\n      maxAttempts: 3\n      \
             interval: PT1S\n",
        )
        .expect("the probe plans");
        let mut state = ExecutionState::new(&plan);

        // Attempt 1: create, answer TRUE, run, fail.
        round(&plan, &mut state);
        assert_eq!(wire(&state.runs[0].history), ["CREATED"]);
        // The execution takes RUNNING on the round after it has a run; that is
        // the only thing left to emit while the `runIf:` is unanswered.
        assert_eq!(
            round(&plan, &mut state),
            vec![Command::SetExecutionState {
                state: RunState::Running
            }]
        );
        assert_eq!(
            advance(&plan, &state, T0),
            Vec::new(),
            "unrendered: nothing to decide"
        );
        state.runs[0].rendered_run_if = Some("true".to_string());
        round(&plan, &mut state);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "SUBMITTED", "RUNNING"]
        );
        apply(
            &mut state,
            &Command::SetRunState {
                run: 0,
                state: RunState::Failed,
            },
        );

        // The retry pass schedules attempt 2 and the clock moves to it.
        round(&plan, &mut state);
        assert_eq!(state.runs[0].history.last(), Some(&RunState::Retrying));
        let due = state.runs[0].not_before.expect("a retry was scheduled");
        for cmd in &advance(&plan, &state, due) {
            apply(&mut state, cmd);
        }
        assert_eq!(
            state.runs[0].history.last(),
            Some(&RunState::Created),
            "reopen_due returns the run to CREATED, which re-enters `open`"
        );

        // THE POINT: attempt 1's answer must not decide attempt 2.
        assert_eq!(
            state.runs[0].rendered_run_if, None,
            "a run that has re-entered CREATED has not been rendered for this \
             attempt; leaving attempt 1's answer in the slot is `advance` \
             deciding attempt 2 from a stale render"
        );
        assert_eq!(
            advance(&plan, &state, due),
            Vec::new(),
            "and so there is nothing to decide until the caller answers again"
        );

        // The caller now says FALSE, and attempt 2 is skipped rather than run.
        state.runs[0].rendered_run_if = Some("false".to_string());
        let cmds = advance(&plan, &state, due);
        assert_eq!(
            cmds,
            vec![Command::SkipRun { run: 0, at: due }],
            "attempt 2's own answer decides attempt 2"
        );

        // The control, on the same state: a caller that answers TRUE again gets
        // attempt 2 started, so the assertion above is about the ANSWER and not
        // about a retried run being unstartable.
        let mut truthy = state.clone();
        truthy.runs[0].rendered_run_if = Some("true".to_string());
        assert!(advance(&plan, &truthy, due)
            .iter()
            .any(|c| matches!(c, Command::StartRun { .. })));
    }

    /// **No corpus flow has both a `retry:` and a `runIf:`**, which is why
    /// `a_run_if_is_re_asked_on_every_attempt` is a committed test rather than
    /// a banked golden entry.
    ///
    /// The two corpus flows that write `runIf: "{{taskrun.attemptsCount == 0}}"`
    /// — `valids/restart-with-finally` and `…-after-execution` — are exactly the
    /// shape this would matter for, and neither declares a `retry:`, so the
    /// expression is constant across the one attempt they take.
    #[test]
    fn no_corpus_flow_has_both_a_retry_and_a_run_if() {
        let mut with_run_if = 0usize;
        let mut both: Vec<String> = Vec::new();
        for (id, source) in every_corpus_flow() {
            let has_run_if = source.lines().any(|l| l.trim_start().starts_with("runIf:"));
            if has_run_if {
                with_run_if += 1;
            }
            if has_run_if && source.lines().any(|l| l.trim_start().starts_with("retry:")) {
                both.push(id);
            }
        }
        assert_eq!(
            with_run_if, 9,
            "the control: this must find the flows that declare a `runIf:` at all"
        );
        assert_eq!(
            both,
            Vec::<String>::new(),
            "a corpus flow now has both, so the re-ask rule is judged by a golden \
             entry rather than by `a_run_if_is_re_asked_on_every_attempt` alone"
        );
    }

    /// **Only a `Runnable` can ever re-enter `CREATED`**, which is why
    /// [`apply`]'s rule 2 clearing [`TaskRun::rendered`] alongside
    /// `rendered_run_if` is unreachable today — and why that half is kept
    /// rather than deleted.
    ///
    /// The mutation that removes the `rendered` half (R2-M16) kills nothing,
    /// and this is the derivation that says why instead of leaving it as an
    /// argument. The chain is structural, not corpus-dependent:
    /// [`retry::scan`] gates on `matches!(kind, TaskKind::Runnable { .. })`, so
    /// only a `Runnable` is ever put into `RETRYING` or given a `not_before`;
    /// [`retry::reopen_due`] reopens exactly the runs that have one; and a
    /// `Runnable` never has a `rendered`, because that slot belongs to an
    /// `If`'s condition, a `Switch`'s value and a `Loop`'s `values`.
    ///
    /// So the code path is provably unreached, and the crate now carries the
    /// test that says so — the treatment `an_errors_list_inside_a_loop_body_is_a_shape_the_corpus_does_not_reach`
    /// already established here. **Anyone who lets a flowable be retried should
    /// expect R2-M16 to start killing something**, and should want it to.
    #[test]
    fn only_a_runnable_can_re_enter_created() {
        // A `Sequential` carrying its OWN `retry:`, whose child fails. Kestra
        // retries the CHILD (`policy_for` walks task -> ancestor -> flow), and
        // the `Sequential` itself is never retried however its child ends.
        let plan = build_plan(
            "id: probe\nnamespace: io.kestra.tests\ntasks:\n  \
             - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    \
             retry:\n      type: constant\n      maxAttempts: 3\n      interval: PT1S\n    \
             tasks:\n      - id: child\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n",
        )
        .expect("the probe plans");

        let mut state = ExecutionState::new(&plan);
        let mut reopened: Vec<String> = Vec::new();
        let mut now = T0;
        for _ in 0..200 {
            let cmds = advance(&plan, &state, now);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    if let Command::SetRunState {
                        run,
                        state: RunState::Created,
                    } = cmd
                    {
                        reopened.push(plan.tasks[state.runs[*run].task].id.clone());
                    }
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
                        state: RunState::Failed,
                    },
                ),
                None => match state
                    .runs
                    .iter()
                    .filter_map(|r| r.not_before)
                    .filter(|t| *t > now)
                    .min()
                {
                    Some(t) => now = t,
                    None => break,
                },
            }
        }

        // The positive control: something WAS reopened, so an empty
        // non-Runnable list below is a fact about kinds and not about a flow
        // that never retried at all.
        assert_eq!(
            reopened,
            ["child", "child"],
            "the child is a `Runnable` with a maxAttempts of 3, so it is \
             reopened twice; if this is empty the assertion below proves nothing"
        );
        let non_runnable: Vec<&String> = reopened
            .iter()
            .filter(|id| {
                !plan
                    .tasks
                    .iter()
                    .any(|t| &&t.id == id && matches!(t.kind, TaskKind::Runnable { .. }))
            })
            .collect();
        assert!(
            non_runnable.is_empty(),
            "a non-`Runnable` was returned to CREATED: {non_runnable:?}. \
             `apply`'s rule 2 clearing `rendered` is reachable now, and R2-M16 \
             should start killing something"
        );
        // And the `Sequential` itself never went RETRYING, which is the same
        // fact read off `scan`'s gate rather than off the reopen commands.
        assert!(
            !state.runs[0].history.contains(&RunState::Retrying),
            "`scan` gates on `TaskKind::Runnable`, so a flowable is never retried"
        );
        assert!(state.runs[0].not_before.is_none());
    }
}
