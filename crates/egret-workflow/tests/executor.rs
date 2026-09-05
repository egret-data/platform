//! Differential oracle for `advance()`: drives egret's state machine over the
//! flows in `oracle-exec/corpus/` and asserts that the **normalised semantic
//! trace** it produces is the one a pinned Kestra produced
//! (`oracle-exec/golden.json`, written by `oracle-exec/normalise.py` from
//! `traces.json`; see `oracle-exec/README.md`).
//!
//! # This is a RATCHET, not a pass/fail
//!
//! `advance()` was a stub at W3a Task 2, so **every** comparable entry
//! mismatched and all 294 were listed in `oracle-exec/baseline.txt`. Task 3
//! implements `sequential` and the root task list and banks 101 of them; 193
//! remain. A test that is simply red for six tasks measures nothing — a NEW
//! mismatch would be invisible in the noise — so this fails in **both**
//! directions:
//!
//! - an id that is **not** in the baseline mismatches — a regression;
//! - an id that **is** in the baseline now matches — progress that has to be
//!   banked by deleting that line, in the same commit that earned it.
//!
//! The second direction is what makes it a ratchet rather than a suppression
//! list. Tasks 3-8 each bank their flowable's slice. The current mismatch
//! count is printed on every run, pass or fail.
//!
//! # What is compared
//!
//! Per golden entry, all of it or none of it:
//!
//! | field | meaning |
//! |---|---|
//! | `execution` | the execution's own history state sequence, verbatim |
//! | `runs` | task id, loop `value`, parent **ordinal**, history states, per-attempt histories — in Kestra's `taskRunList` order, which is creation order |
//! | `order` | `before` / `after` / `overlapping` per ordered pair, kept only where all five Kestra rounds agreed |
//! | `spawned` | sub-execution traces of the same shape, each naming the run it was spawned by and the loop `value` it was spawned with, ordered by `loopRun.index` |
//!
//! A `refused` entry compares one thing: [`egret_workflow::executor::build_plan`]
//! must return `Err`. Over-acceptance — egret answering where Kestra refuses —
//! is the divergence class the programme forbids shipping.
//!
//! **A panicking state machine is a mismatch in every mode, and it fails the
//! test unconditionally** — the baseline does not cover it. A baselined id
//! means "egret does not reproduce this trace yet"; it never means "egret may
//! crash here". Not settling inside [`step_bound`] is reported the same way,
//! and for the same reason: an implementation that spins is a broken
//! instrument, not a wrong answer.
//!
//! # What the oracle FEEDS IN, what it costs, and where it IS circular
//!
//! `advance()` does not decide whether a `Log` task succeeds — Kestra's
//! executor does not either; its worker does. So [`simulate`] supplies exactly
//! one thing from the golden, and this section says precisely what, because an
//! earlier version of it overclaimed.
//!
//! **Fed:** the last element of the `state` sequence of a run whose `PlanTask`
//! is `Runnable`, and of each of that run's attempts — see [`golden_outcome`].
//!
//! **Not fed, and therefore compared:** which runs are created, in what order,
//! under which parent, with what `value`; every state in a run's history
//! *except* its last; the whole of a **flowable's** history including its last;
//! the execution's own history; the happens-before relation; the
//! sub-executions.
//!
//! Feeding a *flowable's* state would be circular in the way that matters: a
//! `Sequential`, `Parallel`, `Dag`, `If`, `Switch` or `Loop` run's state is the
//! executor's decision and is exactly what this oracle exists to judge.
//!
//! **It IS circular with respect to `golden.json` for that one field**, and the
//! demonstration has to be one that could have come out the other way.
//! `oracle-exec/README.md` records it in full; in short, with a state machine
//! that reproduces `valids/labels-update-task-deduplicate` exactly, rewriting
//! that entry's run terminal **and** its attempt terminal from `SUCCESS` to
//! `KILLED` — an outcome Kestra never produced for that flow — leaves the entry
//! **still matching**, because [`golden_outcome`] feeds the same lie into
//! egret's side. A normalisation bug or a capture anomaly *in that one field*
//! is invisible in both directions. What contains it is that `golden.json` is
//! machine-derived and re-derivable.
//!
//! An earlier version of this note used `valids/minimal2` and was **vacuous**:
//! that entry cannot match under any value, because its golden run history
//! carries `SUBMITTED`, which [`RunState`] cannot represent. The ratchet
//! reported the same 294 mismatches before and after, so the experiment
//! distinguished nothing. A demonstration that survives deleting the mechanism
//! it demonstrates is not evidence.
//!
//! # `corpus.json` and `golden.json` must describe the same flows
//!
//! Only `golden.json` is compared, so a corpus flow that quietly acquires no
//! golden entry would be **silently never tested**. That is the one failure the
//! ratchet cannot see, so [`every_corpus_flow_is_compared_or_excluded_by_name`]
//! binds the two and prints the excluded counts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use egret_workflow::executor::{
    advance, apply, build_plan, Command, ExecutionState, Plan, PlanTask, RunIx, RunState, TaskKind,
    Tick,
};
use serde::Deserialize;

const ORACLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec");

// --------------------------------------------------------------------------
// the golden
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GoldenEntry {
    id: String,
    verdict: Verdict,
    /// Present on `refused` entries: which of the two evidence classes Task 1
    /// measured this refusal to be. Printed in failures so a refusal that is
    /// really a missing plugin can never be mistaken for one Kestra judged.
    #[serde(default)]
    rejected_class: Option<String>,
    #[serde(default)]
    execution: Vec<String>,
    #[serde(default)]
    runs: Vec<GoldenRun>,
    #[serde(default)]
    order: Vec<(usize, usize, String)>,
    #[serde(default)]
    spawned: Vec<GoldenSpawned>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    /// Kestra published it and ran it to a terminal state.
    Runs,
    /// Kestra refused it, and the refusal is evidence about the flow rather
    /// than about the image — `invalid-flow` or `refused-by-both`.
    Refused,
}

#[derive(Debug, Deserialize)]
struct GoldenRun {
    task: String,
    /// Kestra's `TaskRun.value` is a rendered loop value and is not always a
    /// string (`loop-object` iterates maps), so it is read as JSON and
    /// canonicalised rather than assumed.
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    parent: Option<usize>,
    state: Vec<String>,
    attempts: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct GoldenSpawned {
    #[serde(default)]
    parent_run: Option<usize>,
    /// The loop iteration value this sub-execution was spawned with, read from
    /// Kestra's `loopRun.value`. It is **not** on the runs: measured over the
    /// whole corpus, every `TaskRun.value` is null and the iteration value
    /// lives here instead.
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    execution: Vec<String>,
    #[serde(default)]
    runs: Vec<GoldenRun>,
    #[serde(default)]
    order: Vec<(usize, usize, String)>,
    #[serde(default)]
    spawned: Vec<GoldenSpawned>,
}

/// The one shape both sides are rendered into. Comparing two `Trace`s rather
/// than a `Trace` against a `GoldenEntry` is deliberate: it makes it impossible
/// for the comparison to read a field on one side that it never reads on the
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Trace {
    parent_run: Option<usize>,
    value: Option<String>,
    execution: Vec<String>,
    runs: Vec<Run>,
    order: Vec<(usize, usize, String)>,
    spawned: Vec<Trace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    task: String,
    value: Option<String>,
    parent: Option<usize>,
    state: Vec<String>,
    attempts: Vec<Vec<String>>,
}

fn canonical_value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

impl From<&GoldenRun> for Run {
    fn from(g: &GoldenRun) -> Self {
        Run {
            task: g.task.clone(),
            value: canonical_value(&g.value),
            parent: g.parent,
            state: g.state.clone(),
            attempts: g.attempts.clone(),
        }
    }
}

impl From<&GoldenSpawned> for Trace {
    fn from(g: &GoldenSpawned) -> Self {
        Trace {
            parent_run: g.parent_run,
            value: canonical_value(&g.value),
            execution: g.execution.clone(),
            runs: g.runs.iter().map(Run::from).collect(),
            order: g.order.clone(),
            spawned: g.spawned.iter().map(Trace::from).collect(),
        }
    }
}

impl From<&GoldenEntry> for Trace {
    fn from(g: &GoldenEntry) -> Self {
        Trace {
            parent_run: None,
            value: None,
            execution: g.execution.clone(),
            runs: g.runs.iter().map(Run::from).collect(),
            order: g.order.clone(),
            spawned: g.spawned.iter().map(Trace::from).collect(),
        }
    }
}

impl Trace {
    /// Keep only the ordered pairs the golden **has an opinion about**, and
    /// report how many were dropped.
    ///
    /// `normalise.py` keeps a pair only where all five Kestra rounds agreed and
    /// publishes the drops: 13 pairs across the whole harvest flipped between
    /// rounds and are absent from `golden.json` for that reason. egret's side
    /// has no such notion — a pure state machine serialises in rounds, so it
    /// produces a relation for every pair — and comparing a vector of 21
    /// against a vector of 20 makes an entry mismatch on a pair the oracle
    /// explicitly declines to assert.
    ///
    /// **This is not the relation being weakened.** The predicate is untouched
    /// and so are its endpoints — creation on the left, termination on the
    /// right, strict on both sides, which
    /// [`an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended`]
    /// still pins against [`simulate_with`]'s raw output. Every pair the golden
    /// *does* carry is still compared, and a pair the golden carries that egret
    /// does not produce still mismatches, because this only ever removes.
    ///
    /// It was found by Task 4 and it is worth 1 banked entry on its own:
    /// `valids/parallel-nested`'s runs, parents, histories and attempts are
    /// identical to Kestra's, and every one of the 52 pairs the golden keeps is
    /// reproduced exactly; the only difference was the 3 pairs it dropped.
    /// `the_order_pairs_the_golden_dropped_are_the_only_ones_not_compared`
    /// derives the whole population this can touch, so it cannot quietly grow
    /// into a way of not comparing something.
    fn restrict_order_to(&mut self, golden: &Trace) -> usize {
        let keep: BTreeSet<(usize, usize)> =
            golden.order.iter().map(|(i, j, _)| (*i, *j)).collect();
        let before = self.order.len();
        self.order.retain(|(i, j, _)| keep.contains(&(*i, *j)));
        let mut dropped = before - self.order.len();
        for (mine, theirs) in self.spawned.iter_mut().zip(&golden.spawned) {
            dropped += mine.restrict_order_to(theirs);
        }
        dropped
    }

