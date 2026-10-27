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

pub(crate) mod allow_failure;
mod command;
pub mod exit;
mod flowable;
pub(crate) mod kill;
mod plan;
mod retry;
mod state;
pub(crate) mod update;

pub use command::Command;
pub use plan::{
    build_plan, loop_until_gap, pause_gap, plan_stored, DagNode, Plan, PlanError, PlanTask,
    RetryBehavior, RetryDelay, RetryPolicy, TaskIx, TaskKind,
};
pub use state::{
    apply, Attempt, ExecutionMeta, ExecutionState, Resume, RunIx, RunState, TaskRun, Tick,
    WorkingDirPhase,
};

/// Kestra's `FlowableUtils.resolveValues`, over a rendered `Loop.values`.
///
/// Public because two callers outside this module need the same answer and must
/// not each write their own: `tests/executor.rs` derives what a first batch
/// should be from it, and W3b will need it to schedule the batches after the
/// first. See [`flowable::loop_::iteration_values`] for the rule and for the one
/// place it is knowingly wider than Kestra.
pub use flowable::loop_::{
    iteration_values, rendered_from_file, resolve_loop_values, values_name_a_file, LoopValues,
};

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

/// A manual resume: see [`flowable::pause::release_now`].
pub(crate) use flowable::pause::release_now as release_paused_pauses;
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

/// Which iteration a [`TaskKind::LoopUntil`] run is in — Kestra's
/// `iterationCount`, 1-based.
///
/// Public for [`iteration_values`]'s reason, and the second caller is the one
/// that matters: `worker::render` puts this number in front of the loop's
/// children as `{{ outputs.<loop>.iterationCount }}`, which three corpus flows
/// are written around. Two derivations of "which iteration is this" — one for
/// the decision, one for the expression the decision is made from — is the
/// shape that lets a loop exit on a number no child ever saw.
pub use flowable::loop_until::iteration as loop_iteration;

