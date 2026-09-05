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
//! the one flowable `sequential` — which is also the flow's top-level `tasks:`,
//! since Kestra runs that list by the same rule. Every other task type is
//! planned as an opaque [`TaskKind::Runnable`]; see [`build_plan`] for why that
//! is the safe default rather than an approximation.
//!
//! Tasks 4-8 add `parallel`, `dag`, `if`/`switch`, `loop` and the attempt
//! lifecycle. They add variants to [`TaskKind`] and [`Command`]; they do not
//! change the signatures here.

mod command;
mod flowable;
mod plan;
mod state;

pub use command::Command;
pub use plan::{
    build_plan, Plan, PlanError, PlanTask, RetryBehavior, RetryPolicy, TaskIx, TaskKind,
};
pub use state::{apply, Attempt, ExecutionState, RunIx, RunState, TaskRun, Tick};

use flowable::sequential::{self, Step};

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
        let plan = plan_of("valids/sequential");
        let state = run_to_fixpoint(&plan, RunState::Success);

        let mut flowables = 0;
        let mut runnables = 0;
        for run in &state.runs {
            match plan.tasks[run.task].kind {
                TaskKind::Sequential { .. } => {
                    flowables += 1;
                    assert_eq!(
                        wire(&run.history),
                        ["CREATED", "RUNNING", "SUCCESS"],
                        "flowable {} must not pass through SUBMITTED",
                        plan.tasks[run.task].id
                    );
                }
                TaskKind::Runnable { .. } => {
                    runnables += 1;
                    assert_eq!(
                        wire(&run.history),
                        ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
                        "runnable {} must pass through SUBMITTED",
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
        assert!(
            flowables >= 3 && runnables >= 6,
            "valids/sequential must exercise both sides; saw {flowables} flowable(s) \
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
    /// not only the one Task 3 descends — so the bound already covers Tasks
    /// 4-8.
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
            "io.kestra.plugin.core.flow.Parallel",
            "io.kestra.plugin.core.flow.Loop",
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