    /// A compact, diffable rendering for the failure report. One line per run,
    /// because a `{:#?}` of a thirteen-run trace is a page nobody reads.
    fn display(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "execution {:?}", self.execution);
        for (i, r) in self.runs.iter().enumerate() {
            let _ = write!(
                s,
                "\n      [{i}] {} value={:?} parent={:?} {:?} attempts={:?}",
                r.task, r.value, r.parent, r.state, r.attempts
            );
        }
        if !self.order.is_empty() {
            let _ = write!(s, "\n      order {:?}", self.order);
        }
        for (k, sub) in self.spawned.iter().enumerate() {
            let _ = write!(
                s,
                "\n      spawned[{k}] parent_run={:?} value={:?} runs={:?}",
                sub.parent_run,
                sub.value,
                sub.runs.iter().map(|r| r.task.as_str()).collect::<Vec<_>>()
            );
        }
        s
    }
}

// --------------------------------------------------------------------------
// driving egret
// --------------------------------------------------------------------------

/// What one entry's comparison produced.
#[derive(Debug)]
enum Outcome {
    /// egret planned the flow and the simulation settled.
    Trace(Trace),
    /// egret refused the flow at plan time.
    Refused(String),
    /// The state machine panicked, or did not settle. Never covered by the
    /// baseline — see this module's doc.
    Broken(String),
}

/// How many steps one execution may take before the simulation is declared
/// broken.
///
/// Derived, not picked, and **re-derived when tiers 2 and 3 arrived**: the
/// premise this comment used to carry — "a call that emits no command ends the
/// loop" — stopped being true the moment the environment gained anything to do
/// after such a call, and the change that invalidated it did not touch the
/// comment. This is the sole justification for the number that separates
/// "broken instrument" from "wrong answer", so it gets re-derived rather than
/// re-asserted.
///
/// Every iteration consumes one step and is exactly one of three:
///
/// - **tier 1** applies at least one command, and every `Command` writes at
///   most one history entry (`CreateRun` adds a run whose history is
///   `[Created]`; `SetRunState` and `SetExecutionState` push one; `StartRun`
///   pushes one plus one attempt entry). So tier-1 steps ≤ history entries
///   written.
/// - **tier 2** writes exactly one history entry (plus one attempt entry). So
///   tier-2 steps ≤ history entries written.
/// - **tier 3** writes **none** — it is the one iteration that consumes a step
///   for nothing. It is still bounded: each jump strictly increases `now` to a
///   distinct `not_before`, and every `not_before` was written by a
///   `ScheduleRetry`, which is itself a tier-1 command. So tier-3 steps ≤
///   tier-1 commands.
///
/// Total steps ≤ 3 × (history entries the finished trace holds). The
/// widest golden entry holds **765** of them across its execution, its runs,
/// their attempts and its sub-executions — `valids/loop-nested`, whose 39
/// sub-executions are most of that. Measured over all of `golden.json`. The
/// factor of 4 covers the 3 above with headroom for an implementation that is
/// correct but takes more rounds than Kestra's did.
const MEASURED_MAX_HISTORY: usize = 765;
fn step_bound() -> usize {
    4 * MEASURED_MAX_HISTORY
}

/// Read a corpus flow's YAML by its id. Both extensions: `valids/` holds 261
/// `.yaml` and 56 `.yml`, and the id carries neither.
fn corpus_source(id: &str) -> String {
    for ext in ["yaml", "yml"] {
        let path = format!("{ORACLE}/corpus/{id}.{ext}");
        if let Ok(s) = std::fs::read_to_string(&path) {
            return s;
        }
    }
    panic!("no corpus file for {id} under {ORACLE}/corpus/ with either extension");
}

/// The terminal state the golden recorded for **attempt `attempt` of the run
/// at `ix`**, as something `RunState` can hold.
///
/// Per-attempt, not per-run. Feeding the run's final state on every attempt
/// makes a run whose attempts end differently unreproducible, and four such
/// runs exist: `valids/retry-success`'s `failed` (FAILED, FAILED, FAILED,
/// SUCCESS), `valids/retry-with-flowable-errors`'s `run_script`, and both
/// `subflow1` and `subflow2` in `valids/subflow-parent-retry`. Those would have
/// been four permanent baseline lines waiting for Task 7 to discover.
///
/// The measured vocabulary is wider than `RunState` (see its doc): anything
/// egret cannot represent is fed as `Failed`, which makes the entry mismatch —
/// visibly, in the report — rather than quietly turning into a `Success` that
/// happens to agree.
///
/// `ix` is **egret's** run index used to look up **Kestra's** run. Once the two
/// have diverged that feeds one run's outcome to another. It cannot cause a
/// false pass — the trace already differs, which is why the indices diverged —
/// but the failure report is then harder to read, and the reader has to get the
/// alignment from the expected/actual run lists the report prints side by side
/// rather than from the fed value. Aligning by task id instead would guess at
/// which run the author meant, which is worse.
fn golden_outcome(golden: &Trace, ix: usize, attempt: usize) -> RunState {
    let last = golden.runs.get(ix).and_then(|r| {
        r.attempts
            .get(attempt)
            .and_then(|a| a.last())
            .or_else(|| r.state.last())
    });
    match last.map(String::as_str) {
        Some("SUCCESS") => RunState::Success,
        Some("KILLED") => RunState::Killed,
        _ => RunState::Failed,
    }
}

/// The first run the worker is still holding: a `Runnable` whose latest state
/// is `Running`.
///
/// **Earliest-created, deliberately.** `find` over `0..len` means the
/// environment always completes the oldest in-flight run first, so exactly ONE
/// interleaving is reachable: "the later-created one finishes first" never
/// happens here. That costs nothing for the happens-before relation — a run's
/// left endpoint is its creation step, so completion order cannot turn an
/// `overlapping` pair into a `before` one — but Tasks 5 and 7 must know that
/// worker return order is pinned to creation order, because a concurrency cap
/// or an `errors:` path that behaves differently when the *second* task returns
/// first is not exercised by this oracle.
fn in_flight(plan: &Plan, state: &ExecutionState) -> Option<RunIx> {
    (0..state.runs.len()).find(|&ix| {
        let r = &state.runs[ix];
        matches!(
            plan.tasks.get(r.task).map(|t| &t.kind),
            Some(TaskKind::Runnable { .. })
        ) && r.history.last() == Some(&RunState::Running)
    })
}