/// Whether a [`TaskKind::LoopUntil`] run's `condition:` is being asked this
/// round — the gate `worker::render`'s tier B shares with the executor.
///
/// Public for the same reason, and the sharing is the point: Kestra evaluates a
/// `LoopUntil`'s condition in exactly one place behind exactly two guards, and
/// a render tier with its own opinion about when would fail runs on an
/// expression the artifact never evaluated. See
/// [`flowable::loop_until::condition_is_asked`].
pub use flowable::loop_until::condition_is_asked as loop_condition_is_asked;

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
/// | [`TaskRun::rendered`](crate::executor::TaskRun::rendered) | `CREATED` for a `Loop`, `RUNNING` for an `If`/`Switch`/`Pause` | that flowable's own expression — Tasks 5-6, and W8 Task 3's `pauseDuration:` |
///
/// **`Pause` is the fourth kind to use the second slot and it needed no new
/// field**, which is the whole of W8 Task 3's answer to "does the render slot
/// change `advance()`'s contract": it does not. The contract is *(plan, state,
/// now) -> commands* with the caller filling two named fields, and a
/// `pauseDuration:` expression is a fourth reader of a field that already
/// exists, at a state that is already in the table. A `Pause` whose
/// `pauseDuration:` is a literal never enters the population at all — see
/// [`flowable::pause`] — so the flows that stall on an unanswered render are
/// exactly the flows whose author wrote an expression.
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
    if !drives_forward(state) {
        // A kill in progress (spec §5): nothing is created, started, retried
        // or resumed, and nothing below -- `reopen_due`, the walk,
        // `retry::scan`, `pause_mark` -- sees this state. See [`kill`]. A
        // terminal execution gets nothing at all.
        return if state.history.last() == Some(&RunState::Killing) {
            kill::advance_killing(plan, state)
        } else {
            Vec::new()
        };
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
    // End hooks A4: the flow's own `finally:` is the third list of the same
    // rule (`ES:504-512`, `ES:1025-1050` call `findTaskDependingFlowState` for
    // the flow too); the execution's state is still `aggregate(roots)`.
    let mut work = match sequential::drive(
        plan,
        state,
        &plan.roots,
        &plan.errors,
        &plan.finally,
        None,
        now,
    ) {
        Step::Commands(cmds) => cmds,
        Step::Waiting => Vec::new(),
        Step::Done(state) => vec![Command::SetExecutionState { state }],
    };

    // **An `Exit` that decided `KILLED` is the whole round** (final review C1).
    // `parallel::drive_concurrent` collects every child's commands in one
    // batch, so the round an `Exit` kills in can also create a sibling
    // branch's next run or start a `CREATED` sibling. `exit::kill` read the
    // state at the start of the round and cannot have killed those: applied,
    // a `CREATED` leaf would wait under a `KILLING` execution for ever (no
    // claim takes it), and a start applied after the kill's `KILLED` would
    // leave a `RUNNING` run no worker should run. Kestra runs the updating
    // task before the flowables (`ExecutorService.handleExecutionUpdatingTasks`),
    // so the `Exit`'s commands are emitted alone, recomputed from the same
    // state, and the next round -- under `KILLING` -- is `advance_killing`'s.
    if let Some(run) = exit::killed_in(plan, state, &work) {
        return exit::kill(plan, state, run);
    }

    // `handleFlowableTasks`'s retry pass, which runs after `handleEnd` and is
    // therefore behind whatever state the execution just took. That order is
    // visible: `valids/retry-failed-task-attempts`'s execution reads
    // `… FAILED RETRYING …`, never `… RETRYING FAILED …`.
    work.extend(retry::scan(plan, state, now));

    // **The execution's `PAUSED`/`RESTARTED`, answered once for the whole
    // batch.** W8 Task 3.
    //
    // A pause writes its own run's state and nothing else
    // ([`flowable::pause`]); the execution's half is here because a `Parallel`
    // of two `Pause`es enters both in ONE batch, and an emission per run puts
    // `PAUSED` into the execution's history twice. No golden execution history
    // repeats a state, for any construct, in any of its 408 entries.
    //
    // **What is measured and what is chosen.** That a paused execution reads
    // `PAUSED` and a resumed one `RESTARTED` is measured — 18 and 12 of the
    // golden's execution histories, shape `CREATED RUNNING PAUSED RESTARTED
    // RUNNING …`. What Kestra does with TWO pauses is not: no corpus flow
    // declares two in one execution
    // (`the_corpus_has_no_execution_with_two_pauses`), so answering once is a
    // choice against an unmeasured artifact, taken because a repeated state is
    // a shape nothing in the golden has.
    if let Some(mark) = pause_mark(state, &work) {
        if state.history.last() != Some(&mark) {
            work.insert(0, Command::SetExecutionState { state: mark });
        }
    }

    // # `not_running_yet` is Kestra's `isCreated()`, and it is narrower by `RESTARTED`
    //
    // **W8 Task 2, and its population is 12 rather than 0.** Kestra's
    // `State.Type.isCreated()` is `CREATED || RESTARTED`, and
    // `ExecutorService.handleRestart` maps a `RESTARTED` execution straight to
    // `RUNNING` — which is exactly what the arm below does for `CREATED`. All
    // twelve of the golden's `RESTARTED` execution histories read
    // `CREATED RUNNING PAUSED RESTARTED RUNNING …`, so the transition this arm
    // would have to make is measured, not hypothetical.
    //
    // **W8 Task 3 widened it, and is the producer Task 2 said would have to.**
    // `flowable::resume_paused` ends a bounded pause's wait, and the
    // `SetExecutionState { Restarted }` that owes is `pause_mark`'s from the
    // resulting batch, so the arm below now reads Kestra's
    // `isCreated()` exactly. Without the `Restarted` half a resumed execution
    // stops at `CREATED RUNNING PAUSED RESTARTED` and never re-enters
    // `RUNNING`; `a_resumed_pause_re_enters_running` is the test, and reverting
    // this one term is mutation M-P3.
    //
    // The sibling rule — `apply`'s rule 2, which clears a run's renders on
    // re-entering `CREATED` — is narrow for a *different* reason and the two
    // must not be quoted with one sentence: there the population really is
    // **0 of 840 run histories**, because `RESTARTED` is execution-only in this
    // corpus. Mutating rule 2 to include `Restarted` survives the whole suite;
    // mutating this one will not, once a producer exists.
    let not_running_yet = matches!(
        state.history.last(),
        Some(&RunState::Created) | Some(&RunState::Restarted)
    );
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

/// **Whether [`advance`] still drives this execution's runs forward**: it
/// is neither terminal nor `KILLING`. Every other state -- `RUNNING`,
/// `PAUSED`, `RETRYING`, `CREATED`, `RESTARTED` -- reaches the walk, which
/// creates, starts and completes runs. `advance` branches on exactly this,
/// and the worker's update pass (`worker::render::render_updates`, group C2
/// ruling D9) gates on it, so the two cannot disagree about when a sibling
/// may still move.
pub fn drives_forward(state: &ExecutionState) -> bool {
    !state
        .history
        .last()
        .is_some_and(|s| s.is_terminal() || *s == RunState::Killing)
}

/// Which execution state, if any, this batch of commands owes — W8 Task 3.
///
/// `Paused` if the batch enters a pause, `Restarted` if it ends one. Read off
/// the commands rather than off the state, because both facts are decisions
/// [`flowable::pause`] has already taken and re-deriving them here would be a
/// second opinion about which runs are pausing.
///
/// **A batch cannot owe both**, and that is structural rather than lucky: a run
/// that is `RUNNING` this round is entered, a run that is `PAUSED` is resumed,
/// and `sequential::walk` returns at the FIRST child that wants something while
/// `parallel::drive_concurrent`'s rule 3 reaches a given run through exactly
/// one arm. `Paused` is tested first so that a future caller which did produce
/// both would pause rather than resume, which is the half that cannot
/// over-accept.
fn pause_mark(state: &ExecutionState, work: &[Command]) -> Option<RunState> {
    if work.iter().any(|c| matches!(c, Command::PauseRun { .. })) {
        return Some(RunState::Paused);
    }
    // **A `LoopUntil` ending a pause owes `RUNNING`, not `RESTARTED`** -- W8
    // Task 4, and it is measured rather than chosen. Of the golden's 18
    // `PAUSED` execution histories, 12 carry a `RESTARTED` and read
    // `CREATED RUNNING PAUSED RESTARTED RUNNING ...`; the other 6 carry none at
    // all and read `CREATED RUNNING PAUSED RUNNING PAUSED RUNNING ...`. The
    // first 12 are every `Pause` that was resumed and the last 6 are every
    // `LoopUntil` that iterated, so the two constructs leave `PAUSED` by
    // different doors and the command each emits is what separates them.
    // Reading the command rather than the run's kind keeps this function what
    // its doc says it is: a report of decisions the flowables already took.
    if work
        .iter()
        .any(|c| matches!(c, Command::RestartLoop { .. }))
    {
        return Some(RunState::Running);
    }
    // "This batch ends a pause" is read as "it sets the state of a run that is
    // `PAUSED` right now", which is exact: `Command::PauseRun` is the only
    // producer of that state and `flowable::resume_paused` is the only thing
    // that moves a run out of it. Matching on the command's own state instead
    // would have to guess, because a pause ends `SUCCESS` or `FAILED`
    // depending on which bound was written and both are ordinary terminals
    // everywhere else.
    work.iter()
        .any(|c| match c {
            Command::SetRunState { run, .. } => state
                .runs
                .get(*run)
                .and_then(|r| r.history.last())
                .is_some_and(|s| *s == RunState::Paused),
            _ => false,
        })
        .then_some(RunState::Restarted)
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
    ///
    /// **It is [`plan::flatten_flow`], not `build_plan`, since no-silent-stall
    /// Task 1**: the third gate now refuses 52 flows this Kestra ran (an
    /// `OutputValues` child, an `AllowFailure`, ...), and a test of `advance()`
    /// over one of them -- `valids/waitfor` -- is a test of the executor, which
    /// is downstream of the gate. For every flow `build_plan` plans the two
    /// answers are the same plan.
    fn plan_of(corpus_id: &str) -> Plan {
        let flow = crate::spec::parse(&corpus_source(corpus_id))
            .unwrap_or_else(|e| panic!("cannot parse {corpus_id}: {e:?}"));
        plan::flatten_flow(&flow).unwrap_or_else(|e| panic!("cannot flatten {corpus_id}: {e}"))
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

    /// One flow's layout as D3 freezes it: index, id, kind, and the children
    /// `Plan::children_of` returned before package #2.
    fn layout(plan: &Plan) -> Vec<(usize, String, String, Vec<usize>)> {
        (0..plan.tasks.len())
            .map(|ix| {
                let kind = format!("{:?}", plan.tasks[ix].kind);
                let tag = kind.split([' ', '{', '(']).next().unwrap_or("").to_string();
                (ix, plan.tasks[ix].id.clone(), tag, plan.children_of(ix))
            })
            .collect()
    }

    /// **D3: a plan built after package #2 starts with the layout every stored
    /// `task_runs.task_ix` was written against** (`worker/claim.rs:538`, read
    /// back at `:4393`). A list #2 adds may only be APPENDED: an execution that
    /// was running at deploy time is re-planned from its own source at its next
    /// claim, and a shifted index would hand its runs another task's spec.
    /// `EGRET_WRITE_PLAN_LAYOUT=1` rewrites the fixture; that is allowed only in
    /// Task A1, at `68c9f4de`, before any planner change.
    #[test]
    fn the_plan_before_end_hooks_is_a_prefix_of_every_plan_after() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/executor/plan_layout_pre_end_hooks.json"
        );
        let mut now = serde_json::Map::new();
        for (id, source) in every_corpus_flow() {
            if let Ok(plan) = plan::plan_stored(&source) {
                now.insert(id, serde_json::to_value(layout(&plan)).unwrap());
            }
        }
        if std::env::var("EGRET_WRITE_PLAN_LAYOUT").as_deref() == Ok("1") {
            std::fs::write(path, serde_json::to_string_pretty(&now).unwrap()).unwrap();
        }
        let frozen: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(
            frozen.len() >= 300,
            "the fixture covers the corpus ({})",
            frozen.len()
        );
        let mut moved = Vec::new();
        for (id, old) in &frozen {
            let old: Vec<(usize, String, String, Vec<usize>)> =
                serde_json::from_value(old.clone()).unwrap();
            let Some(new) = now.get(id) else {
                moved.push(format!("{id}: planned before #2, not now"));
                continue;
            };
            let new: Vec<(usize, String, String, Vec<usize>)> =
                serde_json::from_value(new.clone()).unwrap();
            for o in &old {
                // A node keeps its index, id and kind, and every old child is still
                // its child (a new `finally` or a loop's new `errors` only adds some).
                match new.get(o.0) {
                    Some(n) if n.1 == o.1 && n.2 == o.2 && o.3.iter().all(|c| n.3.contains(c)) => {}
                    other => moved.push(format!("{id}: ix {} was {o:?}, is {other:?}", o.0)),
                }
            }
        }
        assert!(moved.is_empty(), "{moved:#?}");
    }

    /// W7c Task 1: the topology graph over the whole corpus. For every flow
    /// that parses, `graph::flow_graph` answers `Ok`, and every task the
    /// executor's plan holds (`plan_stored`: parse + flatten, no validation,
    /// where the flow flattens) is exactly one task node. The graph walks the
    /// parsed model and the plan the flattened one, so this is the check that
    /// the two walks see the same tasks.
    #[test]
    fn every_corpus_flow_graphs_and_every_planned_task_is_one_node() {
        use crate::graph::{flow_graph, NodeKind};
        let mut parsed = 0;
        let mut judged = 0;
        let mut tasks_judged = 0;
        for (id, source) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&source) else {
                continue;
            };
            parsed += 1;
            let graph = flow_graph(&flow, None).unwrap_or_else(|e| panic!("{id}: {e}"));
            let Ok(plan) = plan::plan_stored(&source) else {
                continue;
            };
            judged += 1;
            for task in &plan.tasks {
                let nodes = graph
                    .nodes
                    .iter()
                    .filter(|n| matches!(&n.kind, NodeKind::Task { task_id, .. } if *task_id == task.id))
                    .count();
                // An id the plan holds once is exactly one node. An id it
                // holds k times (`invalids/duplicate`, refused but flattened)
                // is k nodes, each with its own uid (`~2`, ...).
                let planned = plan.tasks.iter().filter(|t| t.id == task.id).count();
                assert_eq!(
                    nodes, planned,
                    "{id}: planned task '{}' is {nodes} nodes, planned {planned} times",
                    task.id
                );
                tasks_judged += 1;
            }
        }
        // The population, measured when this test was written (W7c Task 1):
        // 339 flows parse, 336 of them flatten, holding 808 planned tasks. A
        // floor, not an equality, so adding a corpus flow does not fail it; a
        // sweep that judged nothing does.
        assert!(parsed >= 339, "parsed only {parsed} flows");
        assert!(
            judged >= 336,
            "judged only {judged} flows ({parsed} parsed)"
        );
        assert!(tasks_judged >= 808, "judged only {tasks_judged} tasks");
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
            // W8 Task 4. `valids/waitfor`'s condition is truthy on its first
            // iteration, so it is the one corpus `LoopUntil` that terminates
            // without ever pausing -- which is what lets this fixture, whose
            // clock never moves, assert the discriminator on it directly
            // rather than through a panicking arm.
            ("valids/waitfor", &[("waitfor", "true")][..]),
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
                    | TaskKind::Loop { .. }
                    | TaskKind::LoopUntil { .. }
                    | TaskKind::AllowFailure { .. }
                    | TaskKind::WorkingDirectory { .. } => {
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
                    // W8 Task 3's fourth tier. No flow in the list above
                    // declares one -- a `Pause` does not terminate under
                    // `run_to_fixpoint_rendering`, which never moves the clock
                    // -- and `a_pause_enters_paused_without_putting_it_on_the_
                    // attempt` asserts the same discriminator for it directly.
                    // The arm is here because the `match` is exhaustive and a
                    // catch-all would silently stop counting the next kind.
                    TaskKind::Pause { .. } => {
                        panic!(
                            "{corpus_id}: a `Pause` reached this fixture, which never \
                             moves the clock, so it cannot assert anything about one"
                        )
                    }
                    // Kill-and-exit Task 3. No flow in the list declares one;
                    // the arm is here for the exhaustiveness reason the
                    // `Pause` arm gives, and `executor::exit`'s tests assert
                    // its `CREATED RUNNING <state>` directly.
                    TaskKind::Exit { .. } => {
                        panic!("{corpus_id}: no flow in this fixture declares an `Exit`")
                    }
                    // Group C2 Task 3. No flow in the list declares one, and
                    // this fixture never sets `rendered` on one, so it would
                    // wait at `CREATED`; the update tests below assert its
                    // `CREATED RUNNING SUCCESS` directly.
                    TaskKind::ExecutionUpdate { .. } => {
                        panic!("{corpus_id}: no flow in this fixture declares an update task")
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

    /// **`CANCELLED` outranks `WARNING` and loses to `FAILED`** — W8 Task 6,
    /// and the row is Java's own priority rather than a preference.
    ///
    /// `Execution.guessFinalState`'s single pass is `KILLED > FAILED >
    /// CANCELLED > WARNING > PAUSED`
    /// (`core/.../executions/Execution.java:774-789`, each arm guarded on
    /// every higher one). Both neighbours are supplied, in both orders, because
    /// an arm pasted at the wrong end of the chain is right about one neighbour
    /// and wrong about the other — which is the only failure this ordering can
    /// have.
    ///
    /// **No corpus entry can supply a `CANCELLED` run** — `RunState::Cancelled`
    /// has one producer and it writes the EXECUTION's state, never a run's, and
    /// the only road to a run is a subflow/loop child that cancels, which no
    /// corpus flow declares. So this is the whole instrument for the arm.
    #[test]
    fn cancelled_outranks_warning_and_loses_to_failed() {
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("plans");
        for (a, b, expected) in [
            (RunState::Cancelled, RunState::Warning, RunState::Cancelled),
            (RunState::Warning, RunState::Cancelled, RunState::Cancelled),
            (RunState::Cancelled, RunState::Failed, RunState::Failed),
            (RunState::Failed, RunState::Cancelled, RunState::Failed),
            (RunState::Cancelled, RunState::Killed, RunState::Killed),
            (RunState::Killed, RunState::Cancelled, RunState::Killed),
            (RunState::Cancelled, RunState::Success, RunState::Cancelled),
            (RunState::Success, RunState::Cancelled, RunState::Cancelled),
            // The control for the arm below it, unchanged by this task: a
            // `WARNING` still beats a `SUCCESS`.
            (RunState::Warning, RunState::Success, RunState::Warning),
        ] {
            let state = two_terminal_children(&plan, a, b);
            let children = match &plan.tasks[0].kind {
                TaskKind::Parallel { tasks, .. } => tasks.clone(),
                other => panic!("the fixture's root is a Parallel, not {other:?}"),
            };
            assert_eq!(
                flowable::aggregate(&state, &children, Some(0)),
                expected,
                "children ({a:?}, {b:?})"
            );
        }
    }

    /// **The two copies of `guessFinalState` in this crate agree over every
    /// ordered pair of terminals** — W8 Task 6.
    ///
    /// [`flowable::aggregate`] serves `Sequential`/`If`/`Switch`/`LoopUntil`
    /// and `flowable::parallel::aggregate_created` serves `Parallel`/`Dag`.
    /// They are two transliterations of one Java method and they had **drifted
    /// by a whole variant**: W8 Task 5 added `Warning`'s rank to the first and
    /// not to the second, so a `Parallel` with a `WARNING` child answered
    /// `SUCCESS` where the artifact answers `WARNING`. Nothing could notice —
    /// the four corpus entries that put a `WARNING` under a flowable are
    /// baselined on the `AllowFailure` construct, so the ratchet is unmoved
    /// whichever answer is given — and it was found by W8 Task 6 enumerating
    /// every consumer of a terminality judgement before adding `Cancelled` to
    /// any of them.
    ///
    /// Asserting the two against **each other** rather than each against a
    /// literal is deliberate: a literal table would be a third copy. What this
    /// does not license is *"both are right"*; the row tests above are what
    /// pin the chain against Java.
    #[test]
    fn the_two_guess_final_state_copies_agree() {
        let plan = build_plan(
            "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
        )
        .expect("plans");
        let children = match &plan.tasks[0].kind {
            TaskKind::Parallel { tasks, .. } => tasks.clone(),
            other => panic!("the fixture's root is a Parallel, not {other:?}"),
        };
        let terminals: Vec<RunState> = RunState::ALL
            .iter()
            .copied()
            .filter(|s| s.is_terminal())
            .collect();
        assert_eq!(
            terminals.len(),
            7,
            "every terminal is driven; a filter that emptied would make the \
             loop below pass by running nothing"
        );
        let mut pairs = 0usize;
        for &a in &terminals {
            for &b in &terminals {
                let state = two_terminal_children(&plan, a, b);
                assert_eq!(
                    flowable::aggregate(&state, &children, Some(0)),
                    flowable::parallel::aggregate_created(&state, &[1, 2]),
                    "the two `guessFinalState` copies disagree on ({a:?}, {b:?})"
                );
                pairs += 1;
            }
        }
        assert_eq!(pairs, 49);
    }

    /// A `Parallel` whose two children have already terminated in `a` and `b`.
    ///
    /// Written by [`apply`] rather than by hand so that the runs it produces
    /// are the runs `advance()` would have produced — a state built field by
    /// field is a state no executor ever wrote.
    fn two_terminal_children(plan: &Plan, a: RunState, b: RunState) -> ExecutionState {
        let children = match &plan.tasks[0].kind {
            TaskKind::Parallel { tasks, .. } => tasks.clone(),
            other => panic!("the fixture's root is a Parallel, not {other:?}"),
        };
        assert_eq!(children.len(), 2);
        let mut state = ExecutionState::new(plan);
        apply(
            &mut state,
            &Command::CreateRun {
                task: plan.roots[0],
                parent: None,
                value: None,
            },
        );
        apply(&mut state, &Command::StartRun { run: 0, at: T0 });
        for (i, &child) in children.iter().enumerate() {
            apply(
                &mut state,
                &Command::CreateRun {
                    task: child,
                    parent: Some(0),
                    value: None,
                },
            );
            apply(
                &mut state,
                &Command::SetRunState {
                    run: i + 1,
                    state: if i == 0 { a } else { b },
                },
            );
        }
        state
    }

    /// **`concurrent:` caps how many children a `Parallel` creates at once.**
    ///
    /// Kestra's cap is on the *stream* of next task runs
    /// (`limit(concurrency - runningCount)` in `resolveParallelNexts`), so it
    /// bounds creation and not merely submission; the second batch arrives when
    /// the first has drained. Nothing in the 351-flow corpus exercises this
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

    /// **A node is created only once every task it `dependsOn` has
    /// TERMINATED, and the whole eligible batch is created at once.**
    ///
    /// The word was *succeeded* until W8 Task 5 fix round 2, in this doc and in
    /// this test's own name — and on `valids/dag`, whose every dependency
    /// succeeds, the two are the same assertion. The name is kept because
    /// renaming a test loses its history and the fixture really does only
    /// exercise successful dependencies; the three rows that separate the
    /// spellings are `a_dag_node_downstream_of_a_{skipped,warning,killed}
    /// _dependency_still_runs`.
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
    /// **This used to be offered as the reason the predicate's spelling was
    /// unobservable, and that reasoning is retired.** It said Kestra's
    /// `resolveDagNexts` filters on "all dependencies are **terminated**" while
    /// `dag::drive` said *succeeded*, and that no input could reach the
    /// predicate in a state that told them apart because rule 1 had already
    /// closed the branch. W8 Task 5 narrowed rule 1 to Kestra's `isFailed()`,
    /// which is precisely the dependency that argument rested on, and
    /// `dag::drive` says *terminated* now — the same word as the artifact.
    ///
    /// What this row still says, and it is the load-bearing half: **rule 1 does
    /// close the branch on a real `FAILED` dependency**, so nothing downstream
    /// is created and the `Dag` ends `FAILED` rather than stuck. It is the
    /// control for the three `…_still_runs` rows, which would otherwise pass on
    /// an implementation that had stopped closing branches at all.
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
            // W8 Task 4. This fixture's copy is deliberately WITHOUT the
            // children-executed gate `worker::render` applies: it supplies a
            // literal from a table rather than evaluating anything, so an early
            // render here cannot fail a run, and leaving the gate out keeps the
            // fixture from having a second opinion about the executor's own
            // predicate. The gate is asserted where it matters, in
            // `tests/worker.rs`.
            TaskKind::LoopUntil { condition, .. } => Some(condition.clone()),
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

    /// **A file's values are COUNTED in `rendered`, not listed, and every
    /// record counts** (W9a Task 11: the user's ruling of 2026-10-03 -- no
    /// `distinct()` on Kestra's URI path, `readLoopValuesFromUri`, `javap -c`
    /// of the artifact's jar -- and fix round 1: the records live in
    /// `workflow.loop_file_values`, read by index when spawned). The inline
    /// array is the control: the same four values, deduplicated.
    ///
    /// `open` spawns `min(limit, count)` iterations of a file loop, each with
    /// an empty value the spawner fills by index, and a file of no record
    /// succeeds without running, as an empty inline array does.
    #[test]
    fn a_files_values_are_counted_not_listed_and_an_inline_arrays_deduplicated() {
        let uri = "kestra:///n/f/executions/x/tasks/w/r/a.ion";
        let from_file = rendered_from_file(uri, 4);
        assert_eq!(from_file, format!("{uri}\n4"));
        assert_eq!(
            resolve_loop_values(&from_file).unwrap(),
            LoopValues::FromFile { count: 4 }
        );

        let inline = serde_json::to_string(&["1", "1", "{\"a\":1}", "1"]).unwrap();
        assert_eq!(
            resolve_loop_values(&inline).unwrap(),
            LoopValues::Listed(vec!["1".to_string(), "{\"a\":1}".to_string()])
        );

        // A URI the worker has not read is not a list of values, and a file's
        // values are never listed.
        assert!(resolve_loop_values(uri).is_err());
        assert!(iteration_values(&from_file).is_err());

        // `open`: `valids/loop-parallel-less` has `concurrencyLimit: 2`.
        let plan = plan_of("valids/loop-parallel-less");
        let (state, _) = spawn_round(&plan, "loop");
        let ix = (0..state.runs.len())
            .find(|&ix| plan.tasks[state.runs[ix].task].id == "loop")
            .unwrap();
        for (count, spawns, terminal) in
            [(4, 2, None), (1, 1, None), (0, 0, Some(RunState::Success))]
        {
            let mut state = state.clone();
            state.runs[ix].rendered = Some(rendered_from_file(uri, count));
            let cmds = advance(&plan, &state, T0);
            let spawned: Vec<&str> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::SpawnExecution { value, .. } => Some(value.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(spawned, vec![""; spawns], "count {count}: {cmds:?}");
            if let Some(t) = terminal {
                assert!(
                    cmds.iter()
                        .any(|c| matches!(c, Command::SetRunState { state, .. } if *state == t)),
                    "count {count}: {cmds:?}"
                );
            }
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
    /// read**, derived from all 351 flows on every run.
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
            "52 `Loop` tasks across the 351 corpus flows, of which 14 write a \
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
    /// So the difference is measured. Over all 351 corpus flows the wide
    /// reading names **88** tasks and the narrow one **59**; the 29 between them
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
        // (85, 57) -> (88, 59) at group C1 Task 3 (2026-10-18, measured):
        // `valids/if-in-parallel` plans again now that `Assert` is registered;
        // its `for_each` body is `return` and `if` (direct) and `after_if`.
        assert_eq!(
            (wide, narrow),
            (88, 59),
            "88 tasks are inside a loop by the shipped reading and 59 by the \
             direct-children one (85 and 57 until group C1 Task 3 registered \
             `Assert` and `valids/if-in-parallel` planned again). Plan Task 3 moved 78 and 55 by +7 and +2: \
             `valids/working-directory-taskrun` (one `WorkingDirectory` child, one \
             task inside it) and `-taskrun-nested` (five tasks inside its \
             `WorkingDirectory`, one direct child of the `Loop`) plan now, where \
             no-silent-stall had refused them. Before that, the same flows were \
             the `Loop`-holding ones it refused (88 and 63 until no-silent-stall \
             Task 1). The 28 between them are why this is a test: \
             `advance()` never calls `is_loop_body`, so nothing else in the crate \
             can tell the two apart. Both were one higher until the final \
             review implemented `disabled:`, which deletes \
             `valids/disable-flowable`'s `1-each-2` -- a direct child of a \
             `Loop` -- from the plan; the gap is unchanged by that, because the \
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
    /// parse**, over all 351 of them.
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
            // 345 -> 351 at WorkingDirectory Task 8: the six `working-directory-real-*`
            // flows (317 valids -> 323 valids + 28 invalids).
            351,
            "the corpus is 351 flows (323 valids + 28 invalids); if that has moved, \
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
    /// **"Planned" here means the flattening, not `build_plan`'s answer**, since
    /// no-silent-stall Task 1. Every construct this test classifies `Opaque`
    /// except a `Leaf` is now a validation ERROR (`unsupported_allow_failure`,
    /// `unsupported_working_directory`, `unsupported_loop_until_property`; the
    /// first two went with #680 and #683, which planned both as flowables --
    /// noted 2026-10-17, kill Task 2 fix round 1), so
    /// `build_plan`'s third gate answers `Err` for it, as it already did for a
    /// gapped `Pause`. The tiers are read off [`plan::flatten_flow`], the first
    /// two gates alone, and the refusal is asserted beside each.
    ///
    /// **This doc named those two and the loop below has always driven four**,
    /// which is the doc-contradicted-by-its-own-test shape.
    ///
    /// This doc then said the four rows are *"three `models::TaskKind`
    /// variants (`WaitFor` and `LoopUntil` are one), and they are **exactly**
    /// the typed variants `build_plan`'s `match` has no arm for"*. **W8 Task 1
    /// fix round 4 measured that and it was false**, by the probe-declares-its-
    /// arm assertion below: `models::kind_for` knew `"loopuntil"` and had no
    /// row for `"waitfor"` at all, so the four rows were **four** variants —
    /// `AllowFailure`, `WorkingDirectory`, `LoopUntil`, and `Leaf` for the
    /// `WaitFor` spelling.
    ///
    /// **W8 Task 4 made the sentence true by making the two spellings one**:
    /// `kind_for` has a `"waitfor"` row now, both probes declare `LoopUntil`,
    /// and the loop above is down to the two types that are still opaque.
    /// `LoopUntil` left that loop for the same reason `Pause` did — it is
    /// planned into a typed variant now, and per TASK rather than per type, so
    /// BOTH sides of `loop_until_gap` are probed below. `Subflow` was a fifth row
    /// until W6 Task 3 gave it [`TaskKind::Executable`]; a type that reaches
    /// `ModelKind::Leaf` instead (`Pause`) is opaque for a different reason
    /// and is not this test's subject.
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

        // **Two rows, not four: `WaitFor` and `LoopUntil` left this loop at W8
        // Task 4**, which gave them a `plan::TaskKind::LoopUntil` and flattened
        // their children. They are still driven below, by the classifier probe,
        // where they now declare the `LoopUntil` arm and the `Descends` tier.
        // `WorkingDirectory` left this loop at plan Task 3, the same way
        // `AllowFailure` did: it is planned as a flowable with its children in
        // the plan, and its probe is in the classifier test below.
        let ty = "io.kestra.plugin.core.flow.WorkingDirectory";
        let flow = crate::spec::parse(&branch(ty)).unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let plan = plan::flatten_flow(&flow).unwrap_or_else(|e| panic!("{ty}: {e}"));
        assert_eq!(
            plan.tasks.len(),
            2,
            "{ty} is a flowable, so its `inner` child is in the plan"
        );
        assert!(
            matches!(plan.tasks[0].kind, TaskKind::WorkingDirectory { .. }),
            "{ty} must plan as `TaskKind::WorkingDirectory`, got {:?}",
            plan.tasks[0].kind
        );

        // The YAML for the typed flowables whose shape `branch` cannot
        // produce. They are bound here rather than written inline at their
        // assertions below because the classifier probe loop drives the SAME
        // sources: a probe that has drifted away from the control it claims to
        // share a flow with is a probe that stopped testing what it says.
        let dag_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          id: inner\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n";
        let loop_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\"]'\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n";
        let if_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{ inputs.p }}\"\n    then:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n";
        let switch_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"{{ inputs.p }}\"\n    cases:\n      A:\n        - id: inner\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n";
        // The `Subflow` probe. It is NOT `branch`'s shape: a `Subflow` declares
        // `namespace:`/`flowId:` and takes no `tasks:` at all, which is the
        // corpus shape (`valids/flow-concurrency-subflow.yml`) and the whole
        // reason it lands in a tier of its own.
        let subflow_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: io.kestra.tests\n    flowId: child\n";
        // And a `Leaf`, so the arm that catches everything Kestra calls a task
        // is driven by a probe rather than assumed.
        let leaf_src = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n";

        // **m3: this test is called "the derivation" in three shipped docs, and
        // until W8 Task 1 fix round 3 it was four literal strings in a `for`.**
        // A new typed `models::TaskKind` variant with no `build_plan` arm would
        // be opaque, absent from the loop above, and invisible to exactly the
        // sweep method that found the stale lists. The match below is the
        // derivation the name claims: it is **exhaustive over
        // `models::TaskKind`**, so adding a variant is a COMPILE error here and
        // whoever adds it must classify it.
        //
        // # Fix round 4 (I1): it is three-valued, because `build_plan` is
        //
        // Round 3 shipped this classifier as a `bool` -- "are this kind's
        // children flattened into the plan?" -- asserted against
        // `plan.tasks.len() > 1` for five probes out of eleven arms. The arm
        // `K::Subflow(_) => true` was among the six never compared, and it is
        // **false**: `plan.rs`'s `Subflow` arm says so itself (*"It takes no
        // children: a `Subflow` runs a DIFFERENT flow"*), assigns
        // [`TaskKind::Executable`] and calls no `flatten_list`, so the plan
        // holds one task and the predicate is `false`. A short probe list
        // AGREES with a wrong table -- and only that direction is
        // consequential, because a shorter list matches every subset of the
        // table it certifies and nothing goes red. That is the same
        // lost-member asymmetry `plan.rs`'s census records, this time sitting
        // **inside the guard built to stop the previous instance of it**.
        //
        // The defect is a design gap and not a slip. `build_plan`'s outcome
        // space is **three-valued**:
        //
        // * [`TaskKind::Runnable`] -- opaque; the executor hands it to a
        //   worker. Two different reasons land here (a genuine leaf, and a
        //   Kestra flowable `build_plan` has no arm for), and this tier does
        //   not separate them because `build_plan` does not.
        // * [`TaskKind::Executable`] -- the executor runs it itself, and it
        //   takes no children because its children belong to a **different
        //   flow**. **W6 Task 3 created this tier** and `Subflow` is its only
        //   member, which is exactly why `Subflow` is the one construct a
        //   `bool` cannot express.
        // * a typed kind whose children were flattened into the plan.
        //
        // A bare `K::Subflow(_) => false` would have made the test green and
        // true under the old predicate, and would have classified `Subflow`
        // identically to the opaque four -- discarding the W6 Task 3
        // distinction this task exists to keep. It matters forward as well:
        // Kestra has further `ExecutableTask` implementations and the README's
        // `ExecutableUpdatableTask` row anticipates more arriving, so a `bool`
        // gate would force the next one into `true` or `false` with both
        // wrong.
        #[derive(Debug, PartialEq, Eq)]
        enum PlannedAs {
            /// [`TaskKind::Runnable`].
            Opaque,
            /// [`TaskKind::Executable`] -- W6 Task 3's middle tier.
            Executable,
            /// A typed kind; `build_plan` descended and flattened its children.
            Descends,
            /// A typed kind the executor drives ITSELF and whose children are
            /// not in the plan -- W8 Task 3's [`TaskKind::Pause`], since
            /// kill-and-exit Task 3 [`TaskKind::Exit`], and since group C2
            /// Task 3 [`TaskKind::ExecutionUpdate`].
            ///
            /// **A fourth tier, for the reason `Subflow` needed a third.** A
            /// `Pause` is not `Opaque` (no worker ever sees it and no `Pause`
            /// run in the golden carries `SUBMITTED`), not `Executable` (it
            /// spawns nothing), and not `Descends` (`plan.tasks.len()` is 1).
            /// Folding it into any of the three would discard exactly the
            /// distinction that decides whether a `type_id` is written, which
            /// is what decides whether the worker claims it at all.
            Waits,
        }

        // Exhaustive over the MODEL. Adding a `models::TaskKind` variant is a
        // compile error here, and the author must now say which of the three
        // tiers it is planned into rather than answer a yes/no question that
        // has no right answer for the middle one.
        //
        // # `Pause` is the one arm whose tier depends on the task's CONTENT
        //
        // Every other kind is planned into one tier whatever it says. A
        // `Pause` is planned [`TaskKind::Pause`] only when it declares none of
        // the four properties `build_plan` does not reproduce, and an opaque
        // `Runnable` otherwise -- so this arm has to ask the same question
        // `build_plan` asks, through the same `pause_gap`, or the two would be
        // two opinions about which pauses are reproduced.
        //
        // **That makes this ONE arm weaker than the others and it is said out
        // loud rather than left for a reader to assume.** A shared predicate
        // cannot catch a wrong predicate; what it still catches is a wrong
        // TIER, which is the thing this test is named for, and a `Pause`
        // probe is driven from BOTH sides of `pause_gap` below so that neither
        // outcome is the one nobody compares.
        fn classify(task: &crate::models::Task) -> PlannedAs {
            use crate::models::TaskKind as K;
            match &task.kind {
                K::Sequential(_)
                | K::Parallel(_)
                | K::If(_)
                | K::Switch(_)
                | K::Dag(_)
                | K::Loop(_) => PlannedAs::Descends,
                K::Subflow(_) => PlannedAs::Executable,
                K::Pause(p) if pause_gap(p).is_none() => PlannedAs::Waits,
                K::Pause(_) => PlannedAs::Opaque,
                // Kill-and-exit Task 3: an `Exit` is a `Leaf` to the model and
                // a kind the executor drives itself in the plan.
                K::Leaf { .. } if task.r#type == crate::executor::exit::TYPE_ID => PlannedAs::Waits,
                // Group C2 Task 3: the three update types, likewise.
                K::Leaf { .. } if crate::executor::update::UpdateOp::of(&task.r#type).is_some() => {
                    PlannedAs::Waits
                }
                // W8 Task 4: `LoopUntil` moved out of the opaque tier. Like
                // `Pause`, its tier depends on the task's CONTENT -- a bound
                // this pure builder cannot read plans it opaque instead
                // (`loop_until_gap`), and both sides of that predicate are
                // probed below.
                K::LoopUntil(l) if crate::executor::plan::loop_until_gap(l).is_none() => {
                    PlannedAs::Descends
                }
                K::LoopUntil(_) => PlannedAs::Opaque,
                K::AllowFailure(_) => PlannedAs::Descends,
                // Plan Task 3: the block is a flowable now, so it descends.
                K::WorkingDirectory(_) => PlannedAs::Descends,
                K::Leaf { .. } => PlannedAs::Opaque,
            }
        }

        // Exhaustive over the PLAN, and it is a second gate rather than a
        // restatement of the first: the tier is read off the plan VARIANT, not
        // off `plan.tasks.len() > 1`. The length predicate cannot tell
        // `Executable` from `Runnable` at all, and it would also call a
        // `Sequential` with an empty `tasks:` opaque. A new `plan::TaskKind`
        // variant is a compile error here too.
        fn observed(kind: &TaskKind) -> PlannedAs {
            match kind {
                TaskKind::Runnable { .. } => PlannedAs::Opaque,
                TaskKind::Executable { .. } => PlannedAs::Executable,
                TaskKind::Sequential { .. }
                | TaskKind::Parallel { .. }
                | TaskKind::If { .. }
                | TaskKind::Switch { .. }
                | TaskKind::Dag { .. }
                | TaskKind::Loop { .. }
                | TaskKind::LoopUntil { .. }
                | TaskKind::AllowFailure { .. }
                | TaskKind::WorkingDirectory { .. } => PlannedAs::Descends,
                TaskKind::Pause { .. }
                | TaskKind::Exit { .. }
                | TaskKind::ExecutionUpdate { .. } => PlannedAs::Waits,
            }
        }

        // The arm each kind falls into, so probe COVERAGE can be asserted
        // instead of counted. A figure ("all eleven are probed") beside a
        // derivation is the thing this task keeps correcting; a set compared
        // against a set is not.
        fn arm_name(kind: &crate::models::TaskKind) -> &'static str {
            use crate::models::TaskKind as K;
            match kind {
                K::Sequential(_) => "Sequential",
                K::Parallel(_) => "Parallel",
                K::If(_) => "If",
                K::Switch(_) => "Switch",
                K::Dag(_) => "Dag",
                K::Loop(_) => "Loop",
                K::LoopUntil(_) => "LoopUntil",
                K::Subflow(_) => "Subflow",
                K::AllowFailure(_) => "AllowFailure",
                K::WorkingDirectory(_) => "WorkingDirectory",
                K::Pause(_) => "Pause",
                K::Leaf { .. } => "Leaf",
            }
        }

        // Every one of the eleven arms is driven against `build_plan` here, and
        // each probe DECLARES the arm it means to reach, so that a probe which
        // does not reach it fails rather than passes.
        //
        // **That declaration found a false sentence on its first run, which is
        // why it is here and not a formality.** This test's own doc said the
        // four rows above are *"three `models::TaskKind` variants (`WaitFor`
        // and `LoopUntil` are one)"*. **They were not one until W8 Task 4.**
        // `models::kind_for` matched `Some("loopuntil")` and nothing else, and
        // `"waitfor"` appeared in **no** table in this crate -- not `kind_for`,
        // not `spec::validate`'s `KESTRA_FLOW_CONTROL` -- so
        // `io.kestra.plugin.core.flow.WaitFor` parsed to
        // [`crate::models::TaskKind::Leaf`], not to `LoopUntil`.
        //
        // Nothing above could have caught that, and neither could a
        // three-valued classifier on its own: `Leaf` and `LoopUntil` were both
        // planned [`TaskKind::Runnable`], so the plan was byte-identical either
        // way and every prose list saying *"`build_plan` plans `WaitFor`
        // opaque"* stayed true. Only a probe that says which arm it is aiming
        // at can see it -- and the divergence was real below the plan, because
        // a `Leaf` has no `children()` and a `LoopUntil` does.
        //
        // **Both probes declare `LoopUntil` now**, which is the assertion W8
        // Task 4's `kind_for` row had to satisfy: `LoopUntil.java` registers
        // `WaitFor` as an alias on itself, so one row serves both and a fix
        // that reached one spelling would fail here. The ratchet cannot see
        // that row at all -- deleting it leaves the oracle green -- so this
        // probe and the two `spec::validate` tests are the whole instrument.
        // The two `Pause` probes. **Both**, because `pause_gap` has two
        // outcomes and a probe list holding only one of them would leave the
        // other classified by nothing -- which is exactly the lost-member
        // asymmetry this test's own doc records one tier up. Since the
        // 2026-10-11 ruling the second outcome is not a TIER any more:
        // `validate_for_run` refuses it, so `build_plan` answers an error and
        // the probe is asserted for that below rather than driven through
        // the loop.
        let pause_plain = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n";
        let pause_with_behavior = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n    behavior: FAIL\n";
        // The other side of `loop_until_gap`, for the reason both sides of
        // `pause_gap` are probed: a list holding one outcome leaves the other
        // classified by nothing. `maxIterations:` is an expression here, which
        // a pure `build_plan` cannot read, so the task is planned opaque.
        let loop_until_unreadable = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{ true }}\"\n    checkFrequency:\n      maxIterations: \"{{ inputs.n }}\"\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n";

        let probes: [(&str, String); 14] = [
            (
                "AllowFailure",
                branch("io.kestra.plugin.core.flow.AllowFailure"),
            ),
            (
                "WorkingDirectory",
                branch("io.kestra.plugin.core.flow.WorkingDirectory"),
            ),
            // Kestra's alias, and since W8 Task 4 egret's model has a row for
            // it: BOTH spellings must declare `LoopUntil`, which is the
            // assertion the fix had to satisfy. See the paragraph above.
            ("LoopUntil", branch("io.kestra.plugin.core.flow.WaitFor")),
            ("LoopUntil", branch("io.kestra.plugin.core.flow.LoopUntil")),
            (
                "Sequential",
                branch("io.kestra.plugin.core.flow.Sequential"),
            ),
            ("Parallel", branch("io.kestra.plugin.core.flow.Parallel")),
            ("If", if_src.to_string()),
            ("Switch", switch_src.to_string()),
            ("Dag", dag_src.to_string()),
            ("Loop", loop_src.to_string()),
            ("Subflow", subflow_src.to_string()),
            ("Pause", pause_plain.to_string()),
            ("LoopUntil", loop_until_unreadable.to_string()),
            ("Leaf", leaf_src.to_string()),
        ];
        let mut arms_probed: BTreeSet<&'static str> = BTreeSet::new();
        for (declared, src) in &probes {
            let flow = crate::spec::parse(src).unwrap_or_else(|e| panic!("{declared}: {e:?}"));
            let plan = plan::flatten_flow(&flow).unwrap_or_else(|e| panic!("{declared}: {e}"));
            let arm = arm_name(&flow.tasks[0].kind);
            assert_eq!(
                arm, *declared,
                "this probe no longer reaches the `models::TaskKind` arm it was \
                 written for, so it is not testing what the coverage set below \
                 counts it as testing"
            );
            assert_eq!(
                classify(&flow.tasks[0]),
                observed(&plan.tasks[0].kind),
                "{declared}: the exhaustive classifier and `build_plan` disagree \
                 about which of the three tiers this kind is planned into"
            );
            arms_probed.insert(arm);
        }

        // The other side of `pause_gap`: refused at plan, never a tier.
        assert_eq!(
            classify(&crate::spec::parse(pause_with_behavior).unwrap().tasks[0]),
            PlannedAs::Opaque
        );
        let refused = match build_plan(pause_with_behavior) {
            Ok(_) => panic!("a Pause with `behavior:` must not plan"),
            Err(e) => e,
        };
        assert!(
            refused.message.starts_with("unsupported_pause_property:"),
            "{}",
            refused.message
        );

        // **The coverage gate, and it is the whole lesson of I1.** Round 3's
        // table was eleven arms and its probe list was five; the five agreed
        // with the table and six arms -- one of them wrong -- were never
        // compared with anything. So the set of arms actually driven above is
        // compared against the set `arm_name` names, rather than trusted.
        //
        // What this does NOT close, stated rather than left implicit: the
        // expected list below is itself a written list, so a twelfth
        // `models::TaskKind` variant added, classified in all three `match`es
        // and left out of BOTH this list and `probes` would still be
        // unasserted. The compile gate forces the classification; it cannot
        // force a probe, because a probe needs a YAML shape only the author of
        // the variant knows. The mitigation is adjacency: `arm_name`'s match is
        // twenty lines up, and the two lists are read together.
        assert_eq!(
            arms_probed.iter().copied().collect::<Vec<_>>(),
            [
                "AllowFailure",
                "Dag",
                "If",
                "Leaf",
                "Loop",
                "LoopUntil",
                "Parallel",
                "Pause",
                "Sequential",
                "Subflow",
                "Switch",
                "WorkingDirectory",
            ],
            "every arm of the exhaustive classifier must be compared against \
             `build_plan` by a probe; a probe list shorter than the table it \
             certifies agrees with a wrong table"
        );

        // And the three tiers must all be REACHED, so the assertion above
        // cannot be satisfied by a classifier that answers one thing for
        // everything.
        let tiers: BTreeSet<&str> = probes
            .iter()
            .map(|(_, src)| {
                let flow = crate::spec::parse(src).expect("probe parses");
                let plan = plan::flatten_flow(&flow).expect("probe must flatten");
                match observed(&plan.tasks[0].kind) {
                    PlannedAs::Opaque => "Opaque",
                    PlannedAs::Executable => "Executable",
                    PlannedAs::Descends => "Descends",
                    PlannedAs::Waits => "Waits",
                }
            })
            .collect();
        assert_eq!(
            tiers.iter().copied().collect::<Vec<_>>(),
            ["Descends", "Executable", "Opaque", "Waits"],
            "all FOUR of `build_plan`'s outcomes must be exercised, or the \
             four-valued classifier is a three-valued one wearing an enum"
        );

        // The `Subflow` tier, spelled out once rather than only reached through
        // the loop: this is the fact round 3's `bool` could not represent.
        let plan = build_plan(subflow_src).expect("the Subflow probe must plan");
        assert_eq!(
            plan.tasks.len(),
            1,
            "a `Subflow` takes no children: its tasks run in the CHILD execution"
        );
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Executable {
                type_id: "io.kestra.plugin.core.flow.Subflow".to_string()
            },
            "and it is `Executable`, not `Runnable` -- the distinction W6 Task 3 \
             created and the one a `len() > 1` predicate cannot see"
        );

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
        let plan = build_plan(dag_src).expect("the control must plan");
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
        let plan = build_plan(loop_src).expect("the control must plan");
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
        let plan = build_plan(if_src).expect("the control must plan");
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::If {
                condition: "{{ inputs.p }}".to_string(),
                then_: vec![1],
                else_: vec![]
            },
            "an If flattens both branches, as of Task 5"
        );
        let plan = build_plan(switch_src).expect("the control must plan");
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

    /// **`build_plan` refuses every construct of no-silent-stall §3, by its
    /// code** -- every spelling `canonical_flow_type` folds, the FQCN and the
    /// `core.` alias in any case. Each used to plan (an opaque `Runnable`, or
    /// a `Leaf` no handler serves) and every execution of it stalled.
    #[test]
    fn build_plan_refuses_every_static_refusal_by_its_code() {
        let gapped = |ty: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: {ty}\n    condition: \"{{{{ true }}}}\"\n    checkFrequency:\n      interval: \"{{{{ inputs.i }}}}\"\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            )
        };
        let leaf = |ty: &str| {
            format!("id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: {ty}\n")
        };
        let mut probes: Vec<(&str, String)> = Vec::new();
        // A block-level `retry:` on an AllowFailure is refused (spec §2). Every
        // spelling of the type gets the refusal, and the flow without the retry
        // is planned (see `every_static_refusal_...` and the plan test).
        for ty in [
            "io.kestra.plugin.core.flow.AllowFailure",
            "core.allowfailure",
            "core.AllowFailure",
        ] {
            let with_retry = format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    type: {ty}\n    retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 3\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
            );
            probes.push(("unsupported_allow_failure_retry", with_retry));
        }
        for ty in [
            "io.kestra.plugin.core.flow.LoopUntil",
            "io.kestra.plugin.core.flow.WaitFor",
            "core.loopuntil",
            "core.WAITFOR",
        ] {
            probes.push(("unsupported_loop_until_property", gapped(ty)));
        }
        for ty in [
            "io.kestra.plugin.core.flow.NotARealClass",
            "core.somenewgate",
        ] {
            probes.push(("unsupported_flow_task", leaf(ty)));
        }
        // A Kestra-shipped type egret does not port. `core.output.OutputValues`
        // was the example until group C1 Task 2 registered it.
        for ty in [
            "egret.sqll",
            "io.kestra.plugin.notifications.slack.SlackIncomingWebhook",
        ] {
            probes.push(("unknown_task_type", leaf(ty)));
        }
        for (code, src) in &probes {
            let err = build_plan(src).expect_err(code);
            assert!(
                err.message.starts_with(&format!("{code}:")),
                "{code}: the PlanError must carry the code: {}",
                err.message
            );
        }
    }

    /// Every task [`plan::flatten_flow`] plans as a [`TaskKind::Runnable`]
    /// while its `models::TaskKind` is NOT a `Leaf` -- a flowable egret has no
    /// executor for, handed to a worker that has no handler for it -- as
    /// `(path, type)`, the path spelled the way `spec::validate` spells it.
    ///
    /// A planned task carries its id and not its path, so it is matched to the
    /// model by id AND type over the two lists the builder flattens (`tasks:`
    /// and `errors:`, through `children()` and `error_children()`).
    fn opaque_tasks(flow: &crate::models::Flow, plan: &Plan) -> Vec<(String, String)> {
        use crate::models::{Task, TaskKind as K};
        fn walk<'a>(t: &'a Task, path: String, out: &mut Vec<(String, &'a Task)>) {
            for c in t.children().into_iter().chain(t.error_children()) {
                walk(c, format!("{path}.{}", c.id), out);
            }
            out.push((path, t));
        }
        let mut all = Vec::new();
        for (list, tasks) in [("tasks", &flow.tasks), ("errors", &flow.errors)] {
            for t in tasks {
                walk(t, format!("{list}.{}", t.id), &mut all);
            }
        }
        let mut out: Vec<(String, String)> = plan
            .tasks
            .iter()
            .filter_map(|pt| match &pt.kind {
                TaskKind::Runnable { type_id } => Some((pt.id.as_str(), type_id.as_str())),
                _ => None,
            })
            .flat_map(|(id, ty)| {
                all.iter()
                    .filter(move |(_, t)| {
                        t.id == id && t.r#type == ty && !matches!(t.kind, K::Leaf { .. })
                    })
                    .map(|(p, t)| (p.clone(), t.r#type.clone()))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The `(path, type)`s of `src` planned opaque with no `validate_for_run`
    /// error at that path or below it, and how many were planned opaque.
    fn unrefused_opaque(src: &str) -> Option<(Vec<(String, String)>, usize)> {
        let flow = crate::spec::parse(src).ok()?;
        let plan = plan::flatten_flow(&flow).ok()?;
        let errors = crate::spec::validate_for_run(&flow).errors;
        let opaque = opaque_tasks(&flow, &plan);
        let n = opaque.len();
        let unrefused = opaque
            .into_iter()
            .filter(|(path, _)| {
                !errors
                    .iter()
                    .any(|e| e.path == *path || e.path.starts_with(&format!("{path}.")))
            })
            .collect();
        Some((unrefused, n))
    }

    /// **Opaque implies refused (no-silent-stall §3), over the whole corpus.**
    ///
    /// For every corpus flow that parses: every task the builder plans as a
    /// `Runnable` while `models::TaskKind` is not `Leaf` has a
    /// `validate_for_run` error at its path or below it. Such a task is one no
    /// handler can run, and before Task 1 every execution of it stalled in
    /// `RUNNING` (`NotALeaf`, reaped, reclaimed, for ever). A future flowable
    /// planned opaque without a refusal fails this test by flow and path.
    ///
    /// "Planned" is [`plan::flatten_flow`], not `build_plan`: the third gate
    /// now refuses these flows, so `build_plan` would hide exactly the tasks
    /// this is about. A `Leaf` is out of scope here -- whether a deployment
    /// serves its type is the per-deployment refusal of §4.
    ///
    /// The population is asserted, so the guard cannot pass by seeing
    /// nothing: the corpus holds 3 gapped `Pause` tasks the builder reaches
    /// (measured at Task 1; plan Task 3 made `AllowFailure` and
    /// `WorkingDirectory` flowables, so neither is opaque any more), and no gapped
    /// `LoopUntil` (`every_corpus_loop_until_bound_is_a_literal`), so that one
    /// is driven by the synthetic flows below, under a flowable's `errors:`.
    #[test]
    fn every_task_planned_opaque_is_refused_at_its_path() {
        let mut unrefused = Vec::new();
        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        for (id, src) in every_corpus_flow() {
            let Some((missing, _)) = unrefused_opaque(&src) else {
                continue;
            };
            let flow = crate::spec::parse(&src).expect("parsed above");
            let plan = plan::flatten_flow(&flow).expect("flattened above");
            for (_, ty) in opaque_tasks(&flow, &plan) {
                *by_type
                    .entry(crate::models::canonical_flow_type(&ty).unwrap_or(ty))
                    .or_default() += 1;
            }
            unrefused.extend(missing.into_iter().map(|(p, t)| format!("{id}: {p} ({t})")));
        }

        // The constructs the corpus cannot show, nested where the builder
        // reaches them: a `LoopUntil` and a `WaitFor` with a bound it cannot
        // read, a `Pause` with `behavior:`, under a `Sequential`'s `errors:`.
        let synthetic = |inner: &str| {
            format!(
                "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ok\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n    errors:\n{inner}"
            )
        };
        for (label, inner) in [
            (
                "LoopUntil",
                "      - id: lu\n        type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{ true }}\"\n        checkFrequency:\n          maxIterations: \"{{ inputs.n }}\"\n        tasks:\n          - id: body\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n",
            ),
            (
                "WaitFor",
                "      - id: lu\n        type: io.kestra.plugin.core.flow.WaitFor\n        condition: \"{{ true }}\"\n        failOnMaxReached: \"{{ inputs.f }}\"\n        tasks:\n          - id: body\n            type: io.kestra.plugin.core.debug.Return\n            format: x\n",
            ),
            (
                "Pause",
                "      - id: lu\n        type: io.kestra.plugin.core.flow.Pause\n        behavior: FAIL\n",
            ),
        ] {
            let (missing, n) = unrefused_opaque(&synthetic(inner)).expect(label);
            assert_eq!(n, 1, "{label}: the synthetic task must be planned opaque");
            unrefused.extend(missing.into_iter().map(|(p, t)| format!("synthetic {label}: {p} ({t})")));
        }

        assert!(
            unrefused.is_empty(),
            "planned opaque, so no handler can run it, and NOT refused by \
             `validate_for_run` -- every execution of it would stall:\n  {}",
            unrefused.join("\n  ")
        );
        assert_eq!(
            by_type,
            BTreeMap::from([
                ("pause".to_string(), 3),
                // `allowfailure` left this map at spec §5.2, and
                // `workingdirectory` at plan Task 3 (spec §4.2): both are planned
                // flowables now, so neither is an opaque task. The 15 that were
                // here were refused by `unsupported_working_directory`, which
                // plan Task 3 removed.
            ]),
            "the corpus population this guard judges; if it moves, re-measure \
             and say why"
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

    /// **The two over-refusals, and the sweep below asserts there is no
    /// third.** The rows after them are not over-refusals but refusals of
    /// constructs egret cannot run: three `Pause` rows (W7b, 2026-10-11) and
    /// fifty-two from no-silent-stall Task 1 (2026-10-12).
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
            cause: "`namespace: \"{{inputs.namespace}}\"` on a `Subflow`.                     `subflow_namespace_invalid` checks the grammar against the                     UNRENDERED source, so every dynamic subflow reference is                     refused. The check was introduced because publish then encoded                     the pair as `\"{namespace}/{flowId}\"`, where a `/` in either                     half decoded back into a different pair. Since debt 5.1                     (2026-10-19) publish looks the pair up as two columns, so the                     check stands on being Kestra's own grammar; the fix is to                     check a templated target after rendering, at run time, and it                     is W1's",
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
        PublishedButRefused {
            id: "valids/pause-behavior",
            cause: "a `Pause` declaring `behavior:`. Kestra runs it; egret does not, and `executor::pause_gap` names it. Until the user's ruling of 2026-10-11 it planned an opaque runnable with a `not_yet_enforced` warning, and in production every execution of it stalled in `RUNNING` for ever (`NotALeaf`, reaped, reclaimed). The ruling made it the validation error `unsupported_pause_property`: a deliberate refusal, not an over-refusal to close, until egret runs `behavior:`. W8's",
            codes: &["unsupported_pause_property"],
        },
        PublishedButRefused {
            id: "valids/pause-errors-finally-after-execution",
            cause: "a `Pause` declaring `errors:`. Kestra runs it; egret does not, and `executor::pause_gap` names it. Until the user's ruling of 2026-10-11 it planned an opaque runnable with a `not_yet_enforced` warning, and in production every execution of it stalled in `RUNNING` for ever (`NotALeaf`, reaped, reclaimed). The ruling made it the validation error `unsupported_pause_property`: a deliberate refusal, not an over-refusal to close, until egret runs `errors:`. W8's",
            codes: &["unsupported_pause_property"],
        },
        PublishedButRefused {
            id: "valids/pause_on_pause",
            cause: "a `Pause` declaring `onPause:`. Kestra runs it; egret does not, and `executor::pause_gap` names it. Until the user's ruling of 2026-10-11 it planned an opaque runnable with a `not_yet_enforced` warning, and in production every execution of it stalled in `RUNNING` for ever (`NotALeaf`, reaped, reclaimed). The ruling made it the validation error `unsupported_pause_property`: a deliberate refusal, not an over-refusal to close, until egret runs `onPause:`. W8's",
            codes: &["unsupported_pause_property"],
        },
        // no-silent-stall Task 1 (spec §3): 52 rows, one per flow this Kestra
        // published with 200 that the static refusals now refuse -- 4
        // `AllowFailure`, 4 `WorkingDirectory`, and 44 a type Kestra ships and
        // egret does not register (`unknown_task_type`, an error since Task
        // 1). Measured, not argued: the corpus has no gapped `LoopUntil` and no
        // unported control-flow type among its published flows. Each would
        // otherwise start and stall -- the 8 flowable rows as published
        // versions, the 44 `unknown_task_type` rows only as started drafts,
        // because publish already refused those -- and every one is a
        // deliberate refusal, as the three `Pause` rows above are.
        //
        // (2026-10-17, kill-and-exit Task 3: the four `valids/exit*` rows are
        // gone -- `io.kestra.plugin.core.execution.Exit` is registered and
        // planned `TaskKind::Exit`, so those flows plan and the sweep below
        // would name any row left behind.)
        //
        // (2026-10-18, group C1 Task 2: the 21 rows whose only unported type
        // was `io.kestra.plugin.core.output.OutputValues` are gone -- it is
        // registered (`worker::handlers::output_values`) -- and the five rows
        // that also named another type now name only the types still missing.)
        //
        // (2026-10-18, group C1 Task 3: the two rows whose only unported type
        // was `io.kestra.plugin.core.execution.Assert`, `valids/assert` and
        // `valids/if-in-parallel`, are gone -- it is registered
        // (`worker::handlers::assert`).)
        //
        // (2026-10-18, group C1 Task 4: the five `valids/purge_logs_*` rows
        // are gone -- `io.kestra.plugin.core.log.PurgeLogs` is registered
        // (`worker::handlers::purge_logs`).)
        //
        // (2026-10-18, group C2 Task 5: the twelve rows whose unported types
        // were `io.kestra.plugin.core.execution.SetVariables`, `UnsetVariables`
        // or `Labels` are gone -- all three are registered and executor-level
        // (`spec::registry::EXECUTOR_LEVEL`, planned `TaskKind::ExecutionUpdate`).)
        PublishedButRefused {
            id: "valids/retry-with-flowable-errors",
            cause: "an `AllowFailure` flowable with `retry:` on the block itself. Kestra retries the failing child; egret refuses the block-level `retry:` on purpose (spec §2, a non-goal for the AllowFailure flowable package), so the flow is refused where Kestra ran it.",
            codes: &["unsupported_allow_failure_retry"],
        },
    ];

    /// **Every flow this Kestra published with 200 must plan, except two
    /// named over-refusals — swept over the whole corpus, not pinned by
    /// name.**
    ///
    /// *(2026-10-11, W7b Task 4b: the table holds five rows now. The three
    /// added are not over-refusals to close but the user's ruling that a
    /// `Pause` egret cannot run is refused. Renamed at W7b Task 7b from
    /// `…_except_two_named_over_refusals`, which had stopped being true.)*
    ///
    /// *(2026-10-12, no-silent-stall Task 1: fifty-seven rows. The fifty-two
    /// added are the static refusals of that package's spec §3 -- a construct
    /// egret cannot run is refused rather than left to stall -- so the
    /// over-refusals proper are still the first two.)*
    ///
    /// *(2026-10-17, kill-and-exit Task 3: the four `valids/exit*` rows are
    /// removed; `Exit` plans. 2026-10-18, group C1 Task 2: the 21
    /// `OutputValues`-only rows are removed; `OutputValues` is registered.)*
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
    /// **301** of the 351 flows. That is deliberately wider than the golden's
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
    fn every_flow_kestra_published_plans_except_the_named_refusals() {
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
            // 295 -> 301 and 345 -> 351 at WorkingDirectory Task 8: the six
            // `working-directory-real-*` flows publish (200) and are in the corpus.
            (published.len(), entries.len()),
            (301, 351),
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
            // 2050 -> 2080 executions and 4215 -> 4300 taskruns at WorkingDirectory
            // Task 8: the six harvested flows, one execution and 17 taskruns each in
            // each of the five rounds (6 x 5 = 30; 17 x 5 = 85).
            (docs, runs),
            (2080, 4300),
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
    /// direction is never taken: every flow in the 351-flow corpus that
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
            (24, 25, 25),
            "24 corpus flows declare a `retry:` on a task `build_plan` plans (or \
             on the flow), 25 policies between them -- one more than before \
             `valids/allow-failure-with-retry`, whose child retry plans now that \
             AllowFailure is planned (spec §5.2). Before that it was 23, 24, 24; \
             25 and 26 until no-silent-stall Task 1, whose static refusals made \
             two more flows refusable, and every one of them is \
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
             The denominator is all **351** corpus flows, `invalids/` included, \
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
        // (41, 85) -> (42, 88) at group C1 Task 3 (2026-10-18, measured):
        // `valids/if-in-parallel` plans again (`Assert` is registered).
        assert_eq!(
            (flows, wide),
            (42, 88),
            "the 42 corpus flows holding a `Loop` that `build_plan` plans and the \
             88 tasks the wide reading names inside one (41 and 85 until group C1 \
             Task 3 registered `Assert`) -- the same 88 \
             `the_two_readings_of_inside_a_loop_differ_by_a_measured_amount` \
             derives. Plan Task 3 took the two `WorkingDirectory` flows \
             `valids/working-directory-taskrun` and `-taskrun-nested` out of the \
             refused set, so the count is 39 + 2 flows and 78 + 7 tasks (43 and 88 \
             until no-silent-stall Task 1 refused four of those flows). 39 flows \
             and not 38 before this: the flow count is over the YAML and \
             `disabled:` does not change it"
        );
        assert_eq!(
            wide - without_errors,
            0,
            "not one of those 78 is reached through an `errors:` list, so \
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

    // ----------------------------------------------------------------------
    // W8 Task 5: `allowFailure:` / `allowWarning:` are read off the TASK
    // ----------------------------------------------------------------------

    /// A softened flowable FIRST and an unsoftened one after it, so
    /// `plan.tasks[0]` and `plan.tasks[task]` disagree.
    const TWO_BLOCKS: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: soft
    type: io.kestra.plugin.core.flow.Sequential
    allowFailure: true
    tasks:
      - id: a
        type: io.kestra.plugin.core.execution.Fail
  - id: plain
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: b
        type: io.kestra.plugin.core.execution.Fail
"#;

    /// [`TWO_BLOCKS`] without the unsoftened half — the control for what the
    /// EXECUTION does when its only failure is softened.
    const ONE_SOFT_BLOCK: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: soft
    type: io.kestra.plugin.core.flow.Sequential
    allowFailure: true
    tasks:
      - id: a
        type: io.kestra.plugin.core.execution.Fail
"#;

    /// [`ONE_SOFT_BLOCK`] with the OTHER flag — `allowWarning:` and no
    /// `allowFailure:`, the combination no corpus flow writes.
    const WARN_ONLY_BLOCK: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: soft
    type: io.kestra.plugin.core.flow.Sequential
    allowWarning: true
    tasks:
      - id: a
        type: io.kestra.plugin.core.execution.Fail
"#;

    /// The run history of the task with this id, by name rather than by index.
    fn history_of(plan: &Plan, state: &ExecutionState, id: &str) -> Vec<&'static str> {
        let ix = plan
            .tasks
            .iter()
            .position(|t| t.id == id)
            .unwrap_or_else(|| panic!("{id} is in the plan"));
        state
            .runs
            .iter()
            .find(|r| r.task == ix)
            .map(|r| wire(&r.history))
            .unwrap_or_else(|| panic!("{id} has a run"))
    }

    /// **Two flowables in one flow, one carrying `allowFailure: true` and one
    /// carrying nothing, both failing** — the row that says the flags are read
    /// off the task being terminated rather than off anything else.
    ///
    /// # Why this fixture exists, measured rather than argued
    ///
    /// Every corpus flow that declares an `allowFailure:` on a flowable
    /// declares it on the flow's **first** task, so `plan.tasks[task]` and
    /// `plan.tasks[0]` are the same expression on all of them. Mutation M7 —
    /// `let t = &plan.tasks[0];` in [`flowable::descend`] — therefore **survived
    /// the whole crate suite at 0 failures, with the ratchet green and unmoved
    /// at 111**: **792 lib + 54 oracle**, which is 846 and not the "843 lib +
    /// 54 oracle" this sentence carried for one round — 843 was the TOTAL of
    /// the two, written as one of its parts. That is the operand rule this
    /// package keeps paying for: *a two-sided mutation pair on an OPERATOR says
    /// nothing about where the OPERAND came from*, and the seven ratchet
    /// entries that pin this mapping pin the operator only.
    ///
    /// The competing rules, named before the rows: **(a)** the flags come from
    /// the task whose run is being terminated; **(b)** they come from the
    /// flow's first task; **(c)** they are a property of the flow. `soft` and
    /// `plain` below disagree under (a) and agree under both (b) and (c), and
    /// they are in this ORDER on purpose — with `soft` first, (b) and (c) both
    /// soften `plain` too, so one assertion separates (a) from both.
    ///
    /// # What else it pins
    ///
    /// **A `WARNING` sibling does not stop a `Sequential`.** `soft` ends
    /// `WARNING` and `plain` is created and driven anyway, which is
    /// [`current_list`](flowable::current_list)'s rule (`Current::Tasks` unless
    /// a child is `Failed`) and `State.Type.isFailed()` being `this == FAILED`
    /// alone. Had `WARNING` counted as a failure the walk would have stopped at
    /// `soft` and this test would see one flowable run where it sees two.
    #[test]
    fn the_allow_flags_are_read_off_the_task_being_terminated() {
        let plan = build_plan(TWO_BLOCKS).expect("the probe plans");
        let state = run_to_fixpoint(&plan, RunState::Failed);
        assert_eq!(
            (
                history_of(&plan, &state, "soft"),
                history_of(&plan, &state, "a")
            ),
            (
                vec!["CREATED", "RUNNING", "WARNING"],
                vec!["CREATED", "SUBMITTED", "RUNNING", "FAILED"]
            ),
            "the flowable that WROTE `allowFailure: true` is softened, and its \
             child -- which wrote nothing -- is not. The child is the second \
             half on purpose: a rule that softened the whole subtree would move \
             it too"
        );
        assert_eq!(
            (
                history_of(&plan, &state, "plain"),
                history_of(&plan, &state, "b")
            ),
            (
                vec!["CREATED", "RUNNING", "FAILED"],
                vec!["CREATED", "SUBMITTED", "RUNNING", "FAILED"]
            ),
            "and the flowable that wrote NOTHING ends FAILED, though the flow's \
             first task wrote `allowFailure: true`. This is the assertion \
             mutation M7 dies on"
        );
        assert_eq!(
            wire(&state.history).last().copied(),
            Some("FAILED"),
            "the execution aggregates KILLED > FAILED > WARNING, so one \
             softened child beside one unsoftened one is FAILED -- the control \
             for the test below, where the only failure IS softened"
        );
    }

    /// The control the test above needs: with the unsoftened flowable removed,
    /// the execution really does take `WARNING`.
    ///
    /// **Without it, `FAILED` above is ambiguous** — it is the right answer for
    /// an [`flowable::aggregate`] that ranks `Warning` correctly AND for one
    /// that ignores `Warning` entirely, because `plain`'s `FAILED` dominates
    /// either way.
    #[test]
    fn an_execution_whose_only_failure_is_softened_ends_warning() {
        let plan = build_plan(ONE_SOFT_BLOCK).expect("the probe plans");
        let state = run_to_fixpoint(&plan, RunState::Failed);
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "WARNING"],
            "one softened failure and nothing else"
        );
    }

    /// `allowWarning:` **alone** changes nothing, at the flowable site as at
    /// every other — `State$Type.fail`'s bytecode does not read
    /// `isAllowWarning()` when `isAllowFailure()` is false (`ifeq 26` at offset
    /// 4), and spelling B's first `if` is predicated on `allowFailure` too.
    ///
    /// **No corpus flow writes it**: all three `allowWarning:` declarers write
    /// `allowFailure: true` beside it, so the ratchet cannot score this row and
    /// this assertion is its whole instrument. It is also what makes
    /// `the_allow_flags_are_read_off_the_task_being_terminated` mean what it
    /// says — without it, an implementation that read `allow_warning` where it
    /// should read `allow_failure` would pass everything above.
    #[test]
    fn allow_warning_alone_softens_nothing() {
        let plan = build_plan(WARN_ONLY_BLOCK).expect("the probe plans");
        let state = run_to_fixpoint(&plan, RunState::Failed);
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "FAILED"],
            "`allowWarning: true` with no `allowFailure:` leaves a FAILED alone"
        );
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
    /// `Sequential` because a `Sequential` is the cheapest kind that really
    /// has children in the plan to count.
    ///
    /// **This sentence said "because a `Subflow` is planned opaque and could
    /// not tell the difference" until W8 Task 1 fix round 2**, and that had
    /// been false since W6 Task 3 gave `Subflow` a [`TaskKind::Executable`].
    /// It mattered more than a stale name because it was the stated *reason*
    /// for the test's design — a reader checking whether the choice of kind
    /// still holds would have been checking a fact that no longer exists.
    /// W8 Task 1's own opacity sweep missed it: that sweep looked for lists
    /// that *enumerate* the opaque types, and this is a single construct named
    /// inside an argument. The needle that finds it is every `opaque` in the
    /// tree, tense-read for each `Subflow` near one.
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

    /// Drive `advance` to a fixpoint, giving a terminal only to the RUNNABLE
    /// runs whose task id is listed — W8 Task 5 fix round 2.
    ///
    /// **The existing helpers all give every runnable the SAME terminal**, and
    /// the two rows below need a flow where one branch ends badly and another
    /// is still in flight. That is exactly the shape `fail_the_rest` acts on,
    /// so a fixture that could not build it could not test rule 1's guard at
    /// all. A run whose id is not listed is left `RUNNING` on purpose: the
    /// question is whether rule 1 reaches in and FAILS it.
    fn run_to_fixpoint_completing(
        plan: &Plan,
        outcomes: &[(&str, RunState)],
    ) -> (ExecutionState, Vec<Command>) {
        let mut state = ExecutionState::new(plan);
        let mut last: Vec<Command> = Vec::new();
        for _ in 0..1000 {
            let cmds = advance(plan, &state, T0);
            if !cmds.is_empty() {
                last = cmds.clone();
                for cmd in &cmds {
                    apply(&mut state, cmd);
                }
                continue;
            }
            let next = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && state.runs[ix].history.last() == Some(&RunState::Running)
                    && outcomes
                        .iter()
                        .any(|(id, _)| *id == plan.tasks[state.runs[ix].task].id)
            });
            match next {
                Some(ix) => {
                    let id = &plan.tasks[state.runs[ix].task].id;
                    let outcome = outcomes
                        .iter()
                        .find(|(t, _)| t == id)
                        .map(|(_, s)| *s)
                        .expect("an outcome");
                    apply(
                        &mut state,
                        &Command::SetRunState {
                            run: ix,
                            state: outcome,
                        },
                    );
                }
                None => break,
            }
        }
        (state, last)
    }

    /// A `Parallel` whose first branch softens its failure to `WARNING` and
    /// whose second is a bare leaf the driver leaves in flight.
    const PARALLEL_WARNING_SIBLING: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: par
    type: io.kestra.plugin.core.flow.Parallel
    tasks:
      - id: soft
        type: io.kestra.plugin.core.flow.Sequential
        allowFailure: true
        tasks:
          - id: boom
            type: io.kestra.plugin.core.execution.Fail
      - id: sibling
        type: io.kestra.plugin.core.debug.Return
        format: b
"#;

    /// **A `WARNING` branch does not fail its siblings** — the second shipped
    /// defect `RunState::Warning` created, and the one that says the variant
    /// falsified a FAMILY rather than a line.
    ///
    /// [`parallel::drive_concurrent`]'s rule 1 read *"terminal and not
    /// `SUCCESS` or `SKIPPED`"* — an EXCLUSION list, which equals `isFailed()`
    /// only while every other terminal is a failure. It stopped being that
    /// twice: `Retried` at Task 7 and `Warning` at W8 Task 5, both members of
    /// `isTerminatedNoFail()`. So a softened branch made the `Parallel`
    /// `fail_the_rest` — **egret failing runs Kestra succeeds**, which is the
    /// class the comment directly above that line already forbade.
    ///
    /// Measured on the pinned artifact, and **corrected in fix round 3**: this
    /// read *"`lambda$hasFailed$0`, `lambda$hasFailed$1` and
    /// `lambda$hasFailedNoRetry$0` … are each `getState().isFailed()` and
    /// nothing else"*, which is false for the third —
    /// `lambda$hasFailedNoRetry$0` is `isFailed() && shouldNotBeRetried(…)`,
    /// bci 0–24 with a branch. Rule 1 corresponds to **`hasFailed`**, called at
    /// bci 110 of `findTaskDependingFlowState`, whose lambda
    /// (`lambda$hasFailed$1`) is four instructions and really is `isFailed()`
    /// alone. The retry clause belongs to the second gate at bci 120, which is
    /// [`flowable::current_list`]'s `Current::RetryPending`; see
    /// `parallel::drive_concurrent`'s rule 1 for the one place egret still
    /// lacks it and why no corpus flow can reach it.
    ///
    /// **The oracle cannot score this** — no corpus flow puts an
    /// `allowFailure:` inside a `Parallel`, and applying the fix leaves the
    /// ratchet at 111 — so this row is the whole instrument. `sibling` is left
    /// **`RUNNING`** rather than completed, because a sibling that has already
    /// terminated keeps its own state whatever rule 1 does; only a run still in
    /// flight can be reached by `fail_the_rest`.
    #[test]
    fn a_warning_branch_does_not_fail_its_siblings() {
        let plan = build_plan(PARALLEL_WARNING_SIBLING).expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(&plan, &[("boom", RunState::Failed)]);
        assert_eq!(
            history_of(&plan, &state, "soft"),
            ["CREATED", "RUNNING", "WARNING"],
            "the control: the branch really did end WARNING, so the row below \
             is about a WARNING sibling and not about a branch that never \
             finished"
        );
        assert_eq!(
            history_of(&plan, &state, "sibling"),
            ["CREATED", "SUBMITTED", "RUNNING"],
            "still in flight -- rule 1 must not have failed it"
        );
        assert!(
            !state.runs.iter().any(
                |r| r.history.contains(&RunState::Failed) && plan.tasks[r.task].id == "sibling"
            ),
            "no sibling may carry a FAILED that rule 1 wrote"
        );
    }

    /// A `Parallel` whose first branch is a bare leaf the driver gives an
    /// arbitrary terminal, and whose second is left in flight.
    const PARALLEL_LEAF_SIBLING: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: par
    type: io.kestra.plugin.core.flow.Parallel
    tasks:
      - id: doomed
        type: io.kestra.plugin.core.debug.Return
        format: a
      - id: sibling
        type: io.kestra.plugin.core.debug.Return
        format: b
"#;

    /// **Rule 1 is narrowed by exactly three states, and all three are pinned
    /// here** — `Warning`, `Killed` and `Retried`.
    ///
    /// Rule 1 used to be an exclusion list and is `isFailed()` now. Two of the
    /// three states that changed hands were **unguarded** after fix round 2:
    /// probes re-admitting `Killed` and `Retried` to the predicate both
    /// SURVIVED the whole suite at 797 + 54 with the ratchet unmoved, so
    /// nothing would have noticed the narrowing being undone. The `Warning` row
    /// lives in `a_warning_branch_does_not_fail_its_siblings`; these two are
    /// its missing siblings.
    ///
    /// **Right against the artifact**: `hasFailed`'s lambda is
    /// `getState().isFailed()`, and `State.Type.isFailed()` is `this == FAILED`
    /// — so neither `KILLED` nor `RETRIED` switches a branch to its `errors:`.
    /// `KILLED` still dominates the parent's terminal, by a different
    /// mechanism: `guessFinalState` tests it first and breaks, which
    /// [`flowable::aggregate`] reproduces — asserted below, because a narrowing
    /// that also lost the aggregate would be a second defect wearing the first
    /// one's clothes.
    ///
    /// **The oracle cannot score any of it**: no corpus `Parallel` child
    /// reaches `KILLED` or `RETRIED` with a sibling in flight.
    #[test]
    fn a_killed_or_retried_branch_does_not_fail_its_siblings() {
        for (branch, parent) in [
            (RunState::Killed, RunState::Killed),
            (RunState::Retried, RunState::Success),
        ] {
            let plan = build_plan(PARALLEL_LEAF_SIBLING).expect("the probe plans");
            let (state, _) = run_to_fixpoint_completing(&plan, &[("doomed", branch)]);
            assert_eq!(
                history_of(&plan, &state, "doomed").last().copied(),
                Some(branch.wire()),
                "{branch:?}: the control -- the branch really did end there"
            );
            assert_eq!(
                history_of(&plan, &state, "sibling"),
                ["CREATED", "SUBMITTED", "RUNNING"],
                "{branch:?} is not `isFailed()`, so rule 1 must leave a sibling \
                 that is still in flight alone"
            );
            // And once the sibling finishes, the parent's terminal is the
            // AGGREGATE's -- `KILLED` dominates, `RETRIED` has no arm at all.
            let (done, _) = run_to_fixpoint_completing(
                &plan,
                &[("doomed", branch), ("sibling", RunState::Success)],
            );
            assert_eq!(
                history_of(&plan, &done, "par").last().copied(),
                Some(parent.wire()),
                "{branch:?}: `guessFinalState`'s priority pass, unchanged by \
                 the narrowing"
            );
        }
    }

    /// A `Dag` of two INDEPENDENT nodes — no `dependsOn` between them — so one
    /// can fail while the other is still in flight. `{RETRY}` is filled in or
    /// blanked by the two rows that use it.
    const DAG_INDEPENDENT_NODES: &str = r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: d
    type: io.kestra.plugin.core.flow.Dag
    tasks:
      - task:
          id: boom
          type: io.kestra.plugin.core.execution.Fail
{RETRY}      - task:
          id: sibling
          type: io.kestra.plugin.core.debug.Return
          format: b
"#;

    /// **A `Dag` node whose failure will be RETRIED does not fail its
    /// siblings** — Kestra's second gate, and the one path in this executor
    /// that did not go through it.
    ///
    /// `findTaskDependingFlowState` asks two predicates in order: `hasFailed`
    /// (bci 110) and then `hasFailedNoRetry` (bci 120). When every failure will
    /// be retried the second answers false and the method returns
    /// `emptyList()` — start nothing, **fail nothing**, wait.
    ///
    /// egret's counterpart is [`flowable::current_list`]'s
    /// `Current::RetryPending`, whose `failed.iter().all(will_retry)` is the
    /// exact complement of `hasFailedNoRetry`'s `anyMatch`.
    /// `parallel::drive` has always consulted it; **`dag::drive` did not** —
    /// with an empty `errors:` it went straight to `drive_concurrent`, so rule
    /// 1 saw a `FAILED` child mid-retry and `fail_the_rest` killed the
    /// siblings. **egret acting where the artifact does not**, which is
    /// over-acceptance however quiet the ratchet is.
    ///
    /// **The control is the second row**: with the `retry:` removed, the same
    /// flow DOES fail the sibling. Without it this test passes on an
    /// implementation where rule 1 never fires for a `Dag` at all, which is
    /// precisely what a badly-placed gate would produce.
    ///
    /// **The oracle cannot score either row** —
    /// `no_corpus_parallel_child_declares_a_retry` derives that no corpus flow
    /// puts a `retry:` on a direct `Parallel`/`Dag` child, so the artifact and
    /// not the corpus is the authority here.
    #[test]
    fn a_dag_node_awaiting_a_retry_does_not_fail_its_siblings() {
        const RETRY: &str = "          retry:\n            type: constant\n            \
                             interval: PT1S\n            maxAttempts: 3\n";

        let plan =
            build_plan(&DAG_INDEPENDENT_NODES.replace("{RETRY}", RETRY)).expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(&plan, &[("boom", RunState::Failed)]);
        assert_eq!(
            history_of(&plan, &state, "boom").last().copied(),
            Some("RETRYING"),
            "the control for this row: the failure really is mid-retry, not \
             merely failed"
        );
        assert_eq!(
            history_of(&plan, &state, "sibling"),
            ["CREATED", "SUBMITTED", "RUNNING"],
            "still in flight -- the artifact waits for the retry and fails \
             nothing, so `fail_the_rest` must not have run"
        );

        // The discriminating row: the SAME flow with no `retry:`.
        let plain =
            build_plan(&DAG_INDEPENDENT_NODES.replace("{RETRY}", "")).expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(&plain, &[("boom", RunState::Failed)]);
        assert_eq!(
            history_of(&plain, &state, "sibling"),
            ["CREATED", "SUBMITTED", "RUNNING", "FAILED"],
            "with nothing left to retry, `hasFailedNoRetry` is TRUE and rule 1 \
             fires as it always did -- this is what stops the row above \
             passing on a Dag that never fails anything"
        );
    }

    /// **Rule 4 completes a `Dag` under a `KILLED` dependency**, which is what
    /// the two fix-round-2 changes do differently *together*.
    ///
    /// `ended_badly` has a second reader: rule 4 treats a branch as finishable
    /// when every created run is terminal AND either every child has a run **or
    /// rule 1 already closed the branch**. Narrowing rule 1 removes the second
    /// disjunct for `KILLED` and `RETRIED` — so on its own it would leave a
    /// `Dag` with an uncreated node `Waiting` for ever, i.e. STUCK.
    ///
    /// It does not, because the dependency gate moved in the same commit: a
    /// `KILLED` dependency **is** terminated, so `down` is created, so
    /// `created.len() == children.len()` and rule 4's first disjunct carries
    /// it. **The coupling runs in both directions** — `dag.rs`'s doc records
    /// the other one — and this row is what says the pair is closed rather than
    /// merely consistent.
    #[test]
    fn a_dag_node_downstream_of_a_killed_dependency_still_runs() {
        let plan = build_plan(
            r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: d
    type: io.kestra.plugin.core.flow.Dag
    tasks:
      - task:
          id: up
          type: io.kestra.plugin.core.debug.Return
          format: a
      - task:
          id: down
          type: io.kestra.plugin.core.debug.Return
          format: b
        dependsOn:
          - up
"#,
        )
        .expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(
            &plan,
            &[("up", RunState::Killed), ("down", RunState::Success)],
        );
        assert_eq!(
            history_of(&plan, &state, "down"),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "a KILLED dependency is TERMINATED, so the dependant is ready -- \
             without the gate moving with rule 1 this run would not exist and \
             rule 4 would leave the Dag Waiting"
        );
        assert_eq!(
            history_of(&plan, &state, "d").last().copied(),
            Some("KILLED"),
            "and rule 4 completes the branch on the aggregate, KILLED first"
        );
    }

    /// The control the row above needs: rule 1 **does** still fire on a real
    /// `FAILED` sibling.
    ///
    /// Without it, `a_warning_branch_does_not_fail_its_siblings` passes on an
    /// implementation where rule 1 never fires at all — which is exactly what
    /// narrowing the predicate could have produced.
    #[test]
    fn a_failed_branch_still_fails_its_siblings() {
        let plan =
            build_plan(&PARALLEL_WARNING_SIBLING.replace("        allowFailure: true\n", ""))
                .expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(&plan, &[("boom", RunState::Failed)]);
        assert_eq!(
            history_of(&plan, &state, "soft"),
            ["CREATED", "RUNNING", "FAILED"],
            "with the flag removed the branch really does FAIL"
        );
        assert_eq!(
            history_of(&plan, &state, "sibling"),
            ["CREATED", "SUBMITTED", "RUNNING", "FAILED"],
            "and rule 1 fails the sibling that was still in flight"
        );
    }

    /// **A `Dag` node downstream of a `WARNING` dependency still runs** — the
    /// site coupled to rule 1, and the reason the two move together.
    ///
    /// `FlowableUtils.resolveDagNexts`'s dependency filter is
    /// `getState().isTerminated()` — `lambda$resolveDagNexts$3` in
    /// `FlowableUtils.class`, measured, one instruction. `dag::drive` spelled
    /// it `SUCCESS | SKIPPED`, which was the same set only while every other
    /// terminal had already ended the branch through rule 1.
    ///
    /// **Fixing rule 1 alone would have traded one divergence for a worse
    /// one**: with rule 1 no longer firing on a `WARNING` sibling and the
    /// dependency gate still demanding `SUCCESS`, a `Dag` whose upstream node
    /// softens its failure would hang for ever — nothing ready, nothing failed.
    /// That is what this row and the `ended_badly` change together prevent, and
    /// why neither is safe to ship without the other.
    #[test]
    fn a_dag_node_downstream_of_a_warning_dependency_still_runs() {
        let plan = build_plan(
            r#"id: probe
namespace: io.kestra.tests
tasks:
  - id: d
    type: io.kestra.plugin.core.flow.Dag
    tasks:
      - task:
          id: up
          type: io.kestra.plugin.core.flow.Sequential
          allowFailure: true
          tasks:
            - id: boom
              type: io.kestra.plugin.core.execution.Fail
      - task:
          id: down
          type: io.kestra.plugin.core.debug.Return
          format: b
        dependsOn:
          - up
"#,
        )
        .expect("the probe plans");
        let (state, _) = run_to_fixpoint_completing(
            &plan,
            &[("boom", RunState::Failed), ("down", RunState::Success)],
        );
        assert_eq!(
            history_of(&plan, &state, "up"),
            ["CREATED", "RUNNING", "WARNING"],
            "the control: the dependency really did end WARNING"
        );
        assert_eq!(
            history_of(&plan, &state, "down"),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "the dependant is READY on a terminated dependency, whatever \
             terminal it took -- a `Dag` that hangs here is the worse \
             divergence the rule-1 fix would otherwise have created"
        );
    }

    /// **A skipped branch does not fail its siblings** — the guard on
    /// [`parallel::drive_concurrent`]'s rule 1.
    ///
    /// Through W3a, "terminal and not `SUCCESS`" and "terminated in error" were
    /// the same set for every state [`RunState`] could hold. `Skipped` is the
    /// first that was not: `State.java:293` lists it in `isTerminatedNoFail()`
    /// beside `SUCCESS`. Without it being excluded, `fail_the_rest` would
    /// `FAIL` every sibling still in flight the moment one branch's `runIf:`
    /// came back falsy — egret failing runs Kestra succeeds.
    ///
    /// **The rule is no longer an exclusion list, and this row now passes for a
    /// stronger reason than it used to** — W8 Task 5 fix round 2. `Skipped`
    /// was patched OUT of a predicate that admitted everything else;
    /// `Retried` and then `Warning` joined the enum without being patched out,
    /// and `Warning` shipped as a divergence
    /// (`a_warning_branch_does_not_fail_its_siblings`). Rule 1 is
    /// `latest == Some(Failed)` now — Kestra's `isFailed()`, measured — so
    /// `Skipped` is outside it by construction rather than by an exclusion
    /// somebody has to remember to extend.
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

    /// **No corpus flow puts a `retry:` on a DIRECT `Parallel` or `Dag`
    /// child** — the derivation behind the one divergence
    /// [`parallel::drive_concurrent`]'s rule 1 knowingly carries.
    ///
    /// Kestra gates the switch to a branch's `errors:` on **two** predicates in
    /// order (`findTaskDependingFlowState`, bci 110 and 120): `hasFailed`, then
    /// `hasFailedNoRetry`. egret has both — rule 1 is the first, and
    /// [`flowable::current_list`]'s `Current::RetryPending` is the second.
    ///
    /// **This doc said rule 1 "does not consult it" and that was the wrong
    /// diagnosis** (W8 Task 5 fix round 3, corrected in round 4).
    /// `parallel::drive` consults the second gate before `drive_concurrent` is
    /// reached; what skipped it was `dag::drive`, which with an empty `errors:`
    /// called `drive_concurrent` directly. The gate is there now
    /// (`a_dag_node_awaiting_a_retry_does_not_fail_its_siblings`), and what
    /// this test still derives is unchanged and is why the ARTIFACT rather than
    /// the corpus had to be the authority for it.
    ///
    /// **No corpus flow can score the second gate, so the artifact is its
    /// authority.** Three flows declare a `Parallel`/`Dag` *and* a `retry:` —
    /// `valids/full` and `invalids/duplicate-parallel` put the `retry:` on a
    /// top-level task, and **`valids/subflow-parent-retry` puts it inside a
    /// `Parallel`**, on `subflow1`/`subflow2`. That one is a grandchild: rule 1
    /// reads the `Parallel`'s DIRECT children (`seq1`, `seq2`), and a
    /// `Sequential` whose child is mid-retry stays `RUNNING` because
    /// `current_list` answers `RetryPending`. **The shape is one nesting level
    /// from live**, which is why the gate is implemented from the bytecode
    /// rather than left until a corpus flow demands it — and why this test
    /// fails the day one does, telling the reader the golden can now judge it.
    #[test]
    fn no_corpus_parallel_child_declares_a_retry() {
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
                            if child.get("retry").is_some() {
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
                            if task.get("retry").is_some() {
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

        // The positive control, both spellings and both shapes -- an empty
        // result below must be a property of the corpus and not of the walker.
        for (ty, body) in [
            (
                "io.kestra.plugin.core.flow.Parallel",
                "    tasks:\n      - id: on\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n        retry:\n          type: constant\n          interval: PT1S\n      - id: off\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "io.kestra.plugin.core.flow.Dag",
                "    tasks:\n      - task:\n          id: on\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n          retry:\n            type: constant\n            interval: PT1S\n      - task:\n          id: off\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
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
                "{ty}: the walker must find the child with a retry and not the other"
            );
        }

        let mut offenders: Vec<String> = Vec::new();
        let mut both = 0usize;
        for (id, source) in every_corpus_flow() {
            let has_pd = [
                "core.flow.Parallel",
                "core.flow.Dag",
                "core.parallel",
                "core.dag",
            ]
            .iter()
            .any(|k| source.contains(k));
            if has_pd && source.contains("retry:") {
                both += 1;
            }
            let Ok(doc) = serde_norway::from_str::<serde_json::Value>(&source) else {
                continue;
            };
            let mut here = Vec::new();
            walk(&doc, &mut here);
            offenders.extend(here.into_iter().map(|t| format!("{id}/{t}")));
        }
        // **The number that can disagree.** A bare `offenders.is_empty()` is
        // unfalsifiable alone -- it passes on a corpus with no `Parallel` in it
        // at all -- so the wider population is asserted beside it.
        assert_eq!(
            both, 3,
            "corpus flows declaring BOTH a Parallel/Dag and a retry: anywhere. \
             If this moves, re-read whether the new one puts the retry on a \
             DIRECT child"
        );
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "a corpus `Parallel` or `Dag` child declares a `retry:`, so \
             Kestra's second gate -- `hasFailedNoRetry` at bci 120, egret's \
             `Current::RetryPending` -- is now reachable from the oracle and a \
             golden entry can judge it. Re-read \
             `a_dag_node_awaiting_a_retry_does_not_fail_its_siblings` against \
             that entry rather than against the bytecode alone"
        );
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
            // 9 -> 10 at WorkingDirectory Task 8: `working-directory-real-runif-false`.
            with_run_if,
            10,
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
            // 9 -> 10 at WorkingDirectory Task 8: `working-directory-real-runif-false`.
            with_run_if,
            10,
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
    // ----------------------------------------------------------------------
    // `Pause` — W8 Task 3
    // ----------------------------------------------------------------------

    /// [`round`], at a `now` the caller chooses.
    ///
    /// `round` pins `T0`, which is the right default for every rule that does
    /// not read the clock and the wrong one for every rule that does: with
    /// `now` fixed at `Tick(0)` a deadline of `started + d`, of `Tick(0) + d`
    /// and of `now + d` are the SAME NUMBER, and a fixture that cannot tell
    /// three rules apart cannot testify about any of them.
    fn round_at(plan: &Plan, state: &mut ExecutionState, now: Tick) -> Vec<Command> {
        let cmds = advance(plan, state, now);
        for cmd in &cmds {
            apply(state, cmd);
        }
        cmds
    }

    /// Drive `advance` to a fixed point **with a clock that moves the way
    /// `oracle-exec/driver.rs::pass_time` moves it**, completing every
    /// `Runnable` that goes `Running` with `outcome`.
    ///
    /// Returns the final state and the `now` it ended at.
    ///
    /// A fixed `now` cannot testify about a `Pause` at all, and the first draft
    /// of these tests learned that the hard way: with the clock pinned, the
    /// attempt opens at `now` and the deadline is `now + d`, so **every**
    /// bounded pause is still waiting whatever number is chosen. The
    /// environment has to do what the driver does — when nothing else can move,
    /// jump to the EARLIEST deadline still ahead, and never further.
    ///
    /// `pass_time` reads `not_before_ms` with **no state term**, so this reads
    /// `TaskRun::not_before` with none either: a fixture that only woke runs it
    /// believed were paused would be asserting its own belief. An unbounded
    /// pause writes no `not_before`, which is exactly why nothing here can
    /// wake one.
    fn run_to_fixpoint_passing_time(plan: &Plan, outcome: RunState) -> (ExecutionState, Tick) {
        let mut state = ExecutionState::new(plan);
        let mut now = T0;
        for _ in 0..1000 {
            if !round_at(plan, &mut state, now).is_empty() {
                continue;
            }
            let in_flight = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && state.runs[ix].history.last() == Some(&RunState::Running)
            });
            if let Some(ix) = in_flight {
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                );
                continue;
            }
            // `driver::pass_time`: the earliest deadline still ahead of `now`,
            // over every run, with no state term at all.
            match state
                .runs
                .iter()
                .filter_map(|r| r.not_before)
                .filter(|t| *t > now)
                .min()
            {
                Some(next) => now = next,
                None => return (state, now),
            }
        }
        panic!("did not settle in 1000 rounds");
    }

    /// A one-task flow whose `Pause` carries exactly `props`.
    fn pause_flow(props: &str) -> String {
        format!(
            "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: gate\n    \
             type: io.kestra.plugin.core.flow.Pause\n{props}  - id: after\n    \
             type: io.kestra.plugin.core.debug.Return\n    format: done\n"
        )
    }

    /// **`PAUSED` goes on the run and NOT on the attempt**, which is the one
    /// thing `Command::PauseRun` exists for.
    ///
    /// Derived rather than remembered: `PAUSED` is in 20 of the golden's 840
    /// run histories and **0** of its 878 attempt histories
    /// (`the_golden_populations_are_derived_recursed_and_unrecursed` pins the
    /// denominators), and `valids/pause-delay` is that shape — run
    /// `CREATED RUNNING PAUSED SUCCESS`, attempt `CREATED RUNNING SUCCESS`.
    ///
    /// Four things are asserted here and each has a rule that would break it
    /// alone: the attempt (a plain `SetRunState` would carry `PAUSED` onto it),
    /// the absence of `SUBMITTED` (planning a `Pause` `Runnable` would emit
    /// one), the execution's own `PAUSED`, and `not_before` (a bound written
    /// nowhere means the environment never learns the clock has to move).
    #[test]
    fn a_pause_enters_paused_without_putting_it_on_the_attempt() {
        let plan = plan_of("valids/pause-delay");
        let state = run_to_fixpoint(&plan, RunState::Success);

        assert_eq!(
            run_ids(&plan, &state),
            ["pause"],
            "the successor must not be created while the pause is still paused"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED"],
            "the run takes PAUSED"
        );
        assert_eq!(
            wire(&state.runs[0].attempts[0].history),
            ["CREATED", "RUNNING"],
            "the attempt does not: 0 of the golden's 878 attempt histories hold PAUSED"
        );
        assert_eq!(state.runs[0].attempts.len(), 1, "one attempt, still open");
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "PAUSED"],
            "the execution takes PAUSED in the same round the run does"
        );
        assert_eq!(
            state.runs[0].not_before,
            Some(Tick(1_000)),
            "`pauseDuration: PT1S` off an attempt that started at Tick(0)"
        );
    }

    /// **The wait is measured from the attempt's start, not from the
    /// execution's origin and not from the round the pause is entered in.**
    ///
    /// The three rules are separated by construction. The attempt opens at
    /// `Tick(500)`, the pause is entered at `Tick(700)`, and `pauseDuration` is
    /// `PT1S`, so:
    ///
    /// | rule | deadline |
    /// |---|---|
    /// | `attempt_started_at + d` | **1500** |
    /// | `Tick(0) + d` | 1000 |
    /// | `now + d` | 1700 |
    ///
    /// Every one of the three is a plausible implementation and only one of
    /// them is 1500. With the clock pinned at `T0` all three read 1000, which
    /// is why [`round_at`] exists.
    #[test]
    fn the_wait_is_measured_from_the_attempt_and_not_from_the_execution_origin() {
        let plan = plan_of("valids/pause-delay");
        let mut state = ExecutionState::new(&plan);

        round_at(&plan, &mut state, Tick(500)); // CreateRun
        round_at(&plan, &mut state, Tick(500)); // RUNNING, attempt opens at 500
        assert_eq!(
            state.runs[0].attempt_started_at(),
            Some(Tick(500)),
            "the fixture has not set up the thing it is about to measure"
        );
        round_at(&plan, &mut state, Tick(700)); // PAUSED, decided at a later now

        assert_eq!(state.runs[0].not_before, Some(Tick(1_500)));
        assert_ne!(
            state.runs[0].not_before,
            Some(Tick(1_000)),
            "not the origin"
        );
        assert_ne!(state.runs[0].not_before, Some(Tick(1_700)), "not `now`");

        // And the deadline DECIDES, rather than merely being stored: one tick
        // short of it the pause holds, and on it the pause ends.
        let mut short = state.clone();
        assert!(
            advance(&plan, &short, Tick(1_499)).is_empty(),
            "a pause one tick short of its deadline must not move"
        );
        round_at(&plan, &mut short, Tick(1_500));
        assert_eq!(
            wire(&short.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "SUCCESS"]
        );
    }

    /// **Which bound was written decides the terminal**, and the two flows that
    /// say so differ in nothing else.
    ///
    /// `valids/pause-delay` and `valids/pause-timeout` are both a `Pause`
    /// followed by a `core.debug.Return`, both bounded at one second, and their
    /// goldens differ on every field the bound touches: the run ends `SUCCESS`
    /// against `FAILED`, the execution ends `SUCCESS` against `FAILED`, and the
    /// successor has a run in one and no run at all in the other.
    ///
    /// The successor is asserted because it is the half a terminal-only
    /// assertion would miss: a `timeout:` that ended the pause `SUCCESS` would
    /// still produce two runs and a `SUCCESS` execution, i.e. exactly
    /// `pause-delay`'s trace under `pause-timeout`'s flow.
    #[test]
    fn the_bound_that_was_written_decides_the_terminal() {
        for (id, terminal, runs, execution) in [
            (
                "valids/pause-delay",
                "SUCCESS",
                vec!["pause", "last"],
                vec![
                    "CREATED",
                    "RUNNING",
                    "PAUSED",
                    "RESTARTED",
                    "RUNNING",
                    "SUCCESS",
                ],
            ),
            (
                "valids/pause-timeout",
                "FAILED",
                vec!["pause"],
                vec![
                    "CREATED",
                    "RUNNING",
                    "PAUSED",
                    "RESTARTED",
                    "RUNNING",
                    "FAILED",
                ],
            ),
        ] {
            let plan = plan_of(id);
            let (state, ended_at) = run_to_fixpoint_passing_time(&plan, RunState::Success);
            assert_eq!(
                ended_at,
                Tick(1_000),
                "{id}: both flows bound the pause at one second, so the clock \
                 must have been moved to exactly that deadline and no further"
            );
            assert_eq!(run_ids(&plan, &state), runs, "{id}: runs");
            assert_eq!(
                wire(&state.runs[0].history),
                ["CREATED", "RUNNING", "PAUSED", terminal],
                "{id}: the pause run"
            );
            assert_eq!(
                wire(&state.runs[0].attempts[0].history),
                ["CREATED", "RUNNING", terminal],
                "{id}: the attempt takes the terminal and never the PAUSED"
            );
            assert_eq!(wire(&state.history), execution, "{id}: the execution");
        }
    }

    /// **A resumed execution re-enters `RUNNING`, and it does so through
    /// `RESTARTED`.**
    ///
    /// W8 Task 2 left `advance`'s not-running-yet arm narrower than Kestra's
    /// `isCreated()` — `CREATED` only, where Kestra's is `CREATED ||
    /// RESTARTED` — and wrote down that whoever emitted `RESTARTED` would have
    /// to widen it. This is that test: with the `Restarted` term removed the
    /// execution stops at `… PAUSED RESTARTED` and never takes `RUNNING` again,
    /// and `last` is never created because `advance` returns before the walk.
    ///
    /// The assertion is the WHOLE history rather than a `contains`, because
    /// `RESTARTED` appearing somewhere is not the claim — the claim is that it
    /// appears once, between the pause and the re-run, which is the shape all
    /// twelve of the golden's `RESTARTED` executions have.
    #[test]
    fn a_resumed_pause_re_enters_running() {
        let plan = plan_of("valids/pause-delay");
        let (state, _) = run_to_fixpoint_passing_time(&plan, RunState::Success);
        assert_eq!(
            wire(&state.history),
            [
                "CREATED",
                "RUNNING",
                "PAUSED",
                "RESTARTED",
                "RUNNING",
                "SUCCESS"
            ]
        );
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "the successor is an ordinary `Runnable` and is submitted like one"
        );
    }

    /// **A `Pause` with neither `pauseDuration:` nor `timeout:` never resumes
    /// itself, at any clock.**
    ///
    /// This is the over-acceptance W8 Task 3 most risks and **the oracle cannot
    /// see it**: all seven corpus flows written that way reached `PAUSED` on
    /// the pinned artifact and stayed there for the full 90 s of the harvest,
    /// so every one of them is `not-standalone` in `corpus.json` and absent
    /// from `golden.json`. A pause that resumed itself after some default would
    /// leave the ratchet exactly where it is.
    ///
    /// Three rows, and the third is the control that can disagree:
    ///
    /// 1. neither bound, driven at a clock 100 000 years past the origin;
    /// 2. `valids/pause-test`, one of the seven real ones, at the same clock;
    /// 3. the SAME flow as row 1 with `pauseDuration: PT1S` added — which does
    ///    resume, at the same clock, through the same code.
    ///
    /// Without row 3 a `flowable::resume_paused` that answered `Waiting`
    /// unconditionally — or a `deadline` that returned `Unbounded` for
    /// everything — would pass
    /// rows 1 and 2 and be completely broken.
    #[test]
    fn an_unbounded_pause_never_resumes_itself() {
        for (label, src) in [
            ("no bound at all", pause_flow("")),
            ("valids/pause-test", corpus_source("valids/pause-test")),
        ] {
            let plan = build_plan(&src).unwrap_or_else(|e| panic!("{label}: {e}"));
            let paused: Vec<TaskIx> = (0..plan.tasks.len())
                .filter(|&ix| matches!(plan.tasks[ix].kind, TaskKind::Pause { .. }))
                .collect();
            assert_eq!(
                paused.len(),
                1,
                "{label}: the fixture must REACH the construct, or it proves nothing"
            );

            let (state, _) = run_to_fixpoint_passing_time(&plan, RunState::Success);
            let gate = state
                .runs
                .iter()
                .position(|r| paused.contains(&r.task))
                .unwrap_or_else(|| panic!("{label}: the pause got no run"));
            assert_eq!(
                wire(&state.runs[gate].history),
                ["CREATED", "RUNNING", "PAUSED"],
                "{label}: an unbounded pause must still be PAUSED"
            );
            assert_eq!(
                state.runs[gate].not_before, None,
                "{label}: no deadline may be written, or `pass_time` moves the clock for it"
            );
            assert_eq!(
                wire(&state.history),
                ["CREATED", "RUNNING", "PAUSED"],
                "{label}: the execution must not terminate"
            );
            assert!(
                advance(&plan, &state, Tick(i64::MAX)).is_empty(),
                "{label}: nothing this executor can be asked ends an unbounded pause"
            );
        }

        // The control. Same flow, same environment, one property added.
        let bounded = build_plan(&pause_flow("    pauseDuration: PT1S\n")).unwrap();
        let (state, ended_at) = run_to_fixpoint_passing_time(&bounded, RunState::Success);
        assert_eq!(ended_at, Tick(1_000), "the environment did move the clock");
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "SUCCESS"],
            "a BOUNDED pause at the same clock does resume — without this row the \
             assertions above are satisfied by a pause that never works at all"
        );
        assert_eq!(run_ids(&bounded, &state), ["gate", "after"]);
    }

    /// **A manual resume releases every paused `Pause` at once, through the
    /// natural end's door** (W7b Task 3 fix round 1).
    ///
    /// Three bounds, each paused at `T0`, long before any deadline: none, a
    /// `pauseDuration:` and a `timeout:` (whose natural end is `FAILED`). After
    /// [`release_paused_pauses`] the next `advance()` must emit exactly the
    /// commands a passed deadline emits -- `SetRunState { SUCCESS }` on the
    /// run, and `RESTARTED` on the execution -- and the flow must run on to
    /// `after`. The control: without the release, the same state at the same
    /// clock is still waiting.
    #[test]
    fn a_manual_resume_releases_every_pause_through_the_natural_end() {
        for props in ["", "    pauseDuration: PT1H\n", "    timeout: PT1H\n"] {
            let plan = build_plan(&pause_flow(props)).unwrap();
            let mut state = run_to_fixpoint(&plan, RunState::Success);
            assert_eq!(
                wire(&state.runs[0].history),
                ["CREATED", "RUNNING", "PAUSED"],
                "{props:?}"
            );
            assert!(
                advance(&plan, &state, T0).is_empty(),
                "{props:?}: the control waits"
            );

            assert_eq!(release_paused_pauses(&mut state, T0), vec![0], "{props:?}");
            let cmds = round(&plan, &mut state);
            assert!(
                cmds.contains(&Command::SetRunState {
                    run: 0,
                    state: RunState::Success
                }),
                "{props:?}: {cmds:?}"
            );
            assert!(
                cmds.contains(&Command::SetExecutionState {
                    state: RunState::Restarted
                }),
                "{props:?}: a resumed Pause owes the execution RESTARTED: {cmds:?}"
            );
            // On to the end, as `run_to_fixpoint` would from here.
            for _ in 0..100 {
                if round(&plan, &mut state).is_empty() {
                    let next = (0..state.runs.len())
                        .find(|&ix| state.runs[ix].history.last() == Some(&RunState::Running));
                    match next {
                        Some(ix) => apply(
                            &mut state,
                            &Command::SetRunState {
                                run: ix,
                                state: RunState::Success,
                            },
                        ),
                        None => break,
                    }
                }
            }
            assert_eq!(run_ids(&plan, &state), ["gate", "after"], "{props:?}");
            assert_eq!(
                wire(&state.runs[0].history),
                ["CREATED", "RUNNING", "PAUSED", "SUCCESS"],
                "{props:?}"
            );
            assert_eq!(state.history.last(), Some(&RunState::Success), "{props:?}");
            assert!(state.history.contains(&RunState::Restarted), "{props:?}");
            // A second resume finds nothing paused.
            assert!(
                release_paused_pauses(&mut state, T0).is_empty(),
                "{props:?}"
            );
        }
    }

    /// **A manual resume leaves a waiting `LoopUntil` alone**: it carries
    /// `Resume::Iterate`, and resume is a `Pause`'s.
    #[test]
    fn a_manual_resume_does_not_touch_a_loop_until() {
        // A paused run shaped as `LoopUntil` leaves it: `Iterate`, with its deadline.
        let mut state = run_to_fixpoint(&build_plan(&pause_flow("")).unwrap(), RunState::Success);
        state.runs[0].resume_with = Some(Resume::Iterate { at: Tick(500) });
        state.runs[0].not_before = Some(Tick(500));
        assert!(release_paused_pauses(&mut state, T0).is_empty());
        assert_eq!(
            state.runs[0].resume_with,
            Some(Resume::Iterate { at: Tick(500) })
        );
    }

    /// **A literal `pauseDuration:` needs no render; an expression does, and
    /// the RENDER is what decides.**
    ///
    /// The two rows are `valids/pause-delay` (`PT1S`) and
    /// `valids/pause-duration-from-input` (`{{inputs.delay}}`, declared default
    /// `PT1S`), and the render written in is `PT3S` — neither the source text
    /// nor the declared default. A deadline of 3 000 can therefore only have
    /// come from the render: 1 000 would be the default, and the source is not
    /// a duration at all.
    ///
    /// The literal row is the one that says the gate is narrow. A `Pause`
    /// added to the render tier unconditionally would stall `pause-delay` for
    /// ever under a caller that renders nothing, which is what `Step::Waiting`
    /// on an unanswered slot means — and eleven of the twelve baselined `Pause`
    /// entries are literal.
    #[test]
    fn a_literal_pause_duration_needs_no_render_and_an_expression_does() {
        let literal = plan_of("valids/pause-delay");
        let state = run_to_fixpoint(&literal, RunState::Success);
        assert_eq!(
            state.runs[0].rendered, None,
            "a literal must not be routed through the caller"
        );
        assert_eq!(state.runs[0].not_before, Some(Tick(1_000)));

        let expr = plan_of("valids/pause-duration-from-input");
        let mut state = ExecutionState::new(&expr);
        round(&expr, &mut state); // CreateRun
        round(&expr, &mut state); // RUNNING
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING"],
            "the expression is read at RUNNING, so the run must reach it first"
        );
        assert!(
            advance(&expr, &state, T0).is_empty(),
            "an unanswered render is a wait, not a default"
        );

        state.runs[0].rendered = Some("PT3S".to_string());
        round(&expr, &mut state);
        assert_eq!(
            state.runs[0].not_before,
            Some(Tick(3_000)),
            "3000 is the RENDER; 1000 would be the flow's declared default"
        );
    }

    /// **A `Pause` this executor does not reproduce is REFUSED, not planned.**
    ///
    /// Until the 2026-10-11 ruling it was planned an opaque `Runnable` with a
    /// `not_yet_enforced` warning, under `build_plan`'s standing rule for an
    /// unimplemented flowable. For `AllowFailure` and `WorkingDirectory` that
    /// rule is harmless: their opaque run is still claimed by nothing and the
    /// warning says so. For a `Pause` it was not: the opaque run got a
    /// `type_id`, the worker claimed it, `Claimed::input` failed with
    /// `NotALeaf` before recording an outcome, and the reaper released it on
    /// every working timeout, so the execution sat in `RUNNING` for ever. Now
    /// `spec::validate` reports `unsupported_pause_property` and
    /// `build_plan`'s `validate_for_run` gate refuses the flow, naming the
    /// property, before any run exists.
    ///
    /// Five rows, one per property plus `behavior:` written as an EXPRESSION
    /// (its value decides what the artifact does, and `pause_gap` decides at
    /// plan time, so the dynamic form must be refused just like the literal).
    ///
    /// The control is the last row: the same flow with none of the four is a
    /// `TaskKind::Pause`. Without it every row would pass under a `build_plan`
    /// that refused every `Pause`.
    #[test]
    fn a_pause_this_executor_does_not_reproduce_is_refused_at_plan() {
        let rows: [(&str, &str); 5] = [
            (
                "tasks",
                "    pauseDuration: PT1S\n    tasks:\n      - id: inner\n        \
                 type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "errors",
                "    pauseDuration: PT1S\n    errors:\n      - id: oops\n        \
                 type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            ("behavior", "    pauseDuration: PT1S\n    behavior: FAIL\n"),
            (
                "behavior",
                "    pauseDuration: PT1S\n    behavior: \"{{ inputs.b }}\"\n",
            ),
            (
                "onPause",
                "    pauseDuration: PT1S\n    onPause:\n      id: hello\n      \
                 type: io.kestra.plugin.core.debug.Return\n      format: x\n",
            ),
        ];
        for (property, props) in rows {
            let src = pause_flow(props);
            let err = match build_plan(&src) {
                Ok(_) => panic!("{property}: a pause carrying it must not plan"),
                Err(e) => e,
            };
            assert!(
                err.message.starts_with("unsupported_pause_property:"),
                "{property}: refused by the right finding: {}",
                err.message
            );
            assert!(
                err.message.contains(&format!("[tasks.gate.{property}]")),
                "{property}: the refusal names the property: {}",
                err.message
            );
        }

        // The control.
        let src = pause_flow("    pauseDuration: PT1S\n");
        let plan = build_plan(&src).unwrap();
        assert_eq!(
            plan.tasks[0].kind,
            TaskKind::Pause {
                duration: Some("PT1S".to_string())
            }
        );
        let report = crate::spec::validate(&crate::spec::parse(&src).unwrap());
        assert!(
            report.errors.is_empty()
                && !report
                    .warnings
                    .iter()
                    .any(|w| w.path.starts_with("tasks.gate")),
            "a pause this executor DOES reproduce is valid and silent: {report:?}"
        );
    }

    /// **Two pauses in one `Parallel` pause and resume independently, and the
    /// execution's history holds one `PAUSED` and one `RESTARTED`.**
    ///
    /// Two separate rules are driven here and neither has a corpus flow:
    ///
    /// * `parallel::drive_concurrent`'s rule 3 has a `Paused` arm. Without it a
    ///   `Pause` under a `Parallel`, a `Dag` or an `If` pauses and is never
    ///   asked again — a stall, not a wrong trace.
    /// * `pause::execution_state` suppresses a repeat. Without it the execution
    ///   reads `… PAUSED PAUSED …`, and no golden execution history repeats a
    ///   state for any construct.
    ///
    /// The two pauses are one second apart so that the resume round for each is
    /// a different round, which is what "independently" means and what a single
    /// shared deadline would hide.
    ///
    /// **And it is what separates the suppression from a stronger rule nobody
    /// wrote.** The result is one `PAUSED` and TWO `RESTARTED`: the pauses
    /// begin together, so the second `PAUSED` is an immediate repeat, and they
    /// end apart with `RUNNING` between, so the second `RESTARTED` is a
    /// separate event. A suppression keyed on "has this execution ever been
    /// restarted" would pass every other assertion here and be wrong.
    ///
    /// **It does NOT drive `pause_mark`'s suppression, and measuring that is
    /// what found the hole.** Mutation M8 deleted the `if` and this test stayed
    /// green: both pauses are reached by `drive_concurrent`'s rule 3 in ONE
    /// round, so the batch carries two `PauseRun`s and `pause_mark` answers
    /// once for it whether or not the guard is there. The round that can reach
    /// the guard is one where a pause begins while the execution is ALREADY
    /// `PAUSED`, which needs the two to begin in DIFFERENT rounds —
    /// `a_second_pause_beginning_later_does_not_repeat_the_execution_state`
    /// below.
    #[test]
    fn two_pauses_in_one_parallel_resume_independently() {
        let src = "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: both\n    \
                   type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
                   - id: quick\n        type: io.kestra.plugin.core.flow.Pause\n        \
                   pauseDuration: PT1S\n      - id: slow\n        \
                   type: io.kestra.plugin.core.flow.Pause\n        \
                   pauseDuration: PT2S\n";
        let plan = build_plan(src).expect("plans");
        let mut state = ExecutionState::new(&plan);

        // Far enough for both to be created, started and paused, and no
        // further: `advance` answers nothing once both are waiting.
        for _ in 0..10 {
            if round_at(&plan, &mut state, T0).is_empty() {
                break;
            }
        }
        assert_eq!(run_ids(&plan, &state), ["both", "quick", "slow"]);
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "RUNNING", "PAUSED"]
        );
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "RUNNING", "PAUSED"]
        );
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "PAUSED"],
            "one PAUSED, not two"
        );

        // The first deadline only.
        round_at(&plan, &mut state, Tick(1_000));
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "RUNNING", "PAUSED", "SUCCESS"]
        );
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "RUNNING", "PAUSED"],
            "the second pause's own deadline has not come"
        );

        let (state, ended_at) = run_to_fixpoint_passing_time(&plan, RunState::Success);
        assert_eq!(
            ended_at,
            Tick(2_000),
            "the clock stopped at the LATER of the two deadlines"
        );
        assert_eq!(
            wire(&state.runs[2].history),
            ["CREATED", "RUNNING", "PAUSED", "SUCCESS"]
        );
        assert_eq!(
            wire(&state.history),
            [
                "CREATED",
                "RUNNING",
                "PAUSED",
                "RESTARTED",
                "RUNNING",
                "RESTARTED",
                "RUNNING",
                "SUCCESS"
            ],
            "ONE `PAUSED` and TWO `RESTARTED`, which is the shape the \
             suppression actually has and not the one this assertion was first \
             written for. The two pauses BEGIN in the same round, so a second \
             `PAUSED` would be an immediate repeat and is suppressed; they END \
             in different rounds, with `RUNNING` between them, so the second \
             `RESTARTED` is a separate event rather than a repeat. Suppressing \
             that one too would be inventing a rule that says an execution can \
             only be restarted once"
        );
    }

    /// **A second pause that begins a round LATER does not repeat the
    /// execution's `PAUSED`**, which is the only round `pause_mark`'s
    /// suppression can reach.
    ///
    /// **This test exists because mutation M8 survived without it.** Deleting
    /// the `if state.history.last() != Some(&mark)` left the whole crate green:
    /// `two_pauses_in_one_parallel_resume_independently` puts both pauses in
    /// one batch, and `pause_mark` answers once per batch whatever the guard
    /// says. A guard nobody has seen fail is a guard nobody has seen, and this
    /// package has paid for that ten times.
    ///
    /// The fixture staggers them by nesting one inside a `Sequential`: the
    /// sibling `Pause` is reached by `drive_concurrent`'s rule 3 immediately,
    /// while the nested one needs two further rounds (the `Sequential` must go
    /// `RUNNING`, then create and start its child), so the second `PauseRun` is
    /// emitted in a round where the execution's last state is already `PAUSED`.
    ///
    /// The assertion is the whole history, and the `RESTARTED` half is the
    /// control: it is NOT suppressed, because `RUNNING` sits between the two
    /// resumes. A test asserting only "one PAUSED" would also pass under a
    /// guard that suppressed everything.
    #[test]
    fn a_second_pause_beginning_later_does_not_repeat_the_execution_state() {
        let src = "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: both\n    \
                   type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
                   - id: early\n        type: io.kestra.plugin.core.flow.Pause\n        \
                   pauseDuration: PT3S\n      - id: wrapper\n        \
                   type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          \
                   - id: late\n            type: io.kestra.plugin.core.flow.Pause\n            \
                   pauseDuration: PT1S\n";
        let plan = build_plan(src).expect("plans");

        // Drive by hand to the standstill so the ROUNDS can be inspected, which
        // is what this test is about.
        let mut state = ExecutionState::new(&plan);
        let mut marks: Vec<(usize, Vec<&'static str>)> = Vec::new();
        for r in 0..12 {
            let cmds = round_at(&plan, &mut state, T0);
            if cmds.is_empty() {
                break;
            }
            let emitted: Vec<&'static str> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::SetExecutionState { state } => Some(state.wire()),
                    _ => None,
                })
                .collect();
            let paused = cmds
                .iter()
                .filter(|c| matches!(c, Command::PauseRun { .. }))
                .count();
            if paused > 0 {
                marks.push((r, emitted));
            }
        }

        // The fixture must actually stagger them, or it is the parallel test
        // again under a different name.
        assert_eq!(
            marks.len(),
            2,
            "two rounds must each emit exactly one `PauseRun`: {marks:?}"
        );
        assert_ne!(marks[0].0, marks[1].0, "they must be DIFFERENT rounds");
        assert_eq!(
            marks[0].1,
            ["PAUSED"],
            "the first pause moves the execution: {marks:?}"
        );
        assert_eq!(
            marks[1].1,
            Vec::<&str>::new(),
            "the second does NOT, because the execution is already PAUSED — \
             this is the round `pause_mark`'s suppression exists for, and \
             mutation M8 survives without it: {marks:?}"
        );

        let (state, _) = run_to_fixpoint_passing_time(&plan, RunState::Success);
        assert_eq!(
            wire(&state.history),
            [
                "CREATED",
                "RUNNING",
                "PAUSED",
                "RESTARTED",
                "RUNNING",
                "RESTARTED",
                "RUNNING",
                "SUCCESS"
            ],
            "one PAUSED — and TWO RESTARTED, which is the control: the guard \
             suppresses an immediate repeat and nothing else"
        );
        assert_eq!(run_ids(&plan, &state), ["both", "early", "wrapper", "late"]);
    }

    /// Every `behavior:` line in the corpus, by the construct that owns it.
    ///
    /// **This derivation is what fix round 1 lacked and fix round 2 reads the
    /// other way round.** Round 1 found that the corpus's only `Pause`
    /// `behavior:` is an expression and concluded that the literal case was
    /// therefore not worth refusing. The same fact says the opposite: **one
    /// instance is not a sample of a four-valued construct**, and the artifact
    /// measurement (five runs per form) shows it honours the value in every
    /// form, so the corpus's single `FAIL` is the one value where a
    /// "dynamic ⇒ FAILED" rule and the truth happen to agree.
    ///
    /// It is kept because it is the population every claim about `behavior:`
    /// in this crate is made over, and it fails the day a second one appears —
    /// at which point the agreement above stops being a coincidence somebody
    /// has to re-derive.
    #[test]
    fn every_corpus_behavior_line_is_accounted_for() {
        let (mut on_pause_dynamic, mut on_pause_literal, mut elsewhere) = (0usize, 0usize, 0usize);
        let mut names = Vec::new();
        for (id, src) in every_corpus_flow() {
            elsewhere += src.matches("behavior:").count();
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            for (task_id, pause, _) in corpus_pauses(&flow) {
                let Some(b) = pause.behavior.as_ref() else {
                    continue;
                };
                elsewhere -= 1;
                names.push(format!("{id}:{task_id}"));
                match b.source().contains('{') {
                    true => on_pause_dynamic += 1,
                    false => on_pause_literal += 1,
                }
            }
        }
        assert_eq!(
            (on_pause_dynamic, on_pause_literal, elsewhere),
            (1, 0, 30),
            "one dynamic `behavior:` on a `Pause`, NO literal one, and 30 lines \
             owned by `concurrency:`, `retry:` or `sla:` — three constructs \
             whose `behavior:` is a different field with different values. A \
             literal one appearing here does not make `pause_gap` stricter; it \
             makes the corpus able to SEE the difference between honouring the \
             value and refusing the construct, which today it cannot"
        );
        assert_eq!(names, ["valids/pause-behavior:pause"]);
    }

    // ----------------------------------------------------------------------
    // W8 Task 4: `LoopUntil` / `WaitFor`
    // ----------------------------------------------------------------------

    /// A one-`LoopUntil` flow: `props` go on the loop, `body` is its `tasks:`.
    ///
    /// The default body is a single `core.debug.Return`, which
    /// [`run_loop_until`] completes with the outcome it is given -- so the body
    /// is a knob (does the iteration succeed?) and `props` is the other one.
    fn loop_until_flow(props: &str, body: &str) -> String {
        format!(
            "id: lu\nnamespace: io.kestra.tests\ntasks:\n  - id: loop\n    \
             type: io.kestra.plugin.core.flow.LoopUntil\n{props}    tasks:\n{body}"
        )
    }

    const ONE_RETURN: &str = "      - id: body\n        \
                              type: io.kestra.plugin.core.debug.Return\n        \
                              format: x\n";

    /// [`run_to_fixpoint_passing_time`], plus the one render a `LoopUntil`
    /// needs -- and the answers are a LIST, consumed one per render.
    ///
    /// **A list and not a constant, because the condition is asked once per
    /// iteration and the whole construct is about what happens when the answer
    /// CHANGES.** A fixture that returned one string for every render would
    /// pass identically against an implementation that cached the first answer
    /// for ever, which is `Command::RestartLoop` clearing `rendered` -- so it
    /// would be a fixture that cannot see the rule it is placed to watch. The
    /// last entry repeats once the list runs out.
    ///
    /// It applies the executor's OWN gate,
    /// [`loop_condition_is_asked`](crate::executor::loop_condition_is_asked),
    /// rather than rendering whatever is unrendered -- the same call
    /// `worker::render`'s tier B makes, not a copy of it. Without the gate this
    /// fixture renders at the START of every iteration including the one the
    /// loop is already at its limit for, and `asked` -- the sharpest assertion
    /// several of these tests make -- would count an evaluation Kestra never
    /// performs. A fixture that cannot tell those apart cannot testify about
    /// either.
    fn run_loop_until(
        plan: &Plan,
        outcome: RunState,
        answers: &[&str],
    ) -> (ExecutionState, Tick, usize) {
        run_loop_until_with_slack(plan, outcome, answers, 0)
    }

    /// [`run_loop_until`], with the driver's OWN millisecond of slack.
    ///
    /// **`oracle-exec/driver.rs::pass_time` does not land on a deadline, it
    /// lands one millisecond past one** — `.bind(shortfall + 1)`, "because
    /// `Tick` is integral milliseconds and `reopen_due`'s predicate is
    /// `not_before <= now`, so landing exactly on the boundary would depend on
    /// a rounding this driver does not control". Every other fixture in this
    /// file lands exactly on it.
    ///
    /// For every rule in this module but one that difference is invisible,
    /// because the comparisons are `now < at` and a deadline reached is a
    /// deadline reached. **`maxDuration` is the exception**, because its
    /// comparison is against a bound the pauses do not divide evenly: with
    /// `PT3S` and `PT1S` the slack accumulates, and `created + 3000 < now` is
    /// first true at iteration **5** on the exact clock and at iteration **4**
    /// on the driver's. Both are this implementation; the parameter is what
    /// lets one test say so instead of two artifacts disagreeing.
    fn run_loop_until_with_slack(
        plan: &Plan,
        outcome: RunState,
        answers: &[&str],
        slack: i64,
    ) -> (ExecutionState, Tick, usize) {
        let mut state = ExecutionState::new(plan);
        let mut now = T0;
        let mut asked = 0usize;
        for _ in 0..1000 {
            if !round_at(plan, &mut state, now).is_empty() {
                continue;
            }
            // W3c's tier 2a, with the same identity short-circuit
            // `run_to_fixpoint_rendering_run_if` uses: a `runIf:` with no `{{`
            // renders to itself, which is how a literal `runIf: "false"` needs
            // no table here. It goes FIRST, because Kestra decides a `runIf:`
            // before it resolves anything else about the task.
            let unrendered_run_if = (0..state.runs.len()).find(|&ix| {
                plan.tasks[state.runs[ix].task].run_if.is_some()
                    && state.runs[ix].history.last() == Some(&RunState::Created)
                    && state.runs[ix].rendered_run_if.is_none()
            });
            if let Some(ix) = unrendered_run_if {
                let source = plan.tasks[state.runs[ix].task]
                    .run_if
                    .clone()
                    .expect("filtered on Some above");
                assert!(
                    !source.contains("{{"),
                    "this fixture renders a `runIf:` only by identity; '{source}' needs a table"
                );
                state.runs[ix].rendered_run_if = Some(source);
                continue;
            }
            let unrendered = (0..state.runs.len()).find(|&ix| {
                state.runs[ix].history.last() == Some(&RunState::Running)
                    && state.runs[ix].rendered.is_none()
                    && crate::executor::loop_condition_is_asked(
                        plan,
                        &state,
                        ix,
                        state.runs[ix].task,
                        now,
                    )
            });
            if let Some(ix) = unrendered {
                let answer = answers[asked.min(answers.len() - 1)];
                asked += 1;
                state.runs[ix].rendered = Some(answer.to_string());
                continue;
            }
            let in_flight = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && state.runs[ix].history.last() == Some(&RunState::Running)
            });
            if let Some(ix) = in_flight {
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
                Some(next) => now = Tick(next.0 + slack),
                None => return (state, now, asked),
            }
        }
        panic!("did not settle in 1000 rounds");
    }

    /// **The condition is asked, and a truthy answer stops the loop where it
    /// stands** -- one iteration, no `PAUSED`, and no second run of the body.
    ///
    /// `valids/waitfor` is this shape in the corpus: its condition is truthy on
    /// the first iteration and its golden run is `CREATED RUNNING SUCCESS` with
    /// no pause at all.
    ///
    /// **The negative half is the point.** A `LoopUntil` that re-entered
    /// whatever the condition said would still terminate on every corpus flow
    /// that has a `maxIterations:` or a `maxDuration:`, and would still end
    /// `SUCCESS` -- it would simply run the body more times, which no run
    /// history records once the body's runs are thrown away and re-created. So
    /// the body's ATTEMPT COUNT is asserted beside the histories: it is 1, and
    /// a second iteration would make it 1 again, which is exactly why
    /// `asked` -- how many times the fixture was asked for the condition -- is
    /// asserted instead. One iteration asks once.
    #[test]
    fn a_truthy_condition_stops_a_loop_until_where_it_stands() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      interval: PT1S\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, now, asked) = run_loop_until(&plan, RunState::Success, &["true"]);

        assert_eq!(run_ids(&plan, &state), ["loop", "body"]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "SUCCESS"],
            "a truthy condition ends the loop in the round its body finished"
        );
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "the body ran exactly once and as a runnable"
        );
        assert_eq!(asked, 1, "one iteration asks the condition once");
        assert_eq!(now, T0, "nothing waited, so the clock never had to move");
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "SUCCESS"],
            "and the execution never entered PAUSED"
        );
    }

    /// **A `PAUSED` run whose kind is neither `Pause` nor `LoopUntil` must
    /// still leave `PAUSED`** — the class W8 Task 4 fix round 1 half-closed.
    ///
    /// Fix round 1 added a `LoopUntil` arm beside the `Pause` one. That fixed
    /// the two constructs that exist and left the SHAPE: two catch-alls
    /// (`parallel.rs`'s `_ => continue`, `sequential.rs`'s `_ => Waiting`)
    /// keyed on a hand-written list of kinds, so a THIRD pausing construct
    /// stalls for ever and nothing fails.
    ///
    /// The probe is a real [`Command::PauseRun`] applied to a `Runnable`'s run
    /// — which is what a new pausing kind's `descend` arm would emit before
    /// anybody remembered to extend the list — driven under all three
    /// containers, with a `Pause` in the same fixture as the control.
    #[test]
    fn a_paused_run_of_any_kind_leaves_paused() {
        let bodies = [
            (
                "Sequential",
                "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
                 type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: t\n        \
                 type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "Parallel",
                "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
                 type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: t\n        \
                 type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            ),
            (
                "Dag",
                "id: p\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
                 type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          \
                 id: t\n          type: io.kestra.plugin.core.debug.Return\n          format: x\n",
            ),
        ];
        for (label, src) in bodies {
            let plan = build_plan(src).unwrap_or_else(|e| panic!("{label}: {e}"));
            let mut state = ExecutionState::new(&plan);
            // Drive until the leaf is RUNNING, then pause it the way a new
            // pausing construct would.
            let mut now = T0;
            let leaf = loop {
                let cmds = round_at(&plan, &mut state, now);
                if let Some(ix) = (0..state.runs.len()).find(|&ix| {
                    matches!(
                        plan.tasks[state.runs[ix].task].kind,
                        TaskKind::Runnable { .. }
                    ) && state.runs[ix].history.last() == Some(&RunState::Running)
                }) {
                    break ix;
                }
                assert!(!cmds.is_empty(), "{label}: never reached a running leaf");
            };
            apply(
                &mut state,
                &Command::PauseRun {
                    run: leaf,
                    // The probe names a resumer, because since fix round 2 it
                    // CANNOT avoid naming one -- which is the whole fix. A
                    // third pausing construct's `descend` arm now faces the
                    // same requirement at the compiler rather than at a list.
                    resume: crate::executor::Resume::Terminal {
                        at: Tick(10),
                        state: RunState::Success,
                    },
                },
            );
            assert_eq!(state.runs[leaf].history.last(), Some(&RunState::Paused));

            now = Tick(1_000);
            let mut emitted = 0;
            for _ in 0..20 {
                emitted += round_at(&plan, &mut state, now).len();
            }
            assert!(
                state.runs[leaf].history.last() != Some(&RunState::Paused),
                "{label}: the run is STILL `PAUSED` {:?} after 20 rounds past its \
                 deadline and {emitted} command(s) — nothing in this executor will \
                 ever move it, and no assertion but this one can see that",
                wire(&state.runs[leaf].history)
            );
        }
    }

    /// **Every `PAUSED` run carries the [`Resume`] that will end it** — the
    /// invariant [`flowable::resume_paused`](crate::executor::flowable) cites
    /// by name and which, until fix round 3, **did not exist**.
    ///
    /// `resume_paused`'s doc said of its `None` arm: *"`Waiting` on `None` is
    /// unreachable rather than defensive … `every_paused_run_names_its_resumer`
    /// is what keeps it unreachable."* No test of that name had been written —
    /// the arm that stalls a run for ever was defended by a **citation** rather
    /// than by a program, which is the same class as a stale figure and harder
    /// to see, because a name that reads like a test is indistinguishable from
    /// one until it is searched for. This is that program.
    ///
    /// **The observation count is asserted per shape, and that is the point.**
    /// An invariant driven over fixtures that never pause holds *vacuously*, so
    /// a driver that broke would pass this silently. Each shape must be seen to
    /// reach `PAUSED` at least once, or the harness failed rather than the
    /// invariant held.
    ///
    /// It is checked after **every** round and not on the final state:
    /// [`Command::RestartLoop`] clears `resume_with` in the same batch it
    /// pushes `RUNNING`, so an iterating `LoopUntil` is never `PAUSED` at a
    /// fixed point and a final-state check would observe nothing at all.
    ///
    /// The mutation is **MH1** — [`Command::PauseRun`] storing `None` instead
    /// of the [`Resume`] it was handed, which is precisely the row migration
    /// 0142 refuses at the other end of the round trip.
    #[test]
    fn every_paused_run_names_its_resumer() {
        /// The invariant itself, returning how many `PAUSED` runs it saw so the
        /// caller can refuse a vacuous pass.
        fn observe(label: &str, plan: &Plan, state: &ExecutionState) -> usize {
            let mut seen = 0;
            for ix in 0..state.runs.len() {
                if state.runs[ix].history.last() == Some(&RunState::Paused) {
                    seen += 1;
                    assert!(
                        state.runs[ix].resume_with.is_some(),
                        "{label}: run {ix} ('{}') is PAUSED naming no resumer — \
                         `flowable::resume_paused` answers `Waiting` to this run for \
                         ever, which is byte for byte the stall fix round 2 deleted",
                        plan.tasks[state.runs[ix].task].id
                    );
                }
            }
            seen
        }

        let under_parallel = "id: lu\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
             type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: loop\n        \
             type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{ done }}\"\n        \
             checkFrequency:\n          interval: PT1S\n          maxIterations: 3\n        \
             tasks:\n          - id: body\n            \
             type: io.kestra.plugin.core.debug.Return\n            format: x\n";
        let loop_props = "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
                          interval: PT1S\n      maxIterations: 3\n";

        // **All four producers of `PAUSED`, not one.** `Resume` has three
        // variants and each is reached by a different construct: `Terminal`
        // twice (the two bounds choose different terminals), `Never` once, and
        // `Iterate` from the loop — under both walks, because `sequential::walk`
        // and `parallel::drive_concurrent` reach `resume_paused` separately.
        let shapes = [
            (
                "a Pause bounded by pauseDuration",
                pause_flow("    pauseDuration: PT1S\n"),
            ),
            (
                "a Pause bounded by timeout",
                pause_flow("    timeout: PT1S\n"),
            ),
            ("an unbounded Pause", pause_flow("")),
            (
                "a LoopUntil under a Sequential",
                loop_until_flow(loop_props, ONE_RETURN),
            ),
            ("a LoopUntil under a Parallel", under_parallel.to_string()),
        ];

        for (label, src) in &shapes {
            let plan = build_plan(src).unwrap_or_else(|e| panic!("{label}: {e}"));
            let mut state = ExecutionState::new(&plan);
            let mut now = T0;
            let mut seen = 0usize;
            for _ in 0..1000 {
                seen += observe(label, &plan, &state);
                if !round_at(&plan, &mut state, now).is_empty() {
                    continue;
                }
                let unrendered = (0..state.runs.len()).find(|&ix| {
                    state.runs[ix].history.last() == Some(&RunState::Running)
                        && state.runs[ix].rendered.is_none()
                        && crate::executor::loop_condition_is_asked(
                            &plan,
                            &state,
                            ix,
                            state.runs[ix].task,
                            now,
                        )
                });
                if let Some(ix) = unrendered {
                    // Always falsy: this fixture wants the loop to ITERATE, and
                    // a loop that stops on its first answer never pauses.
                    state.runs[ix].rendered = Some("false".to_string());
                    continue;
                }
                let in_flight = (0..state.runs.len()).find(|&ix| {
                    matches!(
                        plan.tasks[state.runs[ix].task].kind,
                        TaskKind::Runnable { .. }
                    ) && state.runs[ix].history.last() == Some(&RunState::Running)
                });
                if let Some(ix) = in_flight {
                    apply(
                        &mut state,
                        &Command::SetRunState {
                            run: ix,
                            state: RunState::Success,
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
                    Some(next) => now = next,
                    // An unbounded pause ends here, with the state it stalls
                    // in — so it is observed once more before the shape is
                    // dropped, which is the only round it is ever `PAUSED` at.
                    None => {
                        seen += observe(label, &plan, &state);
                        break;
                    }
                }
            }
            assert!(
                seen > 0,
                "{label}: {seen} PAUSED observation(s) — this shape never reached a \
                 pause, so the invariant above held vacuously and this test is \
                 measuring the driver rather than the executor"
            );
        }
    }

    /// **A `LoopUntil` inside a `Parallel` or a `Dag` must iterate, and until
    /// fix round 1 it stalled at its first `PAUSED` for ever.**
    ///
    /// `parallel::drive_concurrent`'s `Some(PAUSED)` arm was guarded on
    /// `TaskKind::Pause`, so a `PAUSED` `LoopUntil` fell to `_ => continue` and
    /// nothing ever asked it whether its interval had elapsed. **`Dag` shares
    /// `drive_concurrent`**, so it was two constructs and not one.
    ///
    /// This is the hazard W8's plan states in writing — *a new non-terminal
    /// state breaks a predicate somewhere* — and the reason six enumeration
    /// methods missed it is on the face of it: **the guard is keyed on
    /// `TaskKind`, not on state**, so a sweep over "what reads `RunState`" and
    /// a sweep over "what the new variant reaches" both walk past it.
    ///
    /// **The `Sequential` row is the control**, and it is not decoration: it is
    /// the same loop, the same bounds and the same fixture, one word different,
    /// and it passed throughout. Without it this test would also pass against a
    /// build where nothing iterates at all.
    #[test]
    fn a_loop_until_inside_a_parallel_or_a_dag_iterates_like_one_inside_a_sequential() {
        // Written out per container rather than assembled, because the one
        // shape difference -- a `Dag` nests its children under `- task:` --
        // is exactly the kind of thing a clever re-indent gets wrong while
        // still producing a flow that parses into something ELSE.
        let seq = "id: lu\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
             type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: loop\n        \
             type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{ done }}\"\n        \
             checkFrequency:\n          interval: PT1S\n          maxIterations: 2\n        \
             tasks:\n          - id: body\n            \
             type: io.kestra.plugin.core.debug.Return\n            format: x\n";
        let par = seq.replace("flow.Sequential", "flow.Parallel");
        let dag = "id: lu\nnamespace: io.kestra.tests\ntasks:\n  - id: outer\n    \
             type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          \
             id: loop\n          type: io.kestra.plugin.core.flow.LoopUntil\n          \
             condition: \"{{ done }}\"\n          checkFrequency:\n            \
             interval: PT1S\n            maxIterations: 2\n          tasks:\n            \
             - id: body\n              type: io.kestra.plugin.core.debug.Return\n              \
             format: x\n";

        for (label, src) in [
            ("Sequential", seq.to_string()),
            ("Parallel", par),
            ("Dag", dag.to_string()),
        ] {
            let plan = build_plan(&src).unwrap_or_else(|e| panic!("{label}: {e}"));
            let (state, now, asked) = run_loop_until(&plan, RunState::Success, &["false"]);
            let loop_run = (0..state.runs.len())
                .find(|&ix| {
                    matches!(
                        plan.tasks[state.runs[ix].task].kind,
                        TaskKind::LoopUntil { .. }
                    )
                })
                .unwrap_or_else(|| panic!("{label}: no LoopUntil run"));
            assert_eq!(
                wire(&state.runs[loop_run].history),
                ["CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
                "{label}: three iterations under `maxIterations: 2`, exactly as the \
                 `Sequential` control. A guard keyed on `TaskKind::Pause` leaves this \
                 at `CREATED RUNNING PAUSED` for ever"
            );
            assert_eq!(now, Tick(2000), "{label}: two pauses of PT1S");
            assert_eq!(asked, 2, "{label}");
        }
    }

    /// **A `LoopUntil` with no `checkFrequency.interval:` waits Kestra's
    /// `@Builder.Default` of ONE MINUTE**, and nothing else in this crate can
    /// say so.
    ///
    /// `CheckFrequency.interval` is `@NotNull @Builder.Default
    /// Property.ofValue(Duration.ofMinutes(1))` — a bean default rather than a
    /// parse rule, so an absent key is `None` in `models` and becomes
    /// [`DEFAULT_CHECK_INTERVAL`](crate::executor::plan) in `build_plan`.
    ///
    /// **Every one of the ten corpus flows of this construct writes an
    /// `interval:`**, so the default is unreachable from the corpus and every
    /// other fixture in this file supplies one too — which is the operand hole
    /// fix round 1 closes: the two-sided pairs on the truthiness operator, and
    /// the pause-timing assertions built on `PT1S`, all say where the deadline
    /// is COMPUTED and nothing about where the interval came FROM. Mutating the
    /// constant to 7 s survived the whole suite.
    #[test]
    fn a_loop_until_with_no_interval_waits_kestras_default_minute() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      maxIterations: 1\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, now, _) = run_loop_until(&plan, RunState::Success, &["false"]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
            "two iterations under `maxIterations: 1`"
        );
        assert_eq!(
            now,
            Tick(60_000),
            "and the ONE pause between them is `PT1M` — Kestra's `CheckFrequency` \
             default, not this executor's invention"
        );
    }

    /// **The other two thirds of `isTerminatedNoFail`**, supplied and pinned.
    ///
    /// `State.java:292` is `WARNING || SUCCESS || RETRIED || SKIPPED ||
    /// RESUBMITTED`, and intersected with what `RunState` can hold that is
    /// `{SUCCESS, RETRIED, SKIPPED}`; `FAILED` and `KILLED` are
    /// `isTerminatedInError`. Fix round 1 supplied `SKIPPED` and left the other
    /// two **un-supplied**: every fixture handed the loop a `SUCCESS` child, so
    /// `MK1` (drop `Killed` from the exclusion) and `MR1` (add `Retried` to it)
    /// both survived the whole suite at **780/0**.
    ///
    /// That is the operand rule one more time, and the fix is the same one:
    /// name the competing rules, then supply a discriminating row for each.
    /// The rules here are per-state, so the rows are per-state — `RunState`'s
    /// five terminals are now driven **one each**, and the two-sided pair is
    /// the point: `KILLED` must STOP the loop and `RETRIED` must NOT.
    ///
    /// **Reachability, measured rather than read.** `KILLED` reaches a
    /// `LoopUntil` child through `worker::cancel`, which the kill path fires on
    /// a task-level `timeout:` expiring — a `LoopUntil` child may carry one.
    /// `RETRIED` is written by `retry::scan` for a `CREATE_NEW_EXECUTION`
    /// behaviour, which is a `Runnable`'s `retry:` and so equally available to
    /// a loop's child. Neither occurs in this corpus — no corpus `LoopUntil`
    /// child declares a `timeout:` or a `retry:`, which
    /// `no_corpus_loop_until_declares_a_retry_or_inherits_one` already derives
    /// for one of the two — so **the ratchet cannot score either**, and these
    /// assertions are the whole instrument.
    #[test]
    fn the_five_terminals_split_the_way_is_terminated_no_fail_splits_them() {
        let src = loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 5\n",
            ONE_RETURN,
        );
        let plan = build_plan(&src).expect("it plans");

        // `isTerminatedInError`: the body did not execute, so the loop never
        // asks its condition, never pauses, and takes the children's aggregate.
        for stop in [RunState::Failed, RunState::Killed] {
            let (state, now, asked) = run_loop_until(&plan, stop, &["false"]);
            assert_eq!(
                wire(&state.runs[0].history),
                ["CREATED", "RUNNING", stop.wire()],
                "{stop:?} is `isTerminatedInError`, so one iteration and the \
                 children's aggregate — under `maxIterations: 5` and a condition \
                 that never becomes true, i.e. a loop that would otherwise run six \
                 times"
            );
            assert_eq!(
                asked, 0,
                "{stop:?}: and the condition is never EVALUATED, not merely \
                 ignored — a render that will not resolve fails a run"
            );
            assert_eq!(now, T0, "{stop:?}: nothing waited");
        }

        // `isTerminatedNoFail`: the body executed, so the loop asks its
        // condition and iterates to its limit. `SKIPPED` is
        // `a_skipped_child_still_counts_as_executed_and_the_loop_iterates`,
        // which needs a `runIf:` and so has a fixture of its own.
        // **`Warning` is the sixth row, added by W8 Task 5**, which is what
        // `children_executed`'s doc asks for by name: *"Re-run
        // `the_five_terminals_split_...` with the new variant as a sixth row --
        // a fixture answers only the question it was built for, and this one
        // was built for five."* It is `isTerminatedNoFail`'s FIRST member on
        // the artifact, so a `WARNING` child must let the loop through.
        //
        // **It is the whole instrument for that arm.** Adding `RunState::Warning`
        // to the `!matches!` exclusion below survives the entire workspace
        // suite otherwise -- measured, mutation M11 -- because the one corpus
        // flow that reaches it, `valids/waitfor-child-task-warning`, is
        // baselined on an `OutputValues` child no handler exists for and so
        // mismatches whatever this predicate answers.
        for go in [RunState::Success, RunState::Retried, RunState::Warning] {
            let (state, now, asked) = run_loop_until(&plan, go, &["false"]);
            assert_eq!(
                wire(&state.runs[0].history),
                [
                    "CREATED",
                    "RUNNING",
                    "PAUSED",
                    "RUNNING",
                    "PAUSED",
                    "RUNNING",
                    "PAUSED",
                    "RUNNING",
                    "PAUSED",
                    "RUNNING",
                    "PAUSED",
                    "RUNNING",
                    // **The loop's OWN terminal is its children's aggregate**,
                    // so a `WARNING` body ends the loop `WARNING` where a
                    // `RETRIED` one ends it `SUCCESS`: `guessFinalState` ranks
                    // `WARNING` and has no arm for `RETRIED` at all. Writing
                    // `SUCCESS` on all three rows would have been a fixture
                    // that could not tell those two apart -- and it would have
                    // had to, since W8 Task 5 is the commit that gave
                    // `aggregate` a `Warning` rank.
                    if go == RunState::Warning {
                        "WARNING"
                    } else {
                        "SUCCESS"
                    }
                ],
                "{go:?} is `isTerminatedNoFail`, so the loop iterates to \
                 `iterationCount > maxIterations` — six iterations under a limit of \
                 five, five pauses"
            );
            assert_eq!(asked, 5, "{go:?}: once per iteration but the last");
            assert_eq!(now, Tick(5000), "{go:?}: five pauses of PT1S");
        }
    }

    /// **A `CANCELLED` child STOPS a `LoopUntil`**, and it is the row the old
    /// spelling of `children_executed` would have got wrong — W8 Task 6.
    ///
    /// `childTaskRunExecuted` filters on `isTerminatedNoFail()`, which is
    /// `WARNING || SUCCESS || RETRIED || SKIPPED || RESUBMITTED`
    /// (`State.java:292`) — **`CANCELLED` is not in it**, so a cancelled body
    /// did not execute and the loop takes its children's aggregate without ever
    /// asking its condition, exactly as `FAILED` and `KILLED` do in
    /// [`the_five_terminals_split_the_way_is_terminated_no_fail_splits_them`].
    ///
    /// **The old spelling was a COMPLEMENT and that is why this is a separate
    /// test rather than a sixth row.** `children_executed` read
    /// `s.is_terminal() && !matches!(s, Failed | Killed)`, which is
    /// `isTerminatedNoFail` only for the enum it was written against. W8 Task
    /// 5's `Warning` fell on the side that complement gave it by luck;
    /// `Cancelled` does not, and under the old spelling this loop would have
    /// iterated six times on a body Kestra treats as failed. The predicate is
    /// `RunState::is_terminated_no_fail` now — Kestra's positive list, written
    /// once — and this is the row that would notice it being widened back.
    ///
    /// Nothing in the corpus supplies a `CANCELLED` loop child, so these
    /// assertions are the whole instrument. The `SUCCESS` control beside them
    /// is what stops an implementation that stopped on everything passing.
    #[test]
    fn a_cancelled_child_does_not_count_as_executed() {
        let src = loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 5\n",
            ONE_RETURN,
        );
        let plan = build_plan(&src).expect("it plans");

        let (state, now, asked) = run_loop_until(&plan, RunState::Cancelled, &["false"]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "CANCELLED"],
            "a CANCELLED body is `isTerminatedInError`: one iteration, and the \
             loop takes the children's aggregate -- which ranks CANCELLED \
             (`guessFinalState`'s third arm)"
        );
        assert_eq!(
            asked, 0,
            "and the condition is never EVALUATED, not merely ignored"
        );
        assert_eq!(now, T0, "nothing waited");

        // The control, on the same fixture: a state that IS
        // `isTerminatedNoFail` iterates to the limit. Without it, a
        // `children_executed` that answered `false` for everything would pass
        // the three assertions above.
        let (state, now, asked) = run_loop_until(&plan, RunState::Success, &["false"]);
        assert_eq!(state.runs[0].history.last(), Some(&RunState::Success));
        assert_eq!((asked, now), (5, Tick(5000)));
    }

    /// **A `SKIPPED` child still counts as executed**, so a loop whose body was
    /// skipped by a falsy `runIf:` asks its condition and iterates.
    ///
    /// Kestra's `childTaskRunExecuted` filters on `isTerminatedNoFail()`, which
    /// is `WARNING || SUCCESS || RETRIED || SKIPPED || RESUBMITTED`
    /// (`State.java:292`) — **`SKIPPED` is in it**, and of that set `RunState`
    /// can hold `SUCCESS`, `RETRIED` and `SKIPPED`.
    ///
    /// **This is the discriminating row for a rule nothing else in this file
    /// could tell apart.** `a_failed_body_ends_a_loop_until_without_asking_the_
    /// condition` rules out the WIDER reading (`is_terminated()`, which would
    /// loop a failing body to its limit) and every other fixture here supplies
    /// a `SUCCESS` child — so the NARROWER reading, `matches!(s, Success)`,
    /// agreed with all of them. Mutating to it survived the whole suite until
    /// this test existed, and it is production-reachable rather than
    /// hypothetical: a falsy `runIf:` on a loop's child is the shape, and W3c
    /// already ships `runIf:`.
    #[test]
    fn a_skipped_child_still_counts_as_executed_and_the_loop_iterates() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 2\n",
            "      - id: body\n        type: io.kestra.plugin.core.debug.Return\n        \
             format: x\n        runIf: \"false\"\n",
        ))
        .expect("it plans");
        let (state, now, asked) = run_loop_until(&plan, RunState::Success, &["false"]);
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "SKIPPED"],
            "the body is skipped, with no attempt of its own — W3c's shape"
        );
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
            "and the loop still runs its three iterations: `SKIPPED` is \
             `isTerminatedNoFail`, so `childTaskRunExecuted` is satisfied"
        );
        assert_eq!(
            asked, 2,
            "the condition is asked on every iteration but the last"
        );
        assert_eq!(now, Tick(2000));
    }

    /// **A falsy condition pauses for `interval` and starts the body again**,
    /// and the second iteration's body is a NEW run rather than a second
    /// attempt on the old one.
    ///
    /// Both halves are measured on `valids/waitfor-nested`, whose `loop_1` ran
    /// three iterations and whose child `loop_1_return` appears **once** in the
    /// golden, reading `CREATED SUBMITTED RUNNING SUCCESS` with one attempt.
    ///
    /// The loop's own attempt is asserted too, and it is the other half of the
    /// same measurement: Kestra's `resetAttempts()` drops the attempt and the
    /// next `RUNNING` builds a new one, so the record holds exactly one however
    /// many iterations ran. `Command::RestartLoop` writes the run's history and
    /// not the attempt's, which reaches the same record by not moving.
    #[test]
    fn a_falsy_condition_pauses_for_the_interval_and_runs_the_body_again() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 2\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, now, asked) = run_loop_until(&plan, RunState::Success, &["false"]);

        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
            "maxIterations 2 with a falsy condition runs THREE iterations: \
             `reachedMaximums` is `iterationCount > maxIterations`"
        );
        assert_eq!(
            state.runs[0].attempts.len(),
            1,
            "one attempt, whatever the iteration count -- 
             `retryWaitFor` drops it and the golden holds one"
        );
        assert_eq!(
            wire(&state.runs[0].attempts[0].history),
            ["CREATED", "RUNNING", "SUCCESS"],
            "and the RUNNINGs of the later iterations are not on it"
        );
        assert_eq!(
            wire(&state.runs[1].history),
            ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
            "the body's record is the LAST iteration's, with no trace of the two before"
        );
        assert_eq!(
            state.runs[1].attempts.len(),
            1,
            "a re-created run, not a retried one: a retry would add an attempt"
        );
        assert_eq!(
            asked, 2,
            "once per iteration EXCEPT the last: the third iteration is the one \
             `iterationCount > maxIterations` is true for, and `nextExecutionDate` \
             evaluates the condition only when it is not"
        );
        assert_eq!(
            now,
            Tick(2000),
            "two pauses of PT1S each, measured from the round the iteration ended"
        );
        assert_eq!(
            wire(&state.history),
            ["CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
            "the execution alternates PAUSED/RUNNING and carries NO `RESTARTED`: \
             6 of the golden's 18 PAUSED executions are this shape and all six \
             are a LoopUntil's"
        );
    }

    /// **`iterationCount > maxIterations`, strictly** -- so a limit of `n` runs
    /// `n + 1` iterations when the condition never becomes true.
    ///
    /// `valids/waitfor-max-iterations` is the row that decides it: limit 3, a
    /// condition that never holds, and a golden run of
    /// `CREATED RUNNING PAUSED RUNNING PAUSED RUNNING PAUSED RUNNING FAILED` --
    /// three pauses, four `RUNNING`s. It is the ONLY one of the corpus's five
    /// `maxIterations:` flows that can tell the two readings apart: the other
    /// four stop for a reason that is not the limit (three on their condition,
    /// one on a failed child), so a `>=` reading agrees with all four of them.
    /// `reached_maximums`' own doc carries the table.
    ///
    /// **`failOnMaxReached` decides the TERMINAL and never the count**, which
    /// is why both values are driven over the same limit here. It is read after
    /// the loop has already stopped.
    #[test]
    fn a_limit_of_n_runs_n_plus_one_iterations_and_fail_on_max_decides_only_the_terminal() {
        for (fail_on_max, terminal) in [(false, "SUCCESS"), (true, "FAILED")] {
            let plan = build_plan(&loop_until_flow(
                &format!(
                    "    condition: \"{{{{ done }}}}\"\n    failOnMaxReached: {fail_on_max}\n    \
                     checkFrequency:\n      interval: PT1S\n      maxIterations: 3\n"
                ),
                ONE_RETURN,
            ))
            .expect("it plans");
            let (state, _, asked) = run_loop_until(&plan, RunState::Success, &["false"]);
            assert_eq!(
                wire(&state.runs[0].history),
                [
                    "CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "PAUSED",
                    "RUNNING", terminal
                ],
                "failOnMaxReached: {fail_on_max} -- four iterations under a limit of three, \
                 and the flag changes only the last state"
            );
            assert_eq!(
                asked, 3,
                "the FOURTH iteration never asks the condition: `nextExecutionDate` \
                 evaluates it only `if (!this.reachedMaximums(...))`, so a loop at \
                 its limit renders nothing"
            );
        }
    }

    /// **`checkFrequency.maxDuration` is measured from the loop run's creation,
    /// not from the iteration**, and the bound is checked when the iteration
    /// ENDS.
    ///
    /// Kestra reads the last `isCreated()` entry of the taskrun's own history,
    /// which `resetAttempts()` never appends to -- so however many iterations
    /// have gone by, the instant is the original one. Here that instant is the
    /// attempt's, because a `LoopUntil` opens exactly one attempt and
    /// `Command::RestartLoop` does not reopen it.
    ///
    /// **The comparison is STRICT and this fixture is built to land exactly on
    /// it.** Kestra's is `creationDate.plus(maxDuration).isBefore(now)`, so a
    /// loop whose iteration ends AT `created + PT3S` has not reached the bound;
    /// with `PT1S` between iterations and a clock that jumps to the deadline
    /// and no further, iteration 4 ends at exactly T+3000 and iteration 5 at
    /// T+4000 is the first one past. A `<=` reading stops at four, which is why
    /// the fixture is arranged to land on the boundary rather than near it.
    ///
    /// `oracle-exec/driver.rs::pass_time` lands one millisecond PAST each
    /// deadline, so the real driver reaches the bound one iteration earlier --
    /// which is what `valids/loop-until-restart` shows, at four. **That entry
    /// is baselined at four against a golden of three**, and the gap is the
    /// artifact's delay poller: its two `PT1S` pauses took 1.17 s and 1.92 s on
    /// the captured round, so its third iteration ended at T+3.33 s. A rule
    /// that looked one interval AHEAD would match that golden and would not be
    /// `reachedMaximums`; this test is what pins which of the two egret
    /// implements.
    #[test]
    fn a_max_duration_is_measured_from_the_loops_creation_and_checked_when_an_iteration_ends() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    failOnMaxReached: true\n    checkFrequency:\n      \
             interval: PT1S\n      maxDuration: PT3S\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, now, _) = run_loop_until(&plan, RunState::Success, &["false"]);
        assert_eq!(
            wire(&state.runs[0].history),
            [
                "CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "PAUSED",
                "RUNNING", "PAUSED", "RUNNING", "FAILED"
            ],
            "FIVE iterations on a clock that lands exactly on each deadline: the \
             bound is `created + PT3S < now`, strictly, so the iteration that ends \
             AT T+3000 has not reached it and the one at T+4000 is the first that \
             has. A `<=` reading stops one earlier"
        );
        assert_eq!(now, Tick(4000), "four pauses of PT1S");

        // **And FOUR on the driver's clock, which is the number
        // `valids/loop-until-restart` is baselined at.** Same bounds, same
        // condition, one millisecond of slack per pause -- `pass_time` binds
        // `shortfall + 1`. The slack accumulates against a bound the interval
        // does not divide evenly, so `created + 3000 < now` is first true at
        // T+3003 instead of T+4000.
        //
        // **This assertion exists because two of this task's own artifacts
        // disagreed about that number and nothing in the repo adjudicated
        // it**: the ratchet prints traces and not counts, so "four" and "five"
        // could both be written down and neither be checked. They are both
        // this implementation, on two clocks, and now one test says so.
        let (driver, now, _) = run_loop_until_with_slack(&plan, RunState::Success, &["false"], 1);
        assert_eq!(
            wire(&driver.runs[0].history),
            [
                "CREATED", "RUNNING", "PAUSED", "RUNNING", "PAUSED", "RUNNING", "PAUSED",
                "RUNNING", "FAILED"
            ],
            "FOUR iterations on `pass_time`'s clock -- the trace the ratchet compares \
             against `valids/loop-until-restart`'s golden of three"
        );
        assert_eq!(
            now,
            Tick(3003),
            "three pauses of PT1S, each one millisecond long. **This pins the \
             driver's synthetic constant, not the mechanism**, and the caveat is \
             worth more than the number: the real `pass_time` lands at LEAST a \
             millisecond late, because its `now` is wall-derived and the DB work \
             between two settles takes real time. `slack: 1` is the floor of \
             that, which is the version of the clock a pure fixture can \
             reproduce"
        );
        // **And the bound on how much any of it matters, measured rather than
        // assumed** (W8 Task 4 fix round 2, carried forward from the review):
        // dropping `pass_time`'s `+ 1` entirely leaves the ratchet at
        // **118 / 118 / 0 broken**. *No golden entry's verdict depends on the
        // millisecond.* The shapes at risk were enumerated rather than sampled
        // -- `valids/loop-until-restart` and `valids/waitfor-child-task-warning`
        // (a `maxDuration` the interval does not divide evenly) and the banked
        // `valids/retry-new-execution-{flow,task}-duration` (a `RetryPolicy`
        // `maxDuration` against an accumulating origin) -- and not one flips.
        // So the four-versus-five split this test records is a property of the
        // DRIVER's arithmetic and not of anything the oracle scores.
    }

    /// **A loop with no maximum and a falsy condition never stops**, which is
    /// the over-acceptance this construct's `None` arms exist to refuse.
    ///
    /// The mirror of `an_unbounded_pause_never_resumes_itself`. A `LoopUntil`
    /// with neither `maxIterations:` nor `maxDuration:` is exactly Kestra's
    /// "unlimited" (`CheckFrequency`'s own javadoc says so for both fields),
    /// and `valids/waitfor-nested` is written that way -- all three of its
    /// loops stop on the condition alone. An implementation that invented a
    /// default limit would bank the same entries and ship a different product,
    /// and nothing in the ratchet could see it.
    ///
    /// Driven by rounds rather than to a fixed point, because a fixed point is
    /// what this must NOT have.
    #[test]
    fn an_unbounded_loop_until_with_a_falsy_condition_never_stops() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      interval: PT1S\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let mut state = ExecutionState::new(&plan);
        let mut now = T0;
        for _ in 0..200 {
            if !round_at(&plan, &mut state, now).is_empty() {
                continue;
            }
            if state.runs[0].rendered.is_none()
                && crate::executor::loop_condition_is_asked(&plan, &state, 0, 0, now)
            {
                state.runs[0].rendered = Some("false".to_string());
                continue;
            }
            if let Some(ix) = (1..state.runs.len())
                .find(|&ix| state.runs[ix].history.last() == Some(&RunState::Running))
            {
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: RunState::Success,
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
                Some(next) => now = next,
                None => panic!(
                    "an unbounded LoopUntil settled: {:?}. Nothing this executor \
                     holds may end one -- a default limit would be a terminal the \
                     artifact does not produce",
                    wire(&state.runs[0].history)
                ),
            }
        }
        assert!(
            !state.runs[0]
                .history
                .last()
                .is_some_and(|s| s.is_terminal()),
            "after 200 rounds the loop must still be going: {:?}",
            wire(&state.runs[0].history)
        );
        assert!(
            now >= Tick(10_000),
            "and the clock must have moved by one interval per iteration: {now:?}"
        );
    }

    /// **A body that FAILED ends the loop without asking the condition** --
    /// Kestra's `childTaskRunExecuted`, whose filter is `isTerminatedNoFail()`
    /// and not `isTerminated()`.
    ///
    /// `valids/waitfor-multiple-tasks-failed` is the corpus row, and it is the
    /// only one: its second child is a `Fail`, and its golden is ONE iteration
    /// ending `FAILED` with no `PAUSED` at all -- under `maxIterations: 5` and
    /// a condition that never becomes true, i.e. a loop that would otherwise
    /// have run six times. It is also one of the six entries the `OutputValues`
    /// handler holds, so the ratchet cannot score this line and this test is
    /// the whole instrument.
    /// An implementation that read "all children terminated" would loop a
    /// failing body to its limit.
    ///
    /// `asked` is 0, which is the sharper half: the condition is not merely
    /// ignored, it is never evaluated -- and a render that will not resolve
    /// FAILS a run, so evaluating it would be a terminal the artifact does not
    /// produce rather than a wasted call.
    #[test]
    fn a_failed_body_ends_a_loop_until_without_asking_the_condition() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 5\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, now, asked) = run_loop_until(&plan, RunState::Failed, &["false"]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "FAILED"],
            "one iteration, no pause, and the children's aggregate as the terminal"
        );
        assert_eq!(
            asked, 0,
            "and the condition was never evaluated at all: `resolveState` asks \
             `childTaskRunExecuted` first"
        );
        assert_eq!(now, T0, "nothing waited");
    }

    /// **The condition is re-rendered every iteration, and the answer that
    /// decides iteration `k` is the one taken in iteration `k`.**
    ///
    /// A two-sided pair on the truthiness operator says nothing about where the
    /// OPERAND came from: an implementation that evaluated the condition once
    /// and cached it passes `a_truthy_condition_stops_a_loop_until_where_it_
    /// stands` and `a_falsy_condition_pauses_...` both, because in each of
    /// those every answer is the same string. This is the fixture where they
    /// differ -- `false`, then `true` -- and it is Kestra's own
    /// `runContext.render(this.condition).skipCache()`.
    ///
    /// Asserted from BOTH sides, so neither a stuck-falsy nor a stuck-truthy
    /// cache passes: the loop must run exactly two iterations, which is neither
    /// one (cached truthy) nor its `maxIterations` limit of four (cached
    /// falsy).
    #[test]
    fn the_condition_that_decides_an_iteration_is_that_iterations_render() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 4\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let (state, _, asked) = run_loop_until(&plan, RunState::Success, &["false", "true"]);
        assert_eq!(
            wire(&state.runs[0].history),
            ["CREATED", "RUNNING", "PAUSED", "RUNNING", "SUCCESS"],
            "exactly two iterations: one for the falsy answer, one for the truthy \
             one. A cached truthy answer gives one, a cached falsy answer gives five"
        );
        assert_eq!(asked, 2);
    }

    /// **`iterationCount` is 1-based, and it RESETS when an outer loop restarts
    /// the inner one.**
    ///
    /// `valids/waitfor-nested` is the row that decides both halves at once. Its
    /// three loops each exit on `{{ outputs.<child>.value | number >= 3 }}`
    /// where the child returned the loop's own `iterationCount`, and each runs
    /// exactly three iterations -- so the child saw 1, 2, 3 and not 0, 1, 2.
    /// And its execution carries **26** `PAUSED` states, which is
    /// `2 + 3x2 + 9x2`: `loop_3` is started nine times and pauses twice on
    /// every one of them, which it can only do if its count starts again at 1
    /// each time.
    ///
    /// The derivation is over the run's history, which is what makes the reset
    /// free: `Command::RestartLoop` empties a descendant's history, so an inner
    /// loop that is restarted counts from zero without anything being cleared
    /// on purpose.
    #[test]
    fn the_iteration_count_is_one_based_and_a_restarted_loop_counts_from_one_again() {
        let plan = build_plan(&loop_until_flow(
            "    condition: \"{{ done }}\"\n    checkFrequency:\n      \
             interval: PT1S\n      maxIterations: 2\n",
            ONE_RETURN,
        ))
        .expect("it plans");
        let mut state = ExecutionState::new(&plan);
        let mut now = T0;
        let mut seen = Vec::new();
        for _ in 0..1000 {
            if !round_at(&plan, &mut state, now).is_empty() {
                continue;
            }
            if state.runs[0].rendered.is_none()
                && crate::executor::loop_condition_is_asked(&plan, &state, 0, 0, now)
            {
                // Read at the moment the condition is asked, which is the
                // moment the body of that iteration has finished -- the same
                // moment `worker::render` builds `outputs.<loop>` from it.
                seen.push(crate::executor::loop_iteration(&state, 0));
                state.runs[0].rendered = Some("false".to_string());
                continue;
            }
            if let Some(ix) = (1..state.runs.len())
                .find(|&ix| state.runs[ix].history.last() == Some(&RunState::Running))
            {
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: RunState::Success,
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
                Some(next) => now = next,
                None => break,
            }
        }
        assert_eq!(
            seen,
            [1, 2],
            "the count the children of iteration k see is k, 1-based. A 0-based \
             counter makes `valids/waitfor-nested` exit one iteration late, or \
             never"
        );

        // And the reset, on a run whose history `RestartLoop` emptied: after
        // `Command::RecreateRun` gives it its `CREATED` back it has no `RUNNING`
        // at all, so its own count starts again at 1.
        let mut nested = ExecutionState::new(&plan);
        nested.runs.push(crate::executor::TaskRun {
            task: 0,
            parent: None,
            value: None,
            rendered: Some("cached".into()),
            rendered_run_if: None,
            history: vec![
                RunState::Created,
                RunState::Running,
                RunState::Paused,
                RunState::Running,
            ],
            attempts: Vec::new(),
            not_before: Some(Tick(5)),
            resume_with: None,
            working_dir: None,
        });
        nested.runs.push(crate::executor::TaskRun {
            task: 1,
            parent: Some(0),
            value: None,
            rendered: None,
            rendered_run_if: Some("true".into()),
            history: vec![RunState::Created, RunState::Success],
            attempts: Vec::new(),
            not_before: None,
            resume_with: None,
            working_dir: None,
        });
        assert_eq!(crate::executor::loop_iteration(&nested, 0), 2);
        apply(&mut nested, &Command::RestartLoop { run: 0 });
        assert_eq!(
            crate::executor::loop_iteration(&nested, 0),
            3,
            "the loop's OWN count goes up: its history keeps every RUNNING"
        );
        assert_eq!(
            nested.runs[0].rendered, None,
            "and its cached condition is gone"
        );
        assert!(
            nested.runs[1].history.is_empty()
                && nested.runs[1].rendered_run_if.is_none()
                && nested.runs[1].attempts.is_empty(),
            "while the DESCENDANT is thrown away entirely -- an empty history is \
             this executor's spelling of Kestra's `removeIf`"
        );
        apply(&mut nested, &Command::RecreateRun { run: 1 });
        assert_eq!(
            wire(&nested.runs[1].history),
            ["CREATED"],
            "and the walk gives it back one `CREATED` when it reaches it"
        );
    }

    /// **The one expression in this corpus that Pebble refuses and minijinja
    /// answers**, and the whole of why `valids/loopuntil-failed-flowable` is
    /// still baselined.
    ///
    /// That entry's runs, attempts and ordered pairs are reproduced exactly and
    /// it differs on ONE field: its `LoopUntil` ends `SUCCESS` where the golden
    /// holds `FAILED`. The condition is
    /// `{{ outputs.loop.iterationCount < "5" }}` -- a relational operator over
    /// a number and a string. Pebble's `OperatorUtils` widens both operands to
    /// `Number` and throws on a `String`, the exception comes out of
    /// `runContext.render(this.condition)` inside `nextExecutionDate`, and the
    /// task fails; the artifact's own trace agrees, failing the `loop` taskrun
    /// 11 ms after its only child SUCCEEDED with no `PAUSED` in between, which
    /// is the only outcome `LoopUntil.resolveState` has no other path to.
    /// minijinja orders values across kinds instead and answers a truthy
    /// string, so the loop stops on its first iteration and succeeds.
    ///
    /// **Two halves, and the first is what keeps the second from being a
    /// sentence about one flow.** The scan derives, from all 351 corpus flows,
    /// that this shape occurs exactly once -- so the divergence is bounded, and
    /// a flow added later that writes another one fails here rather than
    /// quietly joining the baseline.
    ///
    /// **This test asserts egret's DIVERGENT answer on purpose**, which is the
    /// one thing a test may not normally do. It is here because the answer is
    /// load-bearing for a baseline line: if egret ever gains Pebble-strict
    /// relational operators, this fails, and the reader is sent to
    /// `oracle-exec/baseline.txt`'s entry for that id -- which will by then be
    /// bankable. A comment claiming the same thing would not be.
    #[test]
    fn the_one_corpus_comparison_pebble_refuses_and_minijinja_answers() {
        /// Every `{{ … }}` region in `src` holding a relational operator whose
        /// right-hand side opens with a quote.
        ///
        /// Hand-written rather than a regex because the corpus quotes the
        /// operand through YAML escaping (`< \"5\"`), and a scanner that has to
        /// be told about the backslash in one place is easier to read than one
        /// that hides it in a character class.
        fn quoted_comparisons(src: &str) -> usize {
            let mut found = 0;
            let bytes = src.as_bytes();
            let mut i = 0;
            while let Some(open) = src[i..].find("{{") {
                let start = i + open + 2;
                let Some(len) = src[start..].find("}}") else {
                    break;
                };
                let region = &src[start..start + len];
                let mut j = 0;
                let rb = region.as_bytes();
                while j < rb.len() {
                    if rb[j] == b'<' || rb[j] == b'>' {
                        let mut k = j + 1;
                        if k < rb.len() && rb[k] == b'=' {
                            k += 1;
                        }
                        while k < rb.len() && rb[k] == b' ' {
                            k += 1;
                        }
                        if k < rb.len() && rb[k] == b'\\' {
                            k += 1;
                        }
                        if k < rb.len() && (rb[k] == b'"' || rb[k] == b'\'') {
                            found += 1;
                        }
                    }
                    j += 1;
                }
                i = start + len + 2;
            }
            let _ = bytes;
            found
        }

        let mut writers = Vec::new();
        for (id, src) in every_corpus_flow() {
            if quoted_comparisons(&src) > 0 {
                writers.push(id);
            }
        }
        assert_eq!(
            writers,
            ["valids/loopuntil-failed-flowable"],
            "the corpus's relational-operator-against-a-quoted-operand population. \
             A second member is a second entry this renderer divergence decides, \
             and it must be reasoned about rather than discovered in a baseline"
        );

        // The control first: the same operator with a BARE number is what every
        // other corpus comparison writes, and it is not this class.
        // `valids/retry-with-flowable-errors` writes
        // `{{ kv(...) < 2 ? ko : 'It works' }}` -- a quote appears in the
        // region and is not the operand.
        assert_eq!(
            quoted_comparisons("{{ a < 2 ? ko : 'It works' }}"),
            0,
            "a quote that is not the operand must not count, or the scan blames a \
             flow whose comparison Pebble is perfectly happy with"
        );
        assert_eq!(quoted_comparisons("{{ a < \\\"5\\\" }}"), 1);

        let mut cx = crate::runners::RenderContext::new();
        cx.insert(
            "outputs",
            serde_json::json!({"loop": {"iterationCount": 1}}),
        );
        let rendered = crate::render_str("{{ outputs.loop.iterationCount < \"5\" }}", &cx).expect(
            "egret's renderer ANSWERS this, and the artifact throws -- if this \
             becomes an error, `valids/loopuntil-failed-flowable` is bankable and \
             `oracle-exec/baseline.txt` says so",
        );
        assert!(
            crate::executor::is_truthy(&rendered),
            "and the answer is truthy, which is what makes the loop SUCCEED on its \
             first iteration where the artifact FAILED the task: {rendered:?}"
        );
    }

    /// **Every `LoopUntil` bound in the corpus is a literal**, which is what
    /// makes `build_plan` reading them without a `RenderContext` a measured
    /// non-issue rather than an assumption -- and what makes
    /// [`plan::loop_until_gap`](crate::executor::plan) an arm nothing in this
    /// corpus takes.
    ///
    /// All four are checked, not just the two the comparable entries use: a
    /// derivation that swept only the fields today's flows happen to write is
    /// the shape this package keeps paying for.
    #[test]
    fn every_corpus_loop_until_bound_is_a_literal() {
        let mut dynamic = Vec::new();
        let mut seen = 0usize;
        for (id, src) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            for (task_id, lu) in corpus_loop_untils(&flow) {
                seen += 1;
                if let Some(field) = crate::executor::plan::loop_until_gap(lu) {
                    dynamic.push(format!("{id}:{task_id} = {field}"));
                }
            }
        }
        assert!(
            seen >= 12,
            "the walk found {seen} LoopUntil/WaitFor tasks, which is fewer than the \
             12 the ten corpus flows of this construct declare between them -- it \
             has stopped reaching them"
        );
        assert!(
            dynamic.is_empty(),
            "a corpus LoopUntil writes a bound `build_plan` cannot read, so it is \
             planned OPAQUE and the arm that does it is reachable after all: {dynamic:?}"
        );
    }

    /// **No corpus `LoopUntil` can be retried**, which is what bounds
    /// [`loop_iteration`](crate::executor::loop_iteration)'s derivation.
    ///
    /// Counting `RUNNING`s is `iterationCount` only while a run enters
    /// `RUNNING` once per iteration and for no other reason. A retry re-enters
    /// `CREATED` and then `RUNNING` again, and the count would carry the
    /// previous attempt's iterations into the new one. Nothing can reach it --
    /// `retry::scan` retries a `Runnable` or an `Executable` and a `LoopUntil`
    /// is neither -- and this says so from the corpus's side as well, over both
    /// ways a task can acquire a policy.
    #[test]
    fn no_corpus_loop_until_declares_a_retry_or_inherits_one() {
        let mut with_retry = Vec::new();
        for (id, src) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            let flow_level = flow.retry.is_some();
            for (task_id, _) in corpus_loop_untils(&flow) {
                if flow_level {
                    with_retry.push(format!("{id}:{task_id} (flow-level retry:)"));
                }
            }
            fn walk(
                task: &crate::models::Task,
                under_retry: bool,
                id: &str,
                out: &mut Vec<String>,
            ) {
                let under = under_retry || task.retry.is_some();
                if under && matches!(task.kind, crate::models::TaskKind::LoopUntil(_)) {
                    out.push(format!("{id}:{} (task-level retry:)", task.id));
                }
                for child in task.children().into_iter().chain(task.error_children()) {
                    walk(child, under, id, out);
                }
            }
            for task in flow.tasks.iter().chain(flow.errors.iter()) {
                walk(task, false, &id, &mut with_retry);
            }
        }
        assert!(
            with_retry.is_empty(),
            "a corpus LoopUntil is inside a retry policy, so `loop_iteration`'s \
             RUNNING count can carry a previous attempt's iterations: {with_retry:?}"
        );
    }

    /// Every `LoopUntil`/`WaitFor` in a flow, at any depth, with its task id.
    fn corpus_loop_untils(flow: &crate::models::Flow) -> Vec<(String, &crate::models::LoopUntil)> {
        fn walk<'a>(
            task: &'a crate::models::Task,
            out: &mut Vec<(String, &'a crate::models::LoopUntil)>,
        ) {
            if let crate::models::TaskKind::LoopUntil(lu) = &task.kind {
                out.push((task.id.clone(), lu));
            }
            for child in task.children().into_iter().chain(task.error_children()) {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        for task in flow
            .tasks
            .iter()
            .chain(flow.errors.iter())
            .chain(flow.finally_.iter())
            .chain(flow.after_execution.iter())
        {
            walk(task, &mut out);
        }
        out
    }

    /// Every `pauseDuration:` and `timeout:` written on a corpus `Pause`, and
    /// the two facts `flowable::pause` relies on: **no `Pause` writes both**,
    /// and **every one that parses at all parses as ISO-8601**.
    ///
    /// The precedence in `pause::deadline` is a choice with no measurement
    /// behind it, and this is what says so out loud rather than leaving the
    /// next reader to assume it was measured. If a corpus flow ever writes
    /// both, this fails and whoever added it has to measure the artifact.
    #[test]
    fn every_corpus_pause_bound_is_one_or_none() {
        let mut both = Vec::new();
        let mut seen = 0usize;
        for (id, src) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            for (task_id, pause, timeout) in corpus_pauses(&flow) {
                seen += 1;
                if pause.pause_duration.is_some() && timeout.is_some() {
                    both.push(format!("{id}:{task_id}"));
                }
            }
        }
        assert!(
            seen >= 20,
            "the walk found {seen} pauses, which is fewer than the 20 corpus flows \
             known to declare one — it has stopped reaching them"
        );
        assert!(
            both.is_empty(),
            "a corpus Pause writes BOTH bounds, so `pause::deadline`'s unmeasured \
             precedence now decides a comparable entry: {both:?}"
        );
    }

    /// **Every `timeout:` on a corpus `Pause` is an ISO-8601 literal**, which
    /// is what makes `build_plan` reading it without a `RenderContext` a
    /// measured non-issue rather than an assumption.
    ///
    /// The same fact for `pauseDuration:` is NOT asserted and must not be:
    /// `valids/pause-duration-from-input` writes an expression into it, which
    /// is exactly why that one goes through the render slot and this one does
    /// not.
    #[test]
    fn every_corpus_pause_timeout_is_a_literal() {
        let mut dynamic = Vec::new();
        let mut literals = 0usize;
        for (id, src) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            for (task_id, _, timeout) in corpus_pauses(&flow) {
                let Some(t) = timeout else { continue };
                match retry::parse_duration(t.source()) {
                    Some(_) => literals += 1,
                    None => dynamic.push(format!("{id}:{task_id} = {:?}", t.source())),
                }
            }
        }
        assert!(
            dynamic.is_empty(),
            "a corpus Pause's `timeout:` is not a literal, so `build_plan` cannot \
             read it: {dynamic:?}"
        );
        assert_eq!(
            literals, 3,
            "the three baselined `timeout:`-on-a-Pause declarations — \
             pause-timeout, pause-timeout-allow-failure and \
             pause-errors-finally-after-execution. A zero here would satisfy the \
             assertion above while measuring nothing"
        );
    }

    /// **No comparable corpus flow declares two `Pause`es in one execution**,
    /// which is the population `pause::execution_state`'s suppression would be
    /// measured against if one existed.
    #[test]
    fn the_corpus_has_no_execution_with_two_pauses() {
        let mut many = Vec::new();
        for (id, src) in every_corpus_flow() {
            let Ok(flow) = crate::spec::parse(&src) else {
                continue;
            };
            let n = corpus_pauses(&flow).len();
            if n > 1 {
                many.push(format!("{id} ({n})"));
            }
        }
        assert!(
            many.is_empty(),
            "a corpus flow declares more than one Pause, so the repeat-suppression \
             in `pause::execution_state` is now measurable and must be measured \
             against the artifact rather than chosen: {many:?}"
        );
    }

    /// Every `Pause` in a flow, with the `timeout:` that sits on its `Task`.
    ///
    /// The walk is `Task::children()` plus `error_children()`, i.e. the same
    /// two lists `Plan::children_of` reads, so a `Pause` nested inside any
    /// flowable's `tasks:` or `errors:` is found. A walk over the flow's top
    /// level alone would miss six of the corpus's twenty.
    fn corpus_pauses(
        flow: &crate::models::Flow,
    ) -> Vec<(
        String,
        &crate::models::Pause,
        Option<&crate::models::Property<std::time::Duration>>,
    )> {
        fn walk<'a>(
            task: &'a crate::models::Task,
            out: &mut Vec<(
                String,
                &'a crate::models::Pause,
                Option<&'a crate::models::Property<std::time::Duration>>,
            )>,
        ) {
            if let crate::models::TaskKind::Pause(p) = &task.kind {
                out.push((task.id.clone(), p, task.timeout.as_ref()));
            }
            for child in task.children().into_iter().chain(task.error_children()) {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        for task in flow
            .tasks
            .iter()
            .chain(&flow.errors)
            .chain(&flow.finally_)
            .chain(&flow.after_execution)
        {
            walk(task, &mut out);
        }
        out
    }

    /// The corpus `Pause`es that can reach another flowable's `Paused` arm,
    /// derived **from the plan** — `parallel::drive_concurrent`'s new arm cites
    /// this as the reason no corpus flow drives it.
    ///
    /// Read off `Plan` rather than off the parsed flow, and the first draft of
    /// this test read the flow and was wrong twice over. A `Pause` is only
    /// reachable there if it has a `PlanTask` at all and that task is not a
    /// root, and the model walk answered `invalids/workingdirectory-invalid`'s
    /// `Pause` — a flow `build_plan` REFUSES, whose parent is planned opaque
    /// anyway, so neither the pause nor the refusal ever reaches an executor.
    /// A population that includes flows nothing can run is not this population.
    ///
    /// Every member must be UNBOUNDED. A bounded one would reach the arm and
    /// make the comment beside it false.
    #[test]
    fn the_corpus_pauses_that_sit_inside_another_flowable_are_named() {
        let mut nested: Vec<(String, bool)> = Vec::new();
        let mut planned_pauses = 0usize;
        for (id, src) in every_corpus_flow() {
            let Ok(plan) = build_plan(&src) else {
                continue;
            };
            for ix in 0..plan.tasks.len() {
                let TaskKind::Pause { duration } = &plan.tasks[ix].kind else {
                    continue;
                };
                planned_pauses += 1;
                if plan.roots.contains(&ix) {
                    continue;
                }
                nested.push((
                    format!("{id}:{}", plan.tasks[ix].id),
                    duration.is_some() || plan.tasks[ix].timeout.is_some(),
                ));
            }
        }
        assert!(
            planned_pauses >= 12,
            "only {planned_pauses} corpus `Pause`es reach a `TaskKind::Pause`, which \
             is fewer than the twelve baselined entries that declare one — the walk \
             has stopped reaching them and every assertion below is vacuous"
        );
        let names: Vec<&str> = nested.iter().map(|(n, _)| n.as_str()).collect();
        let bounded: Vec<&str> = nested
            .iter()
            .filter(|(_, b)| *b)
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(
            bounded.is_empty(),
            "a BOUNDED corpus Pause is now planned inside another flowable \
             ({bounded:?}), so `parallel::drive_concurrent`'s `Paused` arm is \
             reachable from the corpus after all and the comment beside it is \
             stale. The whole nested set is {names:?}"
        );

        // **The `LoopUntil` half, added at W8 Task 4 fix round 2, and the
        // reason is a method rather than a symmetry.** `parallel.rs`'s comment
        // made two claims: a `Pause` one this test already compared, and a
        // `LoopUntil` one that was PROSE. The prose was false for eleven days
        // in the form Task 4 first wrote it, corrected in fix round 1, and its
        // twin thirty lines up stayed false — because a text edit fixes the
        // sentence it is looking at and a program compares every instance.
        //
        // So the question to ask of a claim is *does it have a program-checked
        // twin?* This one did, and closing the gap is a test extension rather
        // than a sweep.
        //
        // **A `LoopUntil` ALWAYS pauses if it iterates at all** — there is no
        // unbounded/bounded split to filter on, the way there is for a `Pause`
        // — so the assertion is over containment alone.
        let mut inside: Vec<String> = Vec::new();
        let mut contains_one: Vec<String> = Vec::new();
        let mut planned_loops = 0usize;
        for (id, src) in every_corpus_flow() {
            let Ok(plan) = build_plan(&src) else {
                continue;
            };
            for ix in 0..plan.tasks.len() {
                if !matches!(plan.tasks[ix].kind, TaskKind::LoopUntil { .. }) {
                    continue;
                }
                planned_loops += 1;
                // Which flowable, if any, holds this one. A `LoopUntil` nested
                // in another `LoopUntil` is `valids/waitfor-nested` and is NOT
                // this arm's subject: that path is `loop_until::drive`'s own
                // `sequential::drive`, not `drive_concurrent`.
                let container = (0..plan.tasks.len()).find(|&p| {
                    matches!(
                        plan.tasks[p].kind,
                        TaskKind::Parallel { .. } | TaskKind::Dag { .. }
                    ) && plan.children_of(p).contains(&ix)
                });
                if container.is_some() {
                    inside.push(format!("{id}:{}", plan.tasks[ix].id));
                }
                if plan.children_of(ix).iter().any(|&c| {
                    matches!(
                        plan.tasks[c].kind,
                        TaskKind::Parallel { .. } | TaskKind::Dag { .. }
                    )
                }) {
                    contains_one.push(format!("{id}:{}", plan.tasks[ix].id));
                }
            }
        }
        // **5, and it was 12 until no-silent-stall Task 1.** Seven corpus
        // `WaitFor` flows hold an `io.kestra.plugin.core.output.OutputValues`
        // child, an unregistered type and so the error `unknown_task_type`
        // now: `build_plan` refuses them, and a refused flow cannot put a
        // `LoopUntil` in front of `drive_concurrent` either. What is left is
        // still not zero, which is what keeps the assertions below non-vacuous.
        assert!(
            planned_loops >= 5,
            "only {planned_loops} corpus `LoopUntil`s reach a `TaskKind::LoopUntil`, \
             which is fewer than the five the corpus flows `build_plan` still plans \
             declare between them — the walk has stopped reaching them and every \
             assertion below is vacuous"
        );
        assert!(
            inside.is_empty(),
            "a corpus `LoopUntil` is now planned inside a `Parallel` or a `Dag` \
             ({inside:?}), so `parallel::drive_concurrent`'s `Paused` arm is \
             reachable from the corpus after all and BOTH comments beside it are \
             stale — the one at its own line and the one thirty lines above it"
        );
        // The other direction, asserted rather than implied, because it is the
        // half the first version of that comment got backwards: **the
        // containment runs the other way and there is exactly one instance.**
        assert_eq!(
            contains_one,
            ["valids/loopuntil-failed-flowable:loop"],
            "the corpus `LoopUntil`s that CONTAIN a `Parallel` or a `Dag`. A
             comment saying no corpus `LoopUntil` contains one was false when it
             was written, and this is what compares it"
        );
    }
}