/// Run the state machine to a fixed point and build the trace it produced.
///
/// See this module's doc for exactly what this feeds in from the golden, what
/// that costs, and why it is not circular with respect to `advance()`.
///
/// # The environment has three tiers, in this order
///
/// 1. **The state machine decides.** If it emits commands, they are applied and
///    it is asked again. Nothing else happens while it still has work.
/// 2. **The worker finishes ONE task.** Only when tier 1 returns nothing: the
///    earliest-created `Runnable` still `Running` (see [`in_flight`]) reaches
///    the terminal state the golden recorded for its current attempt.
/// 3. **Time passes.** Only when tier 1 has nothing to do and nothing is in
///    flight: `now` jumps to the earliest `not_before` still ahead of it. Time
///    never moves while anything else can happen, so a `timeout` cannot fire
///    early and a `ScheduleRetry` cannot deadlock the simulation.
///
/// Then, and only then, the run is **settled** — or **stuck**, which is a
/// different answer and is reported as one. See the end of the loop.
///
/// **Tier 2 is the reason for this shape, and it was an accident of loop order
/// before.** The environment used to complete every `Running` task at the top of
/// the loop, *before* every tier-1 call, so the state machine was never once
/// invoked with a task in flight — proven, not inferred: a state machine that
/// panics on seeing a `Runnable` in `Running` does not fire at all under the old
/// order and fires immediately under this one. Every worker was zero-latency and
/// no interleaving was ever simulated, which put concurrency caps, `WaitFor` and
/// timeouts — Tasks 5, 6 and 7's subjects — out of reach of the instrument meant
/// to judge them.
///
/// # Why the state machine is a parameter
///
/// [`simulate`] passes the real [`advance`]. The tests below pass their own, so
/// that tiers 2 and 3, the clock rule, the stuck check and the happens-before
/// endpoints are pinned by committed assertions instead of being merely current.
/// They were not: replacing six sites in this function with unconditional
/// panics — tier 1's application body, tier 2, tier 3's scan, [`in_flight`]'s
/// closure, [`step_relation`]'s body and the `order` assembly — left the whole
/// suite reporting `8 passed; 0 failed`, because with `advance()` a stub nothing
/// committed ever reached the second iteration.
fn simulate_with(
    plan: &Plan,
    golden: &Trace,
    mut state_machine: impl FnMut(&Plan, &ExecutionState, Tick) -> Vec<Command>,
) -> Result<Trace, String> {
    let mut state = ExecutionState::new(plan);
    // The step at which each run was CREATED and at which it terminated.
    // Integers, not clocks: the happens-before relation is a claim about
    // SERIALISATION, and a pure state machine serialises in rounds.
    //
    // The left endpoint is **creation**, not `StartRun`, because that is what
    // Kestra's is: `state.startDate` equals `histories[0].date` in 4215 of 4215
    // captured runs, and `histories[0].state` is `CREATED` in all 4215. Taking
    // `StartRun` instead made egret's interval a strict sub-interval of
    // Kestra's, so `Kestra-before` implied `egret-before` and never the
    // converse — egret's `before` set was a superset, which is over-acceptance,
    // the one divergence class this programme forbids shipping. Measured on
    // `valids/labels-update-task-deduplicate` (and `valids/labels-update-task`,
    // the same shape): a state machine that creates both runs in one round and
    // only then serialises them matched `[[0, 1, "before"]]` exactly and was
    // reported as progress to bank. `an_order_pair_is_before_only_if_the_second_
    // run_was_CREATED_after_the_first_ended` is what now stops that.
    let mut created: Vec<Option<usize>> = Vec::new();
    let mut ended: Vec<Option<usize>> = Vec::new();

    let bound = step_bound();
    let mut step = 0usize;
    let mut now = Tick(0);
    loop {
        if step > bound {
            return Err(format!(
                "did not settle within {bound} steps \
                 (see MEASURED_MAX_HISTORY for the derivation)"
            ));
        }

        // Tier 1.
        let cmds = state_machine(plan, &state, now);
        if !cmds.is_empty() {
            for cmd in &cmds {
                match cmd {
                    Command::CreateRun { .. } => {
                        created.push(Some(step));
                        ended.push(None);
                    }
                    Command::SetRunState { run, state: s } if s.is_terminal() => {
                        ended[*run] = Some(step)
                    }
                    _ => {}
                }
                apply(&mut state, cmd);
            }
            step += 1;
            continue;
        }

        // Tier 2.
        if let Some(ix) = in_flight(plan, &state) {
            let attempt = state.runs[ix].attempts.len().saturating_sub(1);
            apply(
                &mut state,
                &Command::SetRunState {
                    run: ix,
                    state: golden_outcome(golden, ix, attempt),
                },
            );
            ended[ix] = Some(step);
            step += 1;
            continue;
        }

        // Tier 3.
        if let Some(t) = state
            .runs
            .iter()
            .filter_map(|r| r.not_before)
            .filter(|t| *t > now)
            .min()
        {
            now = t;
            step += 1;
            continue;
        }

        // Nothing to do, nothing in flight, no deadline ahead. That is either
        // SETTLED or STUCK, and reporting them the same way was a defect: a
        // flowable left `Running` forever is invisible to tier 2, which only
        // picks `Runnable`s, so it fell straight through to here and produced a
        // TRUNCATED trace reported as a mismatch -- a wrong answer a maintainer
        // could baseline, when this module's own rule is that an implementation
        // which cannot finish is a broken instrument. A `ScheduleRetry` whose
        // `not_before` is already past was dropped by the `> now` filter the
        // same way.
        //
        // "Started" is the qualifier that keeps the STUB honest: with
        // `advance()` returning nothing, no run is ever created and the
        // execution never leaves `[Created]`, and that empty trace is a
        // legitimate mismatch -- it is the whole baseline. An execution that has
        // started and not finished is the one that is stuck.
        let started = !state.runs.is_empty() || state.history.len() > 1;
        let unfinished: Vec<String> = state
            .runs
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.history.last().is_some_and(|s| s.is_terminal()))
            .map(|(ix, r)| {
                format!(
                    "run {ix} ({}) is {:?}{}",
                    plan.tasks.get(r.task).map_or("?", |t| t.id.as_str()),
                    r.history.last(),
                    match r.not_before {
                        Some(t) if t <= now => format!(", with not_before {t:?} already past"),
                        Some(t) => format!(", waiting for {t:?}"),
                        None => String::new(),
                    }
                )
            })
            .collect();
        let execution_unfinished = !state.history.last().is_some_and(|s| s.is_terminal());
        if started && (!unfinished.is_empty() || execution_unfinished) {
            return Err(format!(
                "STUCK after {step} step(s) at now={now:?}: nothing to do, nothing in \
                 flight, no deadline ahead, and the execution has not finished \
                 (execution history {:?}); {}",
                state.history.iter().map(|s| s.wire()).collect::<Vec<_>>(),
                if unfinished.is_empty() {
                    "every run is terminal".to_string()
                } else {
                    unfinished.join("; ")
                }
            ));
        }
        break;
    }

    let runs: Vec<Run> = state
        .runs
        .iter()
        .map(|r| Run {
            task: plan
                .tasks
                .get(r.task)
                .map(|t| t.id.clone())
                .unwrap_or_default(),
            value: r.value.clone(),
            parent: r.parent,
            state: r.history.iter().map(|s| s.wire().to_string()).collect(),
            attempts: r
                .attempts
                .iter()
                .map(|a| a.history.iter().map(|s| s.wire().to_string()).collect())
                .collect(),
        })
        .collect();

    let mut order = Vec::new();
    for i in 0..runs.len() {
        for j in (i + 1)..runs.len() {
            if let Some(rel) = step_relation(created[i], ended[i], created[j], ended[j]) {
                order.push((i, j, rel.to_string()));
            }
        }
    }

    Ok(Trace {
        parent_run: None,
        value: None,
        execution: state.history.iter().map(|s| s.wire().to_string()).collect(),
        runs,
        order,
        // Task 6 wires `Command::SpawnExecution` through; until then egret
        // spawns nothing and every entry with a sub-execution mismatches here.
        // `oracle-exec/README.md` records that hole with its count.
        spawned: Vec::new(),
    })
}

/// [`simulate_with`], driven by the real [`advance`].
fn simulate(plan: &Plan, golden: &Trace) -> Result<Trace, String> {
    simulate_with(plan, golden, advance)
}

/// `normalise.py`'s predicate, over step numbers instead of timestamps.
///
/// The same predicate, and now over the same endpoints: creation on the left,
/// termination on the right, strict on both sides. See [`simulate`] for what it
/// cost when only the right-hand endpoints matched.
fn step_relation(
    a_start: Option<usize>,
    a_end: Option<usize>,
    b_start: Option<usize>,
    b_end: Option<usize>,
) -> Option<&'static str> {
    let (a_start, a_end, b_start, b_end) = (a_start?, a_end?, b_start?, b_end?);
    if a_end < b_start {
        Some("before")
    } else if b_end < a_start {
        Some("after")
    } else {
        Some("overlapping")
    }
}

/// Everything that can go wrong, caught: a panic in `build_plan` or `advance`
/// is an [`Outcome::Broken`], never a test crash that takes the other 295
/// entries with it.
fn evaluate(entry: &GoldenEntry) -> Outcome {
    let golden = Trace::from(entry);
    let id = entry.id.clone();
    let caught = std::panic::catch_unwind(move || {
        let source = corpus_source(&id);
        match build_plan(&source) {
            Err(e) => Ok(Outcome::Refused(e.message)),
            Ok(plan) => simulate(&plan, &golden).map(Outcome::Trace),
        }
    });
    match caught {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(msg)) => Outcome::Broken(msg),
        Err(panic) => Outcome::Broken(format!("PANIC: {}", panic_message(panic))),
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

// --------------------------------------------------------------------------
// the four arms
// --------------------------------------------------------------------------

/// What one oracle run found wrong, before it is turned into a message.
struct Findings<'a> {
    /// Ids whose state machine panicked or would not settle. **Not** covered
    /// by the baseline.
    panics: &'a [String],
    /// Baseline ids that are not in `golden.json` at all.
    stale: &'a [&'a String],
    /// Mismatching ids that are not in the baseline.
    regressions: &'a [&'a String],
    /// Baseline ids that now match and must be deleted from the file.
    banked: &'a [&'a String],
    /// Per-id expected/actual detail, for the ones that have it.
    detail: &'a BTreeMap<String, String>,
}

// --------------------------------------------------------------------------
// the environment, pinned
// --------------------------------------------------------------------------
//
// Everything below drives [`simulate_with`] with a hand-written state machine,
// because with `advance()` a stub nothing else reaches the second iteration of
// its loop. Six sites in that function were replaced with unconditional panics
// and the suite still reported `8 passed; 0 failed`; these are what now dies.

/// A plan of N independent `Runnable` roots.
fn probe_plan(ids: &[&str]) -> Plan {
    Plan {
        tasks: ids
            .iter()
            .map(|id| PlanTask {
                id: (*id).to_string(),
                kind: TaskKind::Runnable {
                    type_id: "probe".to_string(),
                },
                retry: None,
                timeout: None,
                errors: Vec::new(),
            })
            .collect(),
        roots: (0..ids.len()).collect(),
        errors: Vec::new(),
    }
}

/// A golden whose runs all end `SUCCESS`, so [`golden_outcome`] feeds `Success`.
fn probe_golden(ids: &[&str]) -> Trace {
    Trace {
        parent_run: None,
        value: None,
        execution: vec!["CREATED".into(), "RUNNING".into(), "SUCCESS".into()],
        runs: ids
            .iter()
            .map(|id| Run {
                task: (*id).to_string(),
                value: None,
                parent: None,
                state: vec!["CREATED".into(), "RUNNING".into(), "SUCCESS".into()],
                attempts: vec![vec!["CREATED".into(), "RUNNING".into(), "SUCCESS".into()]],
            })
            .collect(),
        order: Vec::new(),
        spawned: Vec::new(),
    }
}

/// Starts the first `Created` run whose predecessors are all terminal, and
/// finishes the execution when they all are. Differs from [`lazy_machine`] in
/// exactly one respect: **when** the runs are created.
fn eager_machine(plan: &Plan, st: &ExecutionState, now: Tick) -> Vec<Command> {
    if st.runs.is_empty() {
        let mut out = vec![Command::SetExecutionState {
            state: RunState::Running,
        }];
        out.extend(plan.roots.iter().map(|&task| Command::CreateRun {
            task,
            parent: None,
            value: None,
        }));
        return out;
    }
    serialise_then_finish(st, now)
}

/// Creates each run only once every earlier run has terminated.
fn lazy_machine(plan: &Plan, st: &ExecutionState, now: Tick) -> Vec<Command> {
    if st.runs.is_empty() {
        return vec![
            Command::SetExecutionState {
                state: RunState::Running,
            },
            Command::CreateRun {
                task: plan.roots[0],
                parent: None,
                value: None,
            },
        ];
    }
    let all_terminal = st
        .runs
        .iter()
        .all(|r| r.history.last().is_some_and(|s| s.is_terminal()));
    if all_terminal && st.runs.len() < plan.roots.len() {
        return vec![Command::CreateRun {
            task: plan.roots[st.runs.len()],
            parent: None,
            value: None,
        }];
    }
    serialise_then_finish(st, now)
}

fn serialise_then_finish(st: &ExecutionState, now: Tick) -> Vec<Command> {
    for ix in 0..st.runs.len() {
        match st.runs[ix].history.last() {
            Some(RunState::Created) => {
                let earlier_done = st.runs[..ix]
                    .iter()
                    .all(|p| p.history.last().is_some_and(|s| s.is_terminal()));
                return if earlier_done {
                    vec![Command::StartRun { run: ix, at: now }]
                } else {
                    Vec::new()
                };
            }
            Some(s) if !s.is_terminal() => return Vec::new(),
            _ => {}
        }
    }
    if st.history.last() != Some(&RunState::Success) {
        return vec![Command::SetExecutionState {
            state: RunState::Success,
        }];
    }
    Vec::new()
}

/// **`before` means the second run was CREATED after the first ENDED**, not
/// merely started after it.
///
/// The committed guard for the endpoint that closed an over-acceptance. Two
/// machines that differ in exactly one respect — when the runs are created —
/// produce **identical runs** and **different order**. With the left endpoint
/// moved back to `StartRun`, [`eager_machine`] also reports `before` and this
/// test dies; on the real corpus that same revert let an eager machine be
/// reported as progress to bank on `valids/labels-update-task-deduplicate`.
#[test]
fn an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended() {
    let plan = probe_plan(&["a", "b"]);
    let golden = probe_golden(&["a", "b"]);

    let eager = simulate_with(&plan, &golden, eager_machine).expect("eager must settle");
    let lazy = simulate_with(&plan, &golden, lazy_machine).expect("lazy must settle");

    assert_eq!(
        eager.runs, lazy.runs,
        "the two machines must differ ONLY in when the runs are created; if their \
         runs differ, this test is not about the endpoint"
    );
    assert_eq!(
        eager.order,
        vec![(0, 1, "overlapping".to_string())],
        "both runs were created in one round, so neither can be `before` the other"
    );
    assert_eq!(
        lazy.order,
        vec![(0, 1, "before".to_string())],
        "`b` was created after `a` terminated, which is exactly what Kestra's \
         `before` records"
    );
}

/// **Restricting the order to the golden's own pairs cannot hide a
/// disagreement on a pair the golden kept.**
///
/// [`Trace::restrict_order_to`] is a change to what the ratchet compares, made
/// by Task 4 because two of its three flows were unbankable for a reason that
/// was not about the executor. A change to what an oracle compares is exactly
/// the kind that has to carry its own guard: the two machines below produce
/// the same runs and different relations, the golden keeps that pair, and the
/// restriction has to leave the disagreement visible.
///
/// The second half is the control that stops this passing vacuously: with the
/// same pair *absent* from the golden, the restriction removes it and the two
/// become equal — which is the behaviour `valids/parallel-nested` needed and
/// therefore the behaviour that has to be shown, not assumed.
#[test]
fn restricting_the_order_cannot_hide_a_disagreement_on_a_kept_pair() {
    let plan = probe_plan(&["a", "b"]);
    let golden_feed = probe_golden(&["a", "b"]);
    let eager = simulate_with(&plan, &golden_feed, eager_machine).expect("settles");
    let lazy = simulate_with(&plan, &golden_feed, lazy_machine).expect("settles");
    assert_eq!(eager.runs, lazy.runs, "same runs, by construction");
    assert_ne!(
        eager.order, lazy.order,
        "different relation, by construction"
    );

    // The golden KEEPS (0, 1): the disagreement survives the restriction.
    let mut kept = lazy.clone();
    let dropped = kept.restrict_order_to(&eager);
    assert_eq!(
        dropped, 0,
        "nothing to drop when the golden carries the pair"
    );
    assert_ne!(
        kept, eager,
        "restrict_order_to must not turn a disagreement on a KEPT pair into a match"
    );

    // The control: the golden DROPPED (0, 1), so there is nothing to compare.
    let mut unstable_golden = eager.clone();
    unstable_golden.order.clear();
    let mut restricted = lazy.clone();
    let dropped = restricted.restrict_order_to(&unstable_golden);
    assert_eq!(dropped, 1, "the one pair the golden has no opinion about");
    assert_eq!(
        restricted, unstable_golden,
        "with the pair dropped as unstable the two traces agree on everything the \
         oracle actually asserts -- this is the case valids/parallel-nested is"
    );
}

/// **The pairs the golden dropped as unstable are the whole of what the
/// restriction can touch, and they are 12 across 3 entries.**
///
/// Derived from `golden.json` on every run, never listed: this is the entire
/// population [`Trace::restrict_order_to`] is able to remove, so if a
/// re-harvest drops more pairs — or if someone widens the restriction into a
/// way of not comparing something — the number moves here first.
#[test]
fn the_order_pairs_the_golden_dropped_are_the_only_ones_not_compared() {
    fn walk(node: &Trace, id: &str, out: &mut BTreeMap<String, usize>) {
        let n = node.runs.len();
        let full = n * n.saturating_sub(1) / 2;
        if full > node.order.len() {
            *out.entry(id.to_string()).or_default() += full - node.order.len();
        }
        for sub in &node.spawned {
            walk(sub, id, out);
        }
    }
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        walk(&Trace::from(entry), &entry.id, &mut dropped);
    }
    println!("golden entries with dropped ordered pairs: {dropped:?}");
    assert_eq!(
        dropped,
        BTreeMap::from([
            ("valids/dag".to_string(), 1),
            ("valids/if".to_string(), 9),
            ("valids/parallel-nested".to_string(), 3),
        ]),
        "the whole population of pairs the golden declines to assert. \
         `oracle-exec/README.md` reports 13 dropped pairs from the harvest; the 12 \
         here are those minus the one whose entry did not reach golden.json"
    );
}