#[cfg(test)]
mod allow_failure_drive_tests {
    use super::state::{apply, ExecutionState, RunState, Tick};
    use super::{advance, plan, Command, Plan, TaskKind};

    const T0: Tick = Tick(0);

    /// Runs the flow to its end. Each in-flight `Runnable` finishes `FAILED` if
    /// its id is in `fail`, and `SUCCESS` otherwise. Returns the execution's
    /// final state as `advance` reports it, with the run states.
    fn run(src: &str, fail: &[&str]) -> (Plan, ExecutionState, RunState) {
        let flow = crate::spec::parse(src).expect("parses");
        let plan = plan::flatten_flow(&flow).expect("plans");
        let mut state = ExecutionState::new(&plan);
        let mut final_state = None;
        for _ in 0..1000 {
            let cmds = advance(&plan, &state, T0);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    if let Command::SetExecutionState { state } = cmd {
                        final_state = Some(*state);
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
            let Some(ix) = in_flight else { break };
            let id = plan.tasks[state.runs[ix].task].id.as_str();
            // The worker decides a runnable's terminal (`worker::claim`), and
            // that is where `allowFailure:` softens a `FAILED`.
            let raw = if fail.contains(&id) {
                RunState::Failed
            } else {
                RunState::Success
            };
            let outcome = super::allow_failure::for_runnable(&plan, &state, ix, raw, T0);
            apply(
                &mut state,
                &Command::SetRunState {
                    run: ix,
                    state: outcome,
                },
            );
        }
        let final_state = final_state.expect("the execution reached a terminal state");
        (plan, state, final_state)
    }

    /// The last state of the run for `id`, or `None` if no run was created.
    fn last_of(plan: &Plan, state: &ExecutionState, id: &str) -> Option<RunState> {
        let task = plan.tasks.iter().position(|t| t.id == id)?;
        state
            .runs
            .iter()
            .find(|r| r.task == task)
            .and_then(|r| r.history.last().copied())
    }

    const BLOCK: &str = "id: af\nnamespace: io.kestra.tests\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.AllowFailure\n{ALLOW}    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: ko\n        type: io.kestra.plugin.core.execution.Fail\n{KO_FLAG}      - id: skipped\n        type: io.kestra.plugin.core.debug.Return\n        format: s\n    errors:\n      - id: handler\n        type: io.kestra.plugin.core.debug.Return\n        format: h\n    finally:\n      - id: fin\n        type: io.kestra.plugin.core.debug.Return\n        format: f\n  - id: after\n    type: io.kestra.plugin.core.debug.Return\n    format: after\n";

    fn block(allow_warning: bool, child_allow_failure: bool) -> String {
        BLOCK
            .replace(
                "{ALLOW}",
                if allow_warning {
                    "    allowWarning: true\n"
                } else {
                    ""
                },
            )
            .replace(
                "{KO_FLAG}",
                if child_allow_failure {
                    "        allowFailure: true\n"
                } else {
                    ""
                },
            )
    }

    /// A failed child skips its siblings, runs `errors:` and `finally:`, makes
    /// the block `WARNING`, and lets the task after the block run. Measured on
    /// the image: `valids/allow-failure` and `valids/finally-allowfailure`.
    #[test]
    fn a_failed_child_makes_the_block_a_warning_and_the_flow_goes_on() {
        let (plan, state, flow) = run(&block(false, false), &["ko"]);
        assert_eq!(last_of(&plan, &state, "block"), Some(RunState::Warning));
        assert_eq!(last_of(&plan, &state, "ko"), Some(RunState::Failed));
        assert_eq!(
            last_of(&plan, &state, "skipped"),
            None,
            "a sibling after the failed child is not run"
        );
        assert_eq!(last_of(&plan, &state, "handler"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "fin"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "after"), Some(RunState::Success));
        assert_eq!(flow, RunState::Warning);
    }

    /// The block's `allowWarning:` maps a child that already WARNED to SUCCESS.
    /// Measured: `probe-combo-warn` (SUCCESS) and `probe-combo-nowarn` (WARNING).
    #[test]
    fn a_warned_child_is_a_success_only_under_the_blocks_allow_warning() {
        // `ko` is a `Fail`, so it fails; its own `allowFailure:` makes it WARN.
        let (plan, state, _) = run(&block(true, true), &["ko"]);
        assert_eq!(last_of(&plan, &state, "ko"), Some(RunState::Warning));
        assert_eq!(last_of(&plan, &state, "block"), Some(RunState::Success));

        let (plan, state, _) = run(&block(false, true), &["ko"]);
        assert_eq!(last_of(&plan, &state, "block"), Some(RunState::Warning));
    }

    /// `finally:` runs when the block succeeded too, and `errors:` do not.
    #[test]
    fn finally_runs_after_a_successful_block_and_errors_do_not() {
        let (plan, state, flow) = run(&block(false, false), &[]);
        assert_eq!(last_of(&plan, &state, "block"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "fin"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "handler"), None);
        assert_eq!(flow, RunState::Success);
    }
}

/// End hooks A4 (spec §4.1, D2): every flowable and the flow itself choose
/// their list by Kestra's `findTaskDependingFlowState` -- `tasks`, then
/// `errors` (on failure only), then `finally`, one at a time -- and take their
/// state from `tasks` alone.
///
/// The harness is `allow_failure_drive_tests`'s `run`, plus what these shapes
/// need: a literal `If` condition and `runIf:` are rendered as the caller
/// would, time jumps to the earliest `not_before` when nothing else moves (a
/// `retry:`), an optional kill once a named run is `RUNNING`, and an event log
/// (`create:<id>`, `end:<id>:<state>`, `exec:<state>`) in the order things
/// happened.
#[cfg(test)]
mod end_hooks_drive_tests {
    use super::state::{apply, ExecutionState, RunState, Tick};
    use super::{advance, plan, Command, Plan, TaskKind};