/// **Tier 2: the environment finishes an in-flight task, and the state machine
/// is asked again while one is in flight.**
///
/// Both halves are asserted. Deleting tier 2 leaves the run `Running` for ever
/// and the simulation reports STUCK; deleting the `saw_in_flight` half would
/// let the old top-of-loop order pass, which is the defect this replaced.
#[test]
fn the_worker_finishes_one_task_and_the_machine_is_asked_while_it_is_in_flight() {
    let plan = probe_plan(&["a"]);
    let golden = probe_golden(&["a"]);
    let mut saw_in_flight = false;

    let trace = simulate_with(&plan, &golden, |p, st, now| {
        if st
            .runs
            .iter()
            .any(|r| r.history.last() == Some(&RunState::Running))
        {
            saw_in_flight = true;
        }
        eager_machine(p, st, now)
    })
    .expect("must settle");

    assert!(
        saw_in_flight,
        "the state machine must be consulted while a task is in flight -- \
         concurrency caps, WaitFor and timeouts are all decisions taken in \
         exactly that state"
    );
    assert_eq!(
        trace.runs[0].state,
        vec!["CREATED", "RUNNING", "SUCCESS"],
        "the environment must supply the golden's terminal for the in-flight run"
    );
    assert_eq!(trace.execution, vec!["CREATED", "RUNNING", "SUCCESS"]);
}

/// **Tier 3: time moves only to a deadline something is waiting for.**
///
/// The machine refuses to start its run until `now` has reached the
/// `not_before` it scheduled. Without tier 3 the clock never leaves `Tick(0)`,
/// the machine never acts, and the simulation reports STUCK — so a retry could
/// never be reproduced by any implementation.
#[test]
fn time_moves_only_to_a_deadline_something_is_waiting_for() {
    let plan = probe_plan(&["a"]);
    let golden = probe_golden(&["a"]);
    const DUE: Tick = Tick(7);
    let mut seen: Vec<Tick> = Vec::new();

    let trace = simulate_with(&plan, &golden, |p, st, now| {
        seen.push(now);
        if st.runs.is_empty() {
            return vec![
                Command::SetExecutionState {
                    state: RunState::Running,
                },
                Command::CreateRun {
                    task: p.roots[0],
                    parent: None,
                    value: None,
                },
            ];
        }
        if st.runs[0].not_before.is_none() {
            return vec![Command::ScheduleRetry {
                run: 0,
                not_before: DUE,
            }];
        }
        if now < DUE {
            // Waiting. Only tier 3 can get us out of here.
            return Vec::new();
        }
        serialise_then_finish(st, now)
    })
    .expect("must settle");

    assert!(
        seen.contains(&DUE),
        "the clock must reach the scheduled deadline; saw {seen:?}"
    );
    assert!(
        seen.iter().all(|t| *t <= DUE),
        "the clock must never move PAST a deadline something is waiting for, \
         because that is how a timeout fires early; saw {seen:?}"
    );
    assert_eq!(trace.runs[0].state, vec!["CREATED", "RUNNING", "SUCCESS"]);
}

/// **An execution that cannot finish is STUCK, not a short trace.**
///
/// A run left `Created` is invisible to tier 2, which only completes
/// `Runnable`s that are `Running`; it reaches tier 3, finds no deadline, and
/// before this check fell straight out of the loop as a truncated trace — a
/// *wrong answer* a maintainer could baseline, when this module's rule is that
/// an implementation which cannot finish is a broken instrument.
///
/// The second half is the qualifier that keeps the stub honest: a machine that
/// does nothing at all has not started anything, and its empty trace is the
/// legitimate mismatch the whole baseline is made of.
#[test]
fn an_execution_that_cannot_finish_is_reported_as_stuck_and_an_idle_stub_is_not() {
    let plan = probe_plan(&["a"]);
    let golden = probe_golden(&["a"]);

    let err = simulate_with(&plan, &golden, |p, st, _| {
        if st.runs.is_empty() {
            vec![Command::CreateRun {
                task: p.roots[0],
                parent: None,
                value: None,
            }]
        } else {
            Vec::new()
        }
    })
    .expect_err("a run left CREATED for ever must not pass as a finished trace");
    assert!(err.contains("STUCK"), "got: {err}");
    assert!(
        err.contains("run 0 (a)"),
        "the message must name the run that is stuck; got: {err}"
    );

    let idle = simulate_with(&plan, &golden, |_, _, _| Vec::new())
        .expect("a machine that never starts anything is not stuck -- that is the stub");
    assert!(idle.runs.is_empty());
    assert_eq!(idle.execution, vec!["CREATED"]);
}

/// **[`step_relation`] itself**, which nothing else asserted.
#[test]
fn step_relation_is_strict_on_both_sides() {
    let r =
        |a: usize, b: usize, c: usize, d: usize| step_relation(Some(a), Some(b), Some(c), Some(d));
    assert_eq!(
        r(0, 1, 2, 3),
        Some("before"),
        "a ended before b was created"
    );
    assert_eq!(r(2, 3, 0, 1), Some("after"), "b ended before a was created");
    assert_eq!(
        r(0, 2, 1, 3),
        Some("overlapping"),
        "the intervals share a step"
    );
    assert_eq!(
        r(0, 1, 1, 2),
        Some("overlapping"),
        "touching at a step is NOT `before` -- the predicate is strict, matching \
         Kestra's `isAfter`"
    );
    assert_eq!(
        r(0, 0, 0, 0),
        Some("overlapping"),
        "two runs created and ended in the same round overlap"
    );
    assert_eq!(
        step_relation(Some(0), None, Some(1), Some(2)),
        None,
        "a run with no end is unmeasurable, and an unmeasurable pair is dropped"
    );
    assert_eq!(step_relation(None, None, None, None), None);
}

/// **The fed outcome is per ATTEMPT, not per run.**
///
/// Four runs in the golden have attempts that end differently — measured, not
/// assumed: `valids/retry-success`'s `failed` (FAILED, FAILED, FAILED, SUCCESS,
/// with the run itself ending WARNING), `valids/retry-with-flowable-errors`'s
/// `run_script`, and both `subflow1` and `subflow2` in
/// `valids/subflow-parent-retry`. Feeding the run's final state on every attempt
/// makes all four unreproducible — four permanent baseline lines for Task 7 to
/// discover. Changing [`golden_outcome`] back to `r.state.last()` compiles
/// cleanly; this is what dies.
#[test]
fn the_fed_outcome_is_per_attempt_not_per_run() {
    let golden = golden();
    let entry = golden
        .iter()
        .find(|e| e.id == "valids/retry-success")
        .expect("golden.json must carry valids/retry-success");
    let trace = Trace::from(entry);
    let ix = trace
        .runs
        .iter()
        .position(|r| r.task == "failed")
        .expect("valids/retry-success must have a run called `failed`");

    assert_eq!(
        trace.runs[ix].attempts.len(),
        4,
        "the measurement this test rests on has moved"
    );
    assert_eq!(
        trace.runs[ix]
            .attempts
            .iter()
            .map(|a| a.last().unwrap().as_str())
            .collect::<Vec<_>>(),
        vec!["FAILED", "FAILED", "FAILED", "SUCCESS"],
        "the measurement this test rests on has moved"
    );

    assert_eq!(
        golden_outcome(&trace, ix, 0),
        RunState::Failed,
        "attempt 0 ended FAILED and must be fed as Failed"
    );
    assert_eq!(
        golden_outcome(&trace, ix, 3),
        RunState::Success,
        "attempt 3 ended SUCCESS and must be fed as Success -- a per-RUN outcome \
         would feed this on attempt 0 too and no retry could ever be reproduced"
    );
    // The run itself ends WARNING, which `RunState` cannot hold, so the
    // out-of-range fallback is Failed rather than a Success that agrees by luck.
    assert_eq!(golden_outcome(&trace, ix, 9), RunState::Failed);
}

/// The ratchet's whole arithmetic: which baseline lines are stale, which
/// mismatches are regressions, and which baselined ids have been earned.
///
/// A pure function for the same reason [`assemble_failures`] is — the rule
/// below is otherwise unreachable by any test, because a green run sits exactly
/// at baseline with all three lists empty.
///
/// **A panicked id is not banked.** It lands in `panicked` and never in
/// `mismatched`, so `!mismatched.contains(id)` alone read a baselined entry
/// that CRASHED as one that now matches. Measured, with `build_plan` panicking
/// on 7 flows: the run reported the PANIC arm *and* `PROGRESS NOT BANKED: 7
/// entr(y/ies) ... now MATCH. Delete these lines` — telling a maintainer to bank
/// progress on seven entries that could not run at all. A ratchet that hands out
/// a free pass for a crash is the failure this file exists to prevent.
fn classify<'a>(
    golden_ids: &BTreeSet<String>,
    baseline: &'a BTreeSet<String>,
    mismatched: &'a BTreeSet<String>,
    panicked: &[String],
) -> (Vec<&'a String>, Vec<&'a String>, Vec<&'a String>) {
    let stale = baseline
        .iter()
        .filter(|id| !golden_ids.contains(*id))
        .collect();
    let regressions = mismatched
        .iter()
        .filter(|id| !baseline.contains(*id))
        .collect();
    let banked = baseline
        .iter()
        .filter(|id| {
            golden_ids.contains(*id) && !mismatched.contains(*id) && !panicked.contains(id)
        })
        .collect();
    (stale, regressions, banked)
}

/// **A baselined id that PANICKED is not "progress".**
///
/// The rule in [`classify`], which nothing else reaches: a green run has all
/// three lists empty. Deleting `&& !panicked.contains(id)` compiles cleanly and
/// leaves every other test green; this is what dies.
#[test]
fn a_baselined_id_that_panicked_is_not_reported_as_banked() {
    let id = "valids/sequential".to_string();
    let golden_ids = BTreeSet::from([id.clone(), "valids/parallel".to_string()]);
    let baseline = BTreeSet::from([id.clone(), "valids/parallel".to_string()]);
    let mismatched = BTreeSet::from(["valids/parallel".to_string()]);
    let panicked = vec![id.clone()];

    let (stale, regressions, banked) = classify(&golden_ids, &baseline, &mismatched, &panicked);
    assert!(stale.is_empty(), "nothing here is stale; got {stale:?}");
    assert!(
        regressions.is_empty(),
        "the mismatching id IS baselined, so it is not a regression; got {regressions:?}"
    );
    assert!(
        banked.is_empty(),
        "an id that PANICKED must never be reported as progress to bank; got {banked:?}"
    );

    // And the same id, baselined and merely matching, IS banked -- so the
    // assertion above is about the panic and not about `banked` never firing.
    let (_, _, banked) = classify(&golden_ids, &baseline, &mismatched, &[]);
    assert_eq!(
        banked,
        vec![&"valids/sequential".to_string()],
        "with nothing panicking, a baselined id that matches must be banked"
    );
}