    struct Trace {
        plan: Plan,
        state: ExecutionState,
        flow: RunState,
        events: Vec<String>,
    }

    fn id_of<'a>(plan: &'a Plan, state: &ExecutionState, ix: usize) -> &'a str {
        plan.tasks[state.runs[ix].task].id.as_str()
    }

    /// `apply`, recording every run created and every run or execution that
    /// enters a terminal state.
    fn step(plan: &Plan, state: &mut ExecutionState, cmd: &Command, events: &mut Vec<String>) {
        let runs_before = state.runs.len();
        let lasts: Vec<Option<RunState>> = state
            .runs
            .iter()
            .map(|r| r.history.last().copied())
            .collect();
        let exec_before = state.history.last().copied();
        apply(state, cmd);
        for ix in runs_before..state.runs.len() {
            events.push(format!("create:{}", id_of(plan, state, ix)));
        }
        for (ix, before) in lasts.iter().enumerate() {
            let now = state.runs[ix].history.last().copied();
            if now != *before {
                if let Some(s) = now.filter(|s| s.is_terminal()) {
                    events.push(format!("end:{}:{s:?}", id_of(plan, state, ix)));
                }
            }
        }
        let exec_now = state.history.last().copied();
        if exec_now != exec_before {
            if let Some(s) = exec_now.filter(|s| s.is_terminal()) {
                events.push(format!("exec:{s:?}"));
            }
        }
    }

    fn drive(src: &str, fail: &[&str], kill_when_running: Option<&str>) -> Trace {
        let flow = crate::spec::parse(src).expect("parses");
        let plan = plan::flatten_flow(&flow).expect("plans");
        let mut state = ExecutionState::new(&plan);
        let mut events = Vec::new();
        let mut final_state = None;
        let mut now = Tick(0);
        for _ in 0..1000 {
            let cmds = advance(&plan, &state, now);
            if !cmds.is_empty() {
                for cmd in &cmds {
                    if let Command::SetExecutionState { state } = cmd {
                        final_state = Some(*state);
                    }
                    step(&plan, &mut state, cmd, &mut events);
                }
                continue;
            }
            // The caller's render of a literal `runIf:` and `If` condition.
            let mut rendered = false;
            for ix in 0..state.runs.len() {
                let task = &plan.tasks[state.runs[ix].task];
                let last = state.runs[ix].history.last().copied();
                if last == Some(RunState::Created) && state.runs[ix].rendered_run_if.is_none() {
                    if let Some(source) = &task.run_if {
                        state.runs[ix].rendered_run_if = Some(source.clone());
                        rendered = true;
                    }
                }
                if last == Some(RunState::Running) && state.runs[ix].rendered.is_none() {
                    match &task.kind {
                        TaskKind::If { condition, .. } => {
                            state.runs[ix].rendered = Some(condition.clone());
                            rendered = true;
                        }
                        TaskKind::Switch { value, .. } => {
                            state.runs[ix].rendered = Some(value.clone());
                            rendered = true;
                        }
                        _ => {}
                    }
                }
            }
            if rendered {
                continue;
            }
            let killing = state.history.last() == Some(&RunState::Killing);
            if let Some(id) = kill_when_running {
                let target_running = (0..state.runs.len()).any(|ix| {
                    id_of(&plan, &state, ix) == id
                        && state.runs[ix].history.last() == Some(&RunState::Running)
                });
                if target_running && !killing && !state.history.contains(&RunState::Killing) {
                    let cmd = Command::SetExecutionState {
                        state: RunState::Killing,
                    };
                    step(&plan, &mut state, &cmd, &mut events);
                    continue;
                }
            }
            // The worker: a runnable it holds ends, and under a kill it ends
            // `KILLED` (whether or not `advance_killing` marked it `KILLING`).
            let in_flight = (0..state.runs.len()).find(|&ix| {
                matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    TaskKind::Runnable { .. }
                ) && matches!(
                    state.runs[ix].history.last(),
                    Some(RunState::Running | RunState::Killing)
                )
            });
            if let Some(ix) = in_flight {
                let outcome = if killing {
                    RunState::Killed
                } else {
                    let raw = if fail.contains(&id_of(&plan, &state, ix)) {
                        RunState::Failed
                    } else {
                        RunState::Success
                    };
                    super::allow_failure::for_runnable(&plan, &state, ix, raw, now)
                };
                let cmd = Command::SetRunState {
                    run: ix,
                    state: outcome,
                };
                step(&plan, &mut state, &cmd, &mut events);
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
                None => break,
            }
        }
        let flow = final_state.expect("the execution reached a terminal state");
        Trace {
            plan,
            state,
            flow,
            events,
        }
    }

    fn run(src: &str, fail: &[&str]) -> (Plan, ExecutionState, RunState) {
        let t = drive(src, fail, None);
        (t.plan, t.state, t.flow)
    }

    /// The last state of the run for `id`, or `None` if no run was created.
    fn last_of(plan: &Plan, state: &ExecutionState, id: &str) -> Option<RunState> {
        let task = plan.tasks.iter().position(|t| t.id == id)?;
        state
            .runs
            .iter()
            .find(|r| r.task == task)
            .and_then(|r| r.history.last().copied())
    }

    /// The task id of every run, in creation order.
    fn order(plan: &Plan, state: &ExecutionState) -> Vec<String> {
        state
            .runs
            .iter()
            .map(|r| plan.tasks[r.task].id.clone())
            .collect()
    }

    /// The position of `event` in the log; panics with the log if absent.
    fn at(t: &Trace, event: &str) -> usize {
        t.events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no `{event}` in {:?}", t.events))
    }

    const HEAD: &str = "id: f\nnamespace: io.kestra.tests\n";
    const FAIL: &str = "io.kestra.plugin.core.execution.Fail";
    const LOG: &str = "io.kestra.plugin.core.log.Log";

    /// A leaf at `indent` spaces: `- id: <id>` of type `ty`.
    fn leaf(indent: usize, id: &str, ty: &str) -> String {
        let pad = " ".repeat(indent);
        if ty == LOG {
            format!("{pad}- id: {id}\n{pad}  type: {ty}\n{pad}  message: {id}\n")
        } else {
            format!("{pad}- id: {id}\n{pad}  type: {ty}\n")
        }
    }

    /// A `Sequential` `seq` holding `ko` (`Fail`), with `finally: [a1, a2]`.
    const SEQ_FINALLY: &str = "id: f\nnamespace: io.kestra.tests\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ko\n        type: io.kestra.plugin.core.execution.Fail\n    finally:\n      - id: a1\n        type: io.kestra.plugin.core.log.Log\n        message: a1\n      - id: a2\n        type: io.kestra.plugin.core.log.Log\n        message: a2\n";

    /// [`SEQ_FINALLY`] with `errors: [e1, e2]`.
    const SEQ_ERRORS_FINALLY: &str = "id: f\nnamespace: io.kestra.tests\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ko\n        type: io.kestra.plugin.core.execution.Fail\n    errors:\n      - id: e1\n        type: io.kestra.plugin.core.log.Log\n        message: e1\n      - id: e2\n        type: io.kestra.plugin.core.log.Log\n        message: e2\n    finally:\n      - id: a1\n        type: io.kestra.plugin.core.log.Log\n        message: a1\n      - id: a2\n        type: io.kestra.plugin.core.log.Log\n        message: a2\n";

    /// spec §0.1, golden `finally-sequential`: tasks, then finally; the block ends
    /// after its finally and keeps its tasks' state.
    #[test]
    fn a_sequential_runs_finally_after_a_failure_and_stays_failed() {
        let t = drive(SEQ_FINALLY, &["ko"], None);
        assert_eq!(order(&t.plan, &t.state), ["seq", "ko", "a1", "a2"]);
        assert_eq!(last_of(&t.plan, &t.state, "seq"), Some(RunState::Failed));
        assert_eq!(last_of(&t.plan, &t.state, "a1"), Some(RunState::Success));
        assert_eq!(last_of(&t.plan, &t.state, "a2"), Some(RunState::Success));
        assert_eq!(t.flow, RunState::Failed);
        assert!(
            at(&t, "end:a2:Success") < at(&t, "end:seq:Failed"),
            "the block ends after its finally: {:?}",
            t.events
        );
    }

    /// `finally` runs on success too.
    #[test]
    fn a_sequential_runs_finally_after_success() {
        let (plan, state, flow) = run(SEQ_FINALLY, &[]);
        assert_eq!(order(&plan, &state), ["seq", "ko", "a1", "a2"]);
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "a2"), Some(RunState::Success));
        assert_eq!(flow, RunState::Success);
    }

    /// spec §0.2: a failing finally child changes nothing and does not stop the next one.
    #[test]
    fn a_failing_finally_child_neither_fails_the_block_nor_stops_the_next() {
        let (plan, state, flow) = run(SEQ_FINALLY, &["a1"]);
        assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Failed));
        assert_eq!(last_of(&plan, &state, "a2"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
        assert_eq!(flow, RunState::Success);
    }

    /// A failing finally under a failed block: the block stays FAILED (from
    /// its tasks), and the next finally child still runs.
    #[test]
    fn a_failing_finally_child_under_a_failed_block_leaves_it_failed() {
        let (plan, state, flow) = run(SEQ_FINALLY, &["ko", "a1"]);
        assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Failed));
        assert_eq!(last_of(&plan, &state, "a2"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Failed));
        assert_eq!(flow, RunState::Failed);
    }

    /// golden `finally-sequential-error`: errors strictly before finally.
    #[test]
    fn errors_run_before_finally() {
        let t = drive(SEQ_ERRORS_FINALLY, &["ko"], None);
        assert_eq!(
            order(&t.plan, &t.state),
            ["seq", "ko", "e1", "e2", "a1", "a2"]
        );
        assert!(at(&t, "end:e2:Success") < at(&t, "create:a1"));
        assert_eq!(last_of(&t.plan, &t.state, "seq"), Some(RunState::Failed));
        assert_eq!(t.flow, RunState::Failed);
    }

    /// On success the `errors:` list is skipped and `finally:` still runs.
    #[test]
    fn errors_are_skipped_on_success_and_finally_runs() {
        let (plan, state, flow) = run(SEQ_ERRORS_FINALLY, &[]);
        assert_eq!(order(&plan, &state), ["seq", "ko", "a1", "a2"]);
        assert_eq!(flow, RunState::Success);
    }

    /// golden `finally-flow-error`: the flow's own lists in the same order, and
    /// the execution takes its terminal only after its finally.
    #[test]
    fn the_flows_errors_then_finally() {
        let src = format!(
            "{HEAD}tasks:\n{}errors:\n{}{}finally:\n{}{}",
            leaf(2, "ko", FAIL),
            leaf(2, "e1", LOG),
            leaf(2, "e2", LOG),
            leaf(2, "a1", LOG),
            leaf(2, "a2", LOG)
        );
        let t = drive(&src, &["ko"], None);
        assert_eq!(order(&t.plan, &t.state), ["ko", "e1", "e2", "a1", "a2"]);
        assert_eq!(t.flow, RunState::Failed);
        assert!(at(&t, "end:a2:Success") < at(&t, "exec:Failed"));
        assert_eq!(
            t.state
                .history
                .iter()
                .filter(|s| **s == RunState::Failed)
                .count(),
            1,
            "the execution enters FAILED once, after its finally: {:?}",
            t.state.history
        );
    }

    /// golden `finally-flow-error-first`: the sibling after the failure is never created.
    #[test]
    fn the_flows_finally_skips_the_rest_of_tasks() {
        let src = format!(
            "{HEAD}tasks:\n{}{}finally:\n{}",
            leaf(2, "ko", FAIL),
            leaf(2, "ok", LOG),
            leaf(2, "a1", LOG)
        );
        let (plan, state, flow) = run(&src, &["ko"]);
        assert_eq!(order(&plan, &state), ["ko", "a1"]);
        assert_eq!(last_of(&plan, &state, "ok"), None);
        assert_eq!(flow, RunState::Failed);
    }

    /// The flow's finally runs after success, and its state is its tasks'.
    #[test]
    fn the_flows_finally_runs_after_success_and_a_failure_in_it_changes_nothing() {
        let src = format!(
            "{HEAD}tasks:\n{}finally:\n{}{}",
            leaf(2, "ok", LOG),
            leaf(2, "a1", LOG),
            leaf(2, "a2", LOG)
        );
        let t = drive(&src, &["a1"], None);
        assert_eq!(order(&t.plan, &t.state), ["ok", "a1", "a2"]);
        assert_eq!(t.flow, RunState::Success);
        assert!(at(&t, "end:a2:Success") < at(&t, "exec:Success"));
    }

    /// golden `finally-parallel`: finally is sequential under a Parallel.
    #[test]
    fn a_parallels_finally_runs_one_at_a_time() {
        let src = format!(
            "{HEAD}tasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}    finally:\n{}{}",
            leaf(6, "p1", LOG),
            leaf(6, "p2", LOG),
            leaf(6, "a1", LOG),
            leaf(6, "a2", LOG)
        );
        let t = drive(&src, &[], None);
        assert_eq!(order(&t.plan, &t.state), ["par", "p1", "p2", "a1", "a2"]);
        assert!(at(&t, "end:p2:Success") < at(&t, "create:a1"));
        assert!(
            at(&t, "end:a1:Success") < at(&t, "create:a2"),
            "a2 is created only after a1 ended: {:?}",
            t.events
        );
        assert_eq!(last_of(&t.plan, &t.state, "par"), Some(RunState::Success));
    }

    /// golden `finally-parallel` (failed run): the Parallel's errors run, then
    /// its finally, all sequentially; the successful sibling stays SUCCESS.
    #[test]
    fn a_parallels_errors_then_finally_after_a_failure() {
        let src = format!(
            "{HEAD}tasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}    errors:\n{}{}    finally:\n{}{}",
            leaf(6, "p1", LOG),
            leaf(6, "ko", FAIL),
            leaf(6, "e1", LOG),
            leaf(6, "e2", LOG),
            leaf(6, "a1", LOG),
            leaf(6, "a2", LOG)
        );
        let t = drive(&src, &["ko"], None);
        assert_eq!(
            order(&t.plan, &t.state),
            ["par", "p1", "ko", "e1", "e2", "a1", "a2"]
        );
        assert!(at(&t, "end:e1:Success") < at(&t, "create:e2"));
        assert!(at(&t, "end:e2:Success") < at(&t, "create:a1"));
        assert!(at(&t, "end:a1:Success") < at(&t, "create:a2"));
        assert_eq!(last_of(&t.plan, &t.state, "p1"), Some(RunState::Success));
        assert_eq!(last_of(&t.plan, &t.state, "par"), Some(RunState::Failed));
        assert_eq!(t.flow, RunState::Failed);
    }

    /// spec §4.1: with no `errors:`, a failure fails the still-live siblings
    /// (egret's `fail_the_rest`) before the finally starts.
    #[test]
    fn a_parallel_with_no_errors_fails_its_live_siblings_before_its_finally() {
        let src = format!(
            "{HEAD}tasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}    finally:\n{}",
            leaf(6, "ko", FAIL),
            leaf(6, "p1", LOG),
            leaf(6, "a1", LOG)
        );
        let t = drive(&src, &["ko"], None);
        assert_eq!(order(&t.plan, &t.state), ["par", "ko", "p1", "a1"]);
        assert_eq!(last_of(&t.plan, &t.state, "p1"), Some(RunState::Failed));
        assert!(at(&t, "end:p1:Failed") < at(&t, "create:a1"));
        assert_eq!(last_of(&t.plan, &t.state, "a1"), Some(RunState::Success));
        assert_eq!(last_of(&t.plan, &t.state, "par"), Some(RunState::Failed));
    }

    /// golden `finally-dag`: the Dag's dependencies still order its tasks when
    /// it has `errors:`/`finally:`, and the lists follow one at a time.
    #[test]
    fn a_dag_keeps_its_dependencies_and_runs_errors_then_finally() {
        let src = format!(
            "{HEAD}tasks:\n  - id: dag\n    type: io.kestra.plugin.core.flow.Dag\n    tasks:\n      - task:\n          id: t1\n          type: {LOG}\n          message: t1\n      - task:\n          id: t2\n          type: {FAIL}\n        dependsOn: [t1]\n    errors:\n{}    finally:\n{}{}",
            leaf(6, "e1", LOG),
            leaf(6, "a1", LOG),
            leaf(6, "a2", LOG)
        );
        let t = drive(&src, &["t2"], None);
        assert_eq!(
            order(&t.plan, &t.state),
            ["dag", "t1", "t2", "e1", "a1", "a2"]
        );
        assert!(
            at(&t, "end:t1:Success") < at(&t, "create:t2"),
            "t2 depends on t1: {:?}",
            t.events
        );
        assert!(at(&t, "end:a1:Success") < at(&t, "create:a2"));
        assert_eq!(last_of(&t.plan, &t.state, "dag"), Some(RunState::Failed));
    }

    const IF_FINALLY: &str = "id: f\nnamespace: io.kestra.tests\ntasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{COND}\"\n    then:\n      - id: x\n        type: io.kestra.plugin.core.log.Log\n        message: x\n    finally:\n      - id: z\n        type: io.kestra.plugin.core.log.Log\n        message: z\n";

    /// `If.java:188-198`: an If whose branch is empty ends without its finally.
    #[test]
    fn an_if_with_nothing_to_run_skips_its_finally() {
        let (plan, state, flow) = run(&IF_FINALLY.replace("{COND}", "false"), &[]);
        assert_eq!(order(&plan, &state), ["if"]);
        assert_eq!(last_of(&plan, &state, "z"), None);
        assert_eq!(last_of(&plan, &state, "if"), Some(RunState::Success));
        assert_eq!(flow, RunState::Success);
    }

    /// An If that has a branch to run runs its finally after it.
    #[test]
    fn an_if_runs_its_finally_after_its_branch() {
        let (plan, state, _) = run(&IF_FINALLY.replace("{COND}", "true"), &[]);
        assert_eq!(order(&plan, &state), ["if", "x", "z"]);
        assert_eq!(last_of(&plan, &state, "if"), Some(RunState::Success));
    }

    /// A Switch runs its finally after the chosen case.
    #[test]
    fn a_switch_runs_its_finally_after_its_case() {
        let src = format!(
            "{HEAD}tasks:\n  - id: sw\n    type: io.kestra.plugin.core.flow.Switch\n    value: a\n    cases:\n      a:\n{}    finally:\n{}",
            leaf(8, "x", FAIL),
            leaf(6, "z", LOG)
        );
        let (plan, state, _) = run(&src, &["x"]);
        assert_eq!(order(&plan, &state), ["sw", "x", "z"]);
        assert_eq!(last_of(&plan, &state, "sw"), Some(RunState::Failed));
        assert_eq!(last_of(&plan, &state, "z"), Some(RunState::Success));
    }

    /// `FU:163-170`: a block whose children are all disabled is SUCCESS without finally.
    #[test]
    fn a_block_of_disabled_children_skips_its_finally() {
        let src = format!(
            "{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: x\n        type: {LOG}\n        message: x\n        disabled: true\n    finally:\n{}",
            leaf(6, "z", LOG)
        );
        let (plan, state, flow) = run(&src, &[]);
        assert_eq!(order(&plan, &state), ["seq"]);
        assert_eq!(last_of(&plan, &state, "z"), None);
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
        assert_eq!(flow, RunState::Success);
    }

    /// A child skipped by `runIf:` is terminal: the block's tasks are
    /// exhausted and its finally runs.
    #[test]
    fn a_skipped_child_is_done_and_the_finally_runs() {
        let src = format!(
            "{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: x\n        type: {LOG}\n        message: x\n        runIf: \"false\"\n    finally:\n{}",
            leaf(6, "z", LOG)
        );
        let (plan, state, _) = run(&src, &[]);
        assert_eq!(order(&plan, &state), ["seq", "x", "z"]);
        assert_eq!(last_of(&plan, &state, "x"), Some(RunState::Skipped));
        assert_eq!(last_of(&plan, &state, "z"), Some(RunState::Success));
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
    }

    /// A child whose failure is allowed is a WARNING, not a failure: the
    /// block goes on to its next child and its finally, and ends WARNING.
    #[test]
    fn an_allowed_failure_runs_the_rest_and_the_finally() {
        let src = format!(
            "{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ko\n        type: {FAIL}\n        allowFailure: true\n{}    errors:\n{}    finally:\n{}",
            leaf(6, "ok", LOG),
            leaf(6, "e1", LOG),
            leaf(6, "z", LOG)
        );
        let (plan, state, _) = run(&src, &["ko"]);
        assert_eq!(order(&plan, &state), ["seq", "ko", "ok", "z"]);
        assert_eq!(last_of(&plan, &state, "ko"), Some(RunState::Warning));
        assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Warning));
    }

    /// `Execution.java:602-606`: a failure that will be retried starts neither list.
    #[test]
    fn a_retry_pending_failure_starts_no_finally() {
        let src = format!(
            "{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ko\n        type: {FAIL}\n        retry:\n          type: constant\n          interval: PT1S\n          maxAttempts: 2\n    errors:\n{}    finally:\n{}",
            leaf(6, "e1", LOG),
            leaf(6, "z", LOG)
        );
        let t = drive(&src, &["ko"], None);
        let ko_failed: Vec<usize> = t
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| *e == "end:ko:Failed")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(ko_failed.len(), 2, "two attempts: {:?}", t.events);
        assert!(
            ko_failed[1] < at(&t, "create:e1"),
            "errors start only after the last attempt: {:?}",
            t.events
        );
        assert!(at(&t, "end:e1:Success") < at(&t, "create:z"));
        assert_eq!(last_of(&t.plan, &t.state, "seq"), Some(RunState::Failed));
    }

    /// spec §0.4: nothing new is created while the execution is KILLING, so a
    /// kill during a finally stops the finally list.
    #[test]
    fn a_kill_during_finally_creates_no_further_finally_child() {
        let t = drive(SEQ_FINALLY, &["ko"], Some("a1"));
        assert_eq!(order(&t.plan, &t.state), ["seq", "ko", "a1"]);
        assert_eq!(last_of(&t.plan, &t.state, "a1"), Some(RunState::Killed));
        assert_eq!(last_of(&t.plan, &t.state, "a2"), None);
        assert_eq!(t.flow, RunState::Killed);
    }

    /// `Execution.java:589-593` comes FIRST: once a `finally` run exists the
    /// list is `finally`, even if `errors:` has not run. No rule of this
    /// executor writes that state (a failure starts `errors` before
    /// `finally`), so it is built by hand; the arm pins Kestra's precedence,
    /// which mutation (a) of the A4 brief reorders.
    #[test]
    fn once_finally_has_started_errors_are_not_re_entered() {
        let flow = crate::spec::parse(SEQ_ERRORS_FINALLY).expect("parses");
        let plan = plan::flatten_flow(&flow).expect("plans");
        let ix = |id: &str| plan.tasks.iter().position(|t| t.id == id).unwrap();
        let mut state = ExecutionState::new(&plan);
        let t0 = Tick(0);
        for cmd in [
            Command::SetExecutionState {
                state: RunState::Running,
            },
            Command::CreateRun {
                task: ix("seq"),
                parent: None,
                value: None,
            },
            Command::StartRun { run: 0, at: t0 },
            Command::CreateRun {
                task: ix("ko"),
                parent: Some(0),
                value: None,
            },
            Command::StartRun { run: 1, at: t0 },
            Command::SetRunState {
                run: 1,
                state: RunState::Failed,
            },
            Command::CreateRun {
                task: ix("a1"),
                parent: Some(0),
                value: None,
            },
        ] {
            apply(&mut state, &cmd);
        }
        let seq = &plan.tasks[ix("seq")];
        let TaskKind::Sequential { tasks } = &seq.kind else {
            panic!("seq is a Sequential");
        };
        assert_eq!(
            super::flowable::current_list(
                &plan,
                &state,
                tasks,
                &seq.errors,
                &seq.finally,
                Some(0),
                t0
            ),
            super::flowable::Current::Finally(&seq.finally)
        );
        let cmds = advance(&plan, &state, t0);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::CreateRun { task, .. } if *task == ix("e1"))),
            "errors are not started once finally has: {cmds:?}"
        );
    }

    /// A nested block's finally runs when that block ends, before the outer
    /// block's; and a flowable inside the flow's finally runs its own.
    #[test]
    fn finally_lists_nest() {
        let src = format!(
            "{HEAD}tasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n{}        finally:\n{}{}    finally:\n{}finally:\n  - id: fs\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}    finally:\n{}",
            leaf(10, "x", FAIL),
            leaf(10, "zi", LOG),
            leaf(6, "after", LOG),
            leaf(6, "zo", LOG),
            leaf(6, "y", LOG),
            leaf(6, "zf", LOG)
        );
        let (plan, state, flow) = run(&src, &["x"]);
        assert_eq!(
            order(&plan, &state),
            ["outer", "inner", "x", "zi", "zo", "fs", "y", "zf"]
        );
        assert_eq!(last_of(&plan, &state, "inner"), Some(RunState::Failed));
        assert_eq!(last_of(&plan, &state, "outer"), Some(RunState::Failed));
        assert_eq!(last_of(&plan, &state, "fs"), Some(RunState::Success));
        assert_eq!(flow, RunState::Failed);
    }
}