/// Turns [`Findings`] into the failure message, or an empty string.
///
/// A pure function on purpose. The ratchet's own assertion is
/// `assert!(failures.is_empty())`, so **every** rule about what fails lives
/// here and can be exercised without a state machine — in particular the rule
/// that a panic fails outside the baseline, which is otherwise unreachable by
/// any test. Each of the four arms has a test below that dies when that arm is
/// neutered; a green run sits exactly at baseline, so `stale`, `regressions`
/// and `banked` are all empty on every passing run and nothing else would ever
/// construct a [`Findings`].
fn assemble_failures(f: &Findings<'_>) -> String {
    let detail_for = |ids: &[&String]| {
        ids.iter()
            .filter_map(|id| f.detail.get(*id).map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut failures = String::new();

    // Unconditional, and NOT subject to the baseline. A baselined id means
    // "egret does not reproduce this trace yet"; it never means "egret may
    // crash here".
    if !f.panics.is_empty() {
        failures.push_str(&format!(
            "\nPANIC: the state machine panicked or would not settle on {} entr(y/ies). \
             Neither is ever an acceptable outcome, baselined or not:\n{}\n",
            f.panics.len(),
            f.panics
                .iter()
                .filter_map(|id| f.detail.get(id).map(String::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !f.stale.is_empty() {
        failures.push_str(&format!(
            "\noracle-exec/baseline.txt names {} id(s) that are not in golden.json at \
             all -- delete them: {:?}\n",
            f.stale.len(),
            f.stale
        ));
    }
    if !f.regressions.is_empty() {
        failures.push_str(&format!(
            "\nREGRESSION: {} entr(y/ies) mismatch that were NOT in \
             oracle-exec/baseline.txt. Fix them, do not add them to the baseline:\n{}\n",
            f.regressions.len(),
            detail_for(f.regressions)
        ));
    }
    if !f.banked.is_empty() {
        failures.push_str(&format!(
            "\nPROGRESS NOT BANKED: {} entr(y/ies) in oracle-exec/baseline.txt now \
             MATCH. Delete these lines from oracle-exec/baseline.txt in the same commit \
             that fixed them:\n  {}\n",
            f.banked.len(),
            f.banked
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ));
    }

    failures
}

/// **The rule that a panic fails OUTSIDE the ratchet**, which nothing else
/// reaches.
///
/// With no regressions, no stale lines and nothing banked — a run that is green
/// by the ratchet's own reckoning, because every panicking id was already
/// baselined — a panic must still fail. Deleting the `panics` block in
/// [`assemble_failures`] compiles cleanly and leaves every other test green;
/// this is what dies.
#[test]
fn a_panic_fails_even_when_the_ratchet_is_satisfied() {
    let panicked = vec!["valids/sequential".to_string()];
    let mut detail = BTreeMap::new();
    detail.insert(
        "valids/sequential".to_string(),
        "  valids/sequential: PANIC: index out of bounds".to_string(),
    );

    let ratchet_is_happy = Findings {
        panics: &panicked,
        stale: &[],
        regressions: &[],
        banked: &[],
        detail: &detail,
    };
    let message = assemble_failures(&ratchet_is_happy);
    assert!(
        message.contains("PANIC"),
        "a panic on a baselined id must fail the run; got: {message:?}"
    );
    assert!(
        message.contains("valids/sequential"),
        "the failure must name the entry; got: {message:?}"
    );

    // And the same findings without the panic are a pass, so the assertion
    // above is about the panic and not about the function always complaining.
    let clean = Findings {
        panics: &[],
        stale: &[],
        regressions: &[],
        banked: &[],
        detail: &detail,
    };
    assert_eq!(assemble_failures(&clean), "");
}

/// **The `stale` arm fires and names the ids.**
///
/// A baseline line naming an id `golden.json` no longer holds is a free pass
/// that can never be spent, and silently accumulating those is how a ratchet
/// becomes a suppression list.
#[test]
fn a_stale_baseline_line_fails_and_names_it() {
    let gone = "valids/an-id-golden-json-does-not-have".to_string();
    let stale = [&gone];
    let message = assemble_failures(&Findings {
        panics: &[],
        stale: &stale,
        regressions: &[],
        banked: &[],
        detail: &BTreeMap::new(),
    });
    assert!(
        !message.is_empty(),
        "a baseline line with no golden entry behind it must fail the run"
    );
    assert!(
        message.contains("not in golden.json"),
        "the message must say what is wrong; got: {message:?}"
    );
    assert!(
        message.contains("valids/an-id-golden-json-does-not-have"),
        "the message must NAME the stale id, or a maintainer cannot act on it; got: {message:?}"
    );

    assert_eq!(
        assemble_failures(&Findings {
            panics: &[],
            stale: &[],
            regressions: &[],
            banked: &[],
            detail: &BTreeMap::new(),
        }),
        ""
    );
}

/// **The `regressions` arm fires, names the ids, and carries their detail.**
///
/// The direction the ratchet exists for: a trace that differs and is *not*
/// baselined. The detail assertion is deliberate — a regression reported by
/// bare id, with no expected/actual, is most of the way to unactionable.
#[test]
fn a_regression_outside_the_baseline_fails_and_carries_its_detail() {
    let id = "valids/parallel".to_string();
    let regressions = [&id];
    let mut detail = BTreeMap::new();
    detail.insert(
        id.clone(),
        "  valids/parallel:\n    expected execution [\"CREATED\"]\n    actual execution []"
            .to_string(),
    );

    let message = assemble_failures(&Findings {
        panics: &[],
        stale: &[],
        regressions: &regressions,
        banked: &[],
        detail: &detail,
    });
    assert!(
        message.contains("REGRESSION"),
        "a mismatch outside the baseline must fail as a regression; got: {message:?}"
    );
    assert!(
        message.contains("valids/parallel"),
        "the message must name the entry; got: {message:?}"
    );
    assert!(
        message.contains("actual execution []"),
        "the message must carry the per-id detail, not just the id; got: {message:?}"
    );
    assert!(
        message.contains("do not add them to the baseline"),
        "the message must say which way to fix it; got: {message:?}"
    );

    assert_eq!(
        assemble_failures(&Findings {
            panics: &[],
            stale: &[],
            regressions: &[],
            banked: &[],
            detail: &detail,
        }),
        ""
    );
}

/// **The `banked` arm fires and names the ids.**
///
/// The direction that makes this a ratchet rather than a suppression list: a
/// baselined id that now MATCHES must fail until its line is deleted. Tasks 3-8
/// each bank a slice, and this arm is what refuses to let a task claim progress
/// without shrinking the file — or leave progress it did make unrecorded.
#[test]
fn progress_that_is_not_banked_fails_and_names_it() {
    let fixed = "valids/sequential".to_string();
    let banked = [&fixed];
    let message = assemble_failures(&Findings {
        panics: &[],
        stale: &[],
        regressions: &[],
        banked: &banked,
        detail: &BTreeMap::new(),
    });
    assert!(
        message.contains("PROGRESS NOT BANKED"),
        "a baselined id that now matches must fail the run; got: {message:?}"
    );
    assert!(
        message.contains("valids/sequential"),
        "the message must name the line to delete; got: {message:?}"
    );
    assert!(
        message.contains("oracle-exec/baseline.txt"),
        "the message must say which file to edit; got: {message:?}"
    );

    assert_eq!(
        assemble_failures(&Findings {
            panics: &[],
            stale: &[],
            regressions: &[],
            banked: &[],
            detail: &BTreeMap::new(),
        }),
        ""
    );
}

// --------------------------------------------------------------------------
// the corpus/golden binding
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CorpusEntry {
    id: String,
    disposition: String,
    #[serde(default)]
    rejected_class: Option<String>,
}

fn corpus() -> Vec<CorpusEntry> {
    let raw = include_str!("../oracle-exec/corpus.json");
    serde_json::from_str(raw).expect("oracle-exec/corpus.json must parse")
}

fn golden() -> Vec<GoldenEntry> {
    let raw = include_str!("../oracle-exec/golden.json");
    serde_json::from_str(raw).expect("oracle-exec/golden.json must parse")
}

/// Every corpus flow is either compared or excluded **by name**, and the counts
/// are printed.
///
/// An entry that is silently skipped is worse than one that is honestly
/// excluded: the coverage claim keeps its denominator and loses its numerator.
/// The three exclusions are Task 1 and Task 2 measurements, not conveniences:
///
/// - `not-standalone` — Kestra itself cannot run it without what the Java test
///   brings (a resume, a trigger, an uploadable file, a computed input).
/// - a rejection whose class is `task-type-not-in-image` or `checkout-drift` —
///   the pinned image or the sibling checkout differing, **never** Kestra
///   judging the flow invalid. Demanding a refusal for those would manufacture
///   defects out of a packaging accident.
/// - a flow whose normalised trace was not identical in all five Kestra rounds
///   — a golden the oracle does not reproduce cannot judge egret.
#[test]
fn every_corpus_flow_is_compared_or_excluded_by_name() {
    let corpus = corpus();
    let golden_ids: BTreeSet<String> = golden().iter().map(|e| e.id.clone()).collect();
    let unstable: BTreeSet<String> = stability_unstable();

    let mut compared = 0usize;
    let (mut not_standalone, mut not_refusal_evidence, mut unstable_n) = (0, 0, 0);
    let mut unaccounted = Vec::new();

    for e in &corpus {
        if golden_ids.contains(&e.id) {
            compared += 1;
        } else if e.disposition == "not-standalone" {
            not_standalone += 1;
        } else if matches!(
            e.rejected_class.as_deref(),
            Some("task-type-not-in-image") | Some("checkout-drift")
        ) {
            not_refusal_evidence += 1;
        } else if unstable.contains(&e.id) {
            unstable_n += 1;
        } else {
            unaccounted.push(e.id.clone());
        }
    }

    println!(
        "oracle-exec: compared {compared}, excluded not-standalone {not_standalone}, \
         excluded non-evidence rejections {not_refusal_evidence}, \
         excluded unstable traces {unstable_n}, TOTAL {}",
        compared + not_standalone + not_refusal_evidence + unstable_n
    );

    assert!(
        unaccounted.is_empty(),
        "{} corpus flow(s) have no golden entry and no named exclusion, so they are \
         silently never compared: {unaccounted:?}",
        unaccounted.len()
    );
    let extra: Vec<&String> = golden_ids
        .iter()
        .filter(|id| !corpus.iter().any(|c| &c.id == *id))
        .collect();
    assert!(
        extra.is_empty(),
        "golden.json holds {} id(s) that are not in corpus.json: {extra:?}",
        extra.len()
    );
}

/// The ids `normalise.py` measured as not reproducing across five rounds.
fn stability_unstable() -> BTreeSet<String> {
    #[derive(Deserialize)]
    struct Stability {
        unstable_traces: Vec<String>,
    }
    let raw = include_str!("../oracle-exec/stability.json");
    let s: Stability = serde_json::from_str(raw).expect("oracle-exec/stability.json must parse");
    s.unstable_traces.into_iter().collect()
}

/// The baseline: the ids that do not match yet.
///
/// One id per line; `#` starts a comment and blank lines are ignored. Deleting
/// a line is how a task banks progress, and the test refuses to pass if a line
/// is still there for an entry that now matches.
fn baseline() -> BTreeSet<String> {
    include_str!("../oracle-exec/baseline.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

// --------------------------------------------------------------------------
// the ratchet
// --------------------------------------------------------------------------

/// The ratchet. See this module's doc for the two directions it fails in.
#[test]
fn executor_matches_golden() {
    let golden = golden();
    let baseline = baseline();

    let mut mismatched: BTreeSet<String> = BTreeSet::new();
    let mut panicked: Vec<String> = Vec::new();
    let mut detail: BTreeMap<String, String> = BTreeMap::new();
    let mut uncompared_pairs = 0usize;

    for entry in &golden {
        match (entry.verdict, evaluate(entry)) {
            (Verdict::Refused, Outcome::Refused(_)) => {}
            (Verdict::Refused, Outcome::Trace(_)) => {
                mismatched.insert(entry.id.clone());
                detail.insert(
                    entry.id.clone(),
                    format!(
                        "  {}: OVER-ACCEPTED. Kestra refused it ({}); egret planned it.",
                        entry.id,
                        entry
                            .rejected_class
                            .as_deref()
                            .unwrap_or("no class recorded")
                    ),
                );
            }
            (Verdict::Runs, Outcome::Refused(msg)) => {
                mismatched.insert(entry.id.clone());
                detail.insert(
                    entry.id.clone(),
                    format!("  {}: egret REFUSED a flow Kestra ran: {msg}", entry.id),
                );
            }
            (Verdict::Runs, Outcome::Trace(mut actual)) => {
                let expected = Trace::from(entry);
                uncompared_pairs += actual.restrict_order_to(&expected);
                if actual != expected {
                    mismatched.insert(entry.id.clone());
                    detail.insert(
                        entry.id.clone(),
                        format!(
                            "  {}:\n    expected {}\n    actual   {}",
                            entry.id,
                            expected.display(),
                            actual.display()
                        ),
                    );
                }
            }
            (_, Outcome::Broken(msg)) => {
                panicked.push(entry.id.clone());
                detail.insert(entry.id.clone(), format!("  {}: {msg}", entry.id));
            }
        }
    }

    let golden_ids: BTreeSet<String> = golden.iter().map(|e| e.id.clone()).collect();
    let (stale, regressions, banked) = classify(&golden_ids, &baseline, &mismatched, &panicked);

    println!(
        "oracle-exec ratchet: {} golden entries, {} mismatching, {} baselined, {} broken, \
         {uncompared_pairs} ordered pair(s) removed from egret's side for having no \
         counterpart in the golden (see Trace::restrict_order_to; the golden-side \
         population is derived by \
         the_order_pairs_the_golden_dropped_are_the_only_ones_not_compared)",
        golden.len(),
        mismatched.len(),
        baseline.len(),
        panicked.len()
    );

    let failures = assemble_failures(&Findings {
        panics: &panicked,
        stale: &stale,
        regressions: &regressions,
        banked: &banked,
        detail: &detail,
    });
    assert!(failures.is_empty(), "{failures}");
}

// --------------------------------------------------------------------------
// how much of the golden the feed absorbs
// --------------------------------------------------------------------------

/// **How many of the golden's run terminals `golden_outcome` feeds in — the
/// number `oracle-exec/README.md` could only quote until Task 3 built a plan
/// builder.**
///
/// The oracle is circular with respect to `golden.json` for exactly one field:
/// the last element of the `state` sequence of a run whose [`PlanTask`] is
/// `Runnable`, and of each of that run's attempts. This measures how far that
/// reaches, and it is derived from `build_plan` and `golden.json` on every run
/// rather than quoted — the figure this replaced ("812 of 840") came from a
/// reviewer's own plan builder and could not be reproduced here at all.
///
/// Three buckets, and every run terminal is in exactly one:
///
/// - **fed** — the run's task is a `Runnable` in the plan, so its terminal is
///   supplied and cannot be compared.
/// - **flowable** — the run's task is one this executor drives itself
///   (`Sequential`, `Parallel`, `Dag`), whose state is its own decision and is
///   compared.
/// - **unplanned** — the run's task has no `PlanTask` at all, because it lives
///   inside a flowable Task 3 plans opaque and therefore does not descend into.
///   Tasks 4-8 move these into one of the two buckets above.
///
/// The fourth number is the correction, and it is why "fed" is not the honest
/// end state: `SUBMITTED` appears on every task Kestra's *worker* ran and never
/// on a flowable its *executor* ran, so a fed run whose golden history has no
/// `SUBMITTED` is one a complete plan builder would move to `flowable`.
#[test]
fn the_feed_absorbs_a_measured_share_of_the_golden_run_terminals() {
    let (mut fed, mut flowable, mut unplanned, mut refused_flows) =
        (0usize, 0usize, 0usize, 0usize);
    let (mut fed_without_submitted, mut parents, mut submitted) = (0usize, 0usize, 0usize);
    // Which task TYPES egret currently hands to a worker that Kestra's executor
    // ran itself. Derived from the plan's own `type_id`, never a hand list --
    // the family is wider than any one name, and a list would go stale.
    let mut over_fed: BTreeMap<String, usize> = BTreeMap::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        let Ok(plan) = build_plan(&corpus_source(&entry.id)) else {
            refused_flows += 1;
            let mut stack: Vec<&Trace> = vec![&trace];
            while let Some(t) = stack.pop() {
                unplanned += t.runs.len();
                stack.extend(t.spawned.iter());
            }
            continue;
        };
        // Sub-execution runs count too: a LOOP sub-execution and a
        // CREATE_NEW_EXECUTION replacement both run tasks of the SAME flow, so
        // the same plan classifies them. The golden's 840 is this walk, not the
        // top-level lists alone -- which is worth stating, because counting
        // only the top level gives 570 and reads just as much like a
        // measurement.
        let mut stack: Vec<&Trace> = vec![&trace];
        while let Some(t) = stack.pop() {
            let has_children: BTreeSet<usize> = t.runs.iter().filter_map(|r| r.parent).collect();
            for (ix, run) in t.runs.iter().enumerate() {
                if has_children.contains(&ix) {
                    parents += 1;
                }
                if run.state.iter().any(|s| s == "SUBMITTED") {
                    submitted += 1;
                }
                // By task id, which is what the golden carries. A flow with
                // duplicate task ids would take the first; none of the `runs`
                // entries has one.
                match plan
                    .tasks
                    .iter()
                    .find(|p| p.id == run.task)
                    .map(|p| &p.kind)
                {
                    None => unplanned += 1,
                    Some(
                        TaskKind::Sequential { .. }
                        | TaskKind::Parallel { .. }
                        | TaskKind::Dag { .. },
                    ) => flowable += 1,
                    Some(TaskKind::Runnable { type_id }) => {
                        fed += 1;
                        if !run.state.iter().any(|s| s == "SUBMITTED") {
                            fed_without_submitted += 1;
                            *over_fed.entry(type_id.clone()).or_default() += 1;
                        }
                    }
                }
            }
            stack.extend(t.spawned.iter());
        }
    }

    let total = fed + flowable + unplanned;
    println!(
        "golden run terminals: {total} total = {fed} fed + {flowable} flowable + \
         {unplanned} unplanned ({refused_flows} flow(s) build_plan refuses); of the fed, \
         {fed_without_submitted} carry no SUBMITTED and so are flowables a complete \
         plan builder would not feed; {parents} run(s) are the parent of another run; \
         {submitted} carry SUBMITTED and are what a COMPLETE plan builder would feed"
    );

    assert_eq!(
        total, 840,
        "the golden holds 840 run terminals; if that has moved, every count in \
         oracle-exec/README.md is stale"
    );
    assert_eq!(
        (fed, flowable, unplanned, fed_without_submitted),
        (428, 48, 364, 134),
        "these four numbers are written into oracle-exec/README.md under `What the \
         oracle feeds in`. They move as Tasks 4-8 teach `build_plan` more flowables, \
         and when they do the README has to move with them -- which is the point of \
         asserting them here rather than quoting them there."
    );

    // The end state, independent of how complete `build_plan` happens to be:
    // `SUBMITTED` is on every task Kestra's WORKER ran and on no flowable its
    // EXECUTOR ran, so this is what a plan builder that knew every flowable
    // would feed. It also refutes the figure this test replaced: the review's
    // quoted **812** exceeds not only this but the README's own upper bound of
    // 840 - 120 = 720, so it could not have been right whatever the builder.
    assert_eq!(
        (submitted, total - submitted),
        (580, 260),
        "the golden's own worker/executor discriminator has moved"
    );
    // Pinned, not printed. The README's "812 was impossible" rests entirely on
    // the 840 - 120 = 720 ceiling, and a ceiling no test holds is a ceiling that
    // can move without anyone noticing -- inside a test whose whole purpose is
    // that a figure cannot move without the README moving.
    assert_eq!(
        parents, 120,
        "120 golden runs are the parent of another run and are therefore \
         certainly flowables whose terminals are never fed. That is the entire \
         basis for the 720 ceiling in oracle-exec/README.md, and for the finding \
         that the quoted 812 was impossible rather than merely wrong."
    );
    assert!(
        submitted < total - parents,
        "a run that is the parent of another is certainly a flowable, so the fed \
         count can never exceed {}",
        total - parents
    );

    // The types egret over-submits, derived. `SUBMITTED` is Kestra handing a
    // task to a WORKER; a golden run without it was run by the EXECUTOR itself.
    // So every type below is one `advance()` currently treats as a worker task
    // and Kestra does not -- flowables Tasks 4-6 will claim, and one family that
    // no W3a task claims at all.
    println!("over-submitted by type: {over_fed:?}");
    assert_eq!(
        over_fed.values().sum::<usize>(),
        fed_without_submitted,
        "the by-type breakdown must account for every over-submitted run"
    );

    let bucket = |prefix: &str| -> (usize, usize) {
        let m: Vec<usize> = over_fed
            .iter()
            .filter(|(t, _)| t.starts_with(prefix))
            .map(|(_, n)| *n)
            .collect();
        (m.len(), m.iter().sum())
    };
    assert_eq!(
        (
            over_fed.len(),
            bucket("io.kestra.plugin.core.flow."),
            bucket("io.kestra.plugin.core.execution.")
        ),
        (13, (9, 113), (3, 19)),
        "13 task types are over-submitted, down from Task 3's 15: `core.flow.\
         Parallel` and `core.flow.Dag` left the set in the commit that gave them \
         a TaskKind, which is the whole point of asserting it. NINE are still \
         flowables (113 runs) -- and the count went UP for two of them rather \
         than down, because planning a Parallel or a Dag is what makes the \
         opaque flowables INSIDE one reachable at all: `If` 12 -> 19 and \
         `Subflow` 18 -> 22. THREE are `io.kestra.plugin.core.execution.*` (19 \
         runs) -- Kestra's `ExecutionUpdatableTask` family (`Labels`, \
         `SetVariables`, `UnsetVariables`, `Exit`; `core/src/main/java/io/kestra/\
         plugin/core/execution/Labels.java:79`), which is neither a \
         `RunnableTask` nor a flowable and which NO W3a task claims -- nor does \
         any task claim `core.flow.Subflow`, `core.flow.Pause`, \
         `core.flow.AllowFailure` or `core.flow.WorkingDirectory`, 40 more of \
         these runs between them. See oracle-exec/README.md, `States and \
         constructs no W3a task claims`."
    );

    // The remainder is the measured EXCEPTION to the discriminator this test
    // rests on, and it is not zero. `SUBMITTED` is not on every run of a task
    // the worker ran: a `Log` -- an unambiguous `RunnableTask` -- that Kestra
    // never handed to a worker because `runIf` SKIPPED it, or that failed before
    // submission, has no `SUBMITTED` either. So 580 is the number of runs Kestra
    // DID submit, and it is a LOWER bound on what a complete plan builder would
    // feed, not the exact figure.
    let other: BTreeMap<&str, usize> = over_fed
        .iter()
        .filter(|(t, _)| {
            !t.starts_with("io.kestra.plugin.core.flow.")
                && !t.starts_with("io.kestra.plugin.core.execution.")
        })
        .map(|(t, n)| (t.as_str(), *n))
        .collect();
    assert_eq!(
        other,
        BTreeMap::from([("io.kestra.plugin.core.log.Log", 2)]),
        "a run of a genuine RunnableTask with no SUBMITTED is the exception that \
         bounds the 580; if this set grows, oracle-exec/README.md's phrasing of \
         the discriminator has to weaken with it"
    );
}
