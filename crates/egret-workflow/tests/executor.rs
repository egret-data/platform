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
//! **Fed, and new at Task 6: a `Loop` run's terminal too.** That is a
//! **correction** to the sentence below, which named `Loop` among the flowables
//! whose state is compared, and it is made on a measurement rather than for
//! convenience. A `Loop`'s state is a function of its **sub-executions**, and
//! this oracle runs none: `Command::SpawnExecution` is emitted and nothing
//! executes it until W3b/W6. So there is no decision `advance()` could take
//! here to be judged — feeding it is the same class as feeding a worker's
//! result, and the alternative is not "compare it" but STUCK, which the
//! baseline does not cover and which would turn **35** honestly-baselined
//! entries into broken ones
//! ([`the_loop_completion_tier_is_load_bearing_for_exactly_these_entries`]
//! derives the 35 and requires every one of them to be baselined; round 0 of
//! this task said 37, in prose, pinned by nothing).
//!
//! **This trade costs less than round 0 said, twice over.** No terminal moved
//! from compared to fed: a `Loop` was already an opaque `Runnable` at Tasks
//! 3-5, so its terminal was already supplied — and it also carried a
//! `SUBMITTED` Kestra never emits for a flowable, so the relabelling **removed
//! 40 wrong emissions**
//! ([`planning_the_loop_removed_forty_spurious_submitted_emissions`], which
//! validates itself by reproducing all four of Task 5's committed figures). And
//! the aggregation rule is **not** "judged by nothing": it is invisible to the
//! RATCHET, and two independent wrong-aggregation constructions each die to
//! [`the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank`]
//! and to
//! [`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`].
//! That first test's `33` is therefore a **guard**, not only a report — it falls
//! when a loop's terminal stops matching Kestra's.
//! `valids/loop-failed-no-transmit` — a loop whose three iterations all FAILED
//! and which Kestra ended `SUCCESS` — is still reproduced by feeding rather
//! than by deciding, and
//! [`the_feed_absorbs_a_measured_share_of_the_golden_run_terminals`] counts
//! these 55 runs in a bucket of their own so they can never be read as
//! compared.
//!
//! **Supplied, but NOT from the golden (Task 5, widened by Task 6):** the
//! rendered value of an `If`'s condition, a `Switch`'s value or a `Loop`'s
//! `values`. `advance()` is pure and cannot
//! render, so a caller has to; this one renders the flow's **own expression**
//! with egret's **own** W2 renderer over the inputs `corpus.json` records
//! `harvest.py` as having posted for the very execution the golden was
//! captured from — see [`render_context`]. Nothing about which branch Kestra
//! took is read, so the branch decision stays compared rather than fed. What it
//! does add is a dependency on W2 being right — **in one direction only, and
//! that is measured rather than assumed.** A renderer answering `"false"` where
//! Kestra answered `"true"` moves an entry here and the ratchet reports it. The
//! converse does **not**: no banked entry takes an `else` branch at all, so a
//! predicate that always answers truthy leaves `executor_matches_golden`
//! passing. See [`the_caller_renders_and_a_failed_render_fails_the_run`], whose
//! falsy arm is the only assertion in this binary that dies to *every* wrong
//! predicate. A render that FAILS fails the run, which is what Kestra does and
//! is what banks `valids/if-condition-fail` and `valids/variables-invalid`.
//!
//! **Not fed, and therefore compared:** which runs are created, in what order,
//! under which parent, with what `value`; every state in a run's history
//! *except* its last; the whole of a **flowable's** history including its last;
//! the execution's own history; the happens-before relation. **The
//! sub-executions are neither fed nor compared** — egret's side of `spawned` is
//! empty, which is not the same thing as agreeing; see
//! [`the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank`].
//! What IS compared about them is the first batch of spawn *requests*:
//! [`every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`].
//!
//! Feeding a *flowable's* state would be circular in the way that matters: a
//! `Sequential`, `Parallel`, `Dag`, `If` or `Switch` run's state is the
//! executor's decision and is exactly what this oracle exists to judge. A
//! `Loop`'s, in W3a, is not — see the correction above.
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
use egret_workflow::models::InputType;
use egret_workflow::{render_str, RenderContext};
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
    /// **Why removing is equivalent to accepting, and not to weakening.** egret's
    /// relation domain is measured at `{before, overlapping}` — 501 pairs, zero
    /// `after`. A pair is dropped from the golden precisely because Kestra
    /// flipped on it between rounds, which over a two-valued domain means Kestra
    /// was observed producing *both* values. So "remove the pair" is exactly
    /// "accept either relation Kestra was seen to produce", and "egret must not
    /// produce one" is not a coherent alternative, because egret emits a
    /// relation for every pair by construction.
    ///
    /// **The honest cost, measured on a banked entry.** On
    /// `valids/parallel-nested` egret may report *anything* on those 3 pairs and
    /// the entry still matches — corrupting its relation on exactly them leaves
    /// the comparison equal. That is what "the oracle has no opinion here"
    /// means, stated as a cost rather than implied.
    ///
    /// It is worth 1 banked entry on its own: `valids/parallel-nested`'s runs,
    /// parents, histories and attempts are identical to Kestra's, and every one
    /// of the 52 pairs the golden keeps is reproduced exactly; the only
    /// difference was the 3 it dropped.
    /// [`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`]
    /// asserts what this actually removes, and
    /// [`the_golden_has_no_relation_for_exactly_these_pairs`] the golden-side
    /// population, so neither can quietly grow.
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
/// Derived, not picked, and **re-derived whenever a tier arrives**: the
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
/// - **tier 2** (Task 5, the caller's render; a third kind, `Loop`, at Task 6)
///   either writes a history entry —
///   the failed-render arm applies a `SetRunState` — or writes none and sets
///   `rendered` on at least one run. `rendered` is written once per run and
///   never cleared, so the second case is bounded by `state.runs.len()`, which
///   is bounded by the history entries the trace holds (every run has one).
/// - **tier 3** writes exactly one history entry (plus one attempt entry). So
///   tier-3 steps ≤ history entries written.
/// - **tier 4** writes **none** — it is the one iteration that consumes a step
///   for nothing without making progress on a run. It is still bounded: each
///   jump strictly increases `now` to a distinct `not_before`, and every
///   `not_before` was written by a `ScheduleRetry`, which is itself a tier-1
///   command. So tier-4 steps ≤ tier-1 commands.
///
/// Total steps ≤ 4 × (history entries the finished trace holds). The
/// widest golden entry holds **765** of them across its execution, its runs,
/// their attempts and its sub-executions — `valids/loop-nested`, whose 39
/// sub-executions are most of that. Measured over all of `golden.json`. The
/// factor of 4 covers the four above exactly, and the headroom for an
/// implementation that takes more rounds than Kestra's did comes from the
/// bounds themselves being loose (a tier-1 step usually applies several
/// commands, and tier 2 renders every unrendered run in one step).
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

/// The first run something OUTSIDE this simulation is still holding: a
/// `Runnable` whose latest state is `Running`, or -- since Task 6 -- a `Loop`.
///
/// **A `Loop` is here for the same reason a `Runnable` is**, and this is the one
/// place that claim has to be true rather than merely stated: what a `Loop`
/// waits for is its sub-executions, and this oracle runs none, so its terminal
/// cannot be `advance()`'s decision in W3a. It is not a flowable exception
/// carved out for convenience -- a `Sequential`, `Parallel`, `Dag`, `If` or
/// `Switch` waits only on runs this simulation *does* complete, and feeding one
/// of those would be circular. See this module's feed section for the cost.
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
            Some(TaskKind::Runnable { .. }) | Some(TaskKind::Loop { .. })
        ) && r.history.last() == Some(&RunState::Running)
    })
}

/// Run the state machine to a fixed point and build the trace it produced.
///
/// See this module's doc for exactly what this feeds in from the golden, what
/// that costs, and why it is not circular with respect to `advance()`.
///
/// # The environment has four tiers, in this order
///
/// 1. **The state machine decides.** If it emits commands, they are applied and
///    it is asked again. Nothing else happens while it still has work.
/// 2. **The caller renders.** Only when tier 1 returns nothing: every planned
///    `If`/`Switch`/`Loop` run that is at its own render state (see
///    [`renders_at`] -- `RUNNING` for the first two, `CREATED` for a `Loop`)
///    and unrendered gets the value `render` gives for its expression, written
///    into `TaskRun::rendered`. A render that fails FAILS that run -- measured
///    on `valids/if-condition-fail`, whose condition names an input the flow
///    does not declare and whose golden is one run, `CREATED RUNNING FAILED`.
/// 3. **The outside world finishes ONE thing.** Only when tiers 1 and 2 have
///    nothing to do: the earliest-created `Runnable` -- or, since Task 6,
///    `Loop` -- still `Running` (see [`in_flight`]) reaches the terminal state
///    the golden recorded for its current attempt. A `Loop` is here because
///    what it waits on is its sub-executions, and this oracle runs none.
/// 4. **Time passes.** Only when nothing else can happen: `now` jumps to the
///    earliest `not_before` still ahead of it. Time never moves while anything
///    else can happen, so a `timeout` cannot fire early and a `ScheduleRetry`
///    cannot deadlock the simulation.
///
/// **Tier 2 is before the worker, and ALL of the runs rather than one, and
/// this oracle checks both — which a first version of this comment got wrong.**
/// Kestra renders in the *executor* (`If.isTrue` calls `runContext.render`)
/// rather than in a worker, and its executor evaluates the whole execution in
/// one cycle, so both choices follow the artifact. Both variants were then
/// built and run over the whole golden, and the measured result is:
///
/// | variant | the ratchet | the oracle binary |
/// |---|---|---|
/// | render one run per step | **unmoved**, 181 mismatching | [`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`] **FAILS** |
/// | let the worker move first | **unmoved**, 181 mismatching | the same test **FAILS** |
///
/// **Both rows are Task 5's measurement on Task 5's tree and were not
/// re-derived at Task 6**, which gave this tier a third kind and a second render
/// state. The 181 is that task's baseline; it is history, not current
/// behaviour.
///
/// In both, `valids/if` joins that test's `runs_differ` list: its run **order**
/// stops matching Kestra's `taskRunList`. So the shipped shape is the one that
/// reproduces Kestra's creation order and a committed test says so. Neither
/// choice is free, and the earlier "the batching is free" was an artefact of
/// checking the ratchet's count and not the binary.
///
/// **What the caller can render is deliberately narrow** — see
/// [`render_context`]: `inputs` from `corpus.json` and `vars` from the flow.
/// An expression over a task output, a loop `item` or the KV store fails, and a
/// failed render is a `FAILED` run rather than a guessed branch.
///
/// Then, and only then, the run is **settled** — or **stuck**, which is a
/// different answer and is reported as one. See the end of the loop.
///
/// **The worker tier is the reason for this shape, and it was an accident of
/// loop order before.** The environment used to complete every `Running` task at the top of
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
/// that tiers 2, 3 and 4, the clock rule, the stuck check and the happens-before
/// endpoints are pinned by committed assertions instead of being merely current.
/// They were not: replacing six sites in this function with unconditional
/// panics — tier 1's application body, the worker tier, the clock scan,
/// [`in_flight`]'s
/// closure, [`step_relation`]'s body and the `order` assembly — left the whole
/// suite reporting `8 passed; 0 failed`, because with `advance()` a stub nothing
/// committed ever reached the second iteration.
fn simulate_with(
    plan: &Plan,
    golden: &Trace,
    render: &dyn Fn(&str) -> Result<String, String>,
    state_machine: impl FnMut(&Plan, &ExecutionState, Tick) -> Vec<Command>,
) -> Result<Trace, String> {
    simulate_recording_spawns(plan, golden, render, state_machine).map(|(trace, _)| trace)
}

/// One [`Command::SpawnExecution`] the state machine asked for, in emission
/// order.
///
/// It is returned **beside** the [`Trace`] rather than inside it, and that is
/// the point: a spawn request is not a sub-execution. `Trace::spawned` holds
/// the sub-execution traces the golden carries and egret's side of it stays
/// empty until W3b runs one, so putting a request there would make an entry
/// look compared on a field nothing produces. What IS compared is here, by
/// `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    /// The **`Loop`'s** own index, not a body task's — see
    /// [`Command::SpawnExecution`] for why a single body index could not name
    /// what a sub-execution runs.
    task: usize,
    parent_run: RunIx,
    value: String,
}

/// [`simulate_with`], also handing back the spawns it asked for.
fn simulate_recording_spawns(
    plan: &Plan,
    golden: &Trace,
    render: &dyn Fn(&str) -> Result<String, String>,
    mut state_machine: impl FnMut(&Plan, &ExecutionState, Tick) -> Vec<Command>,
) -> Result<(Trace, Vec<Spawn>), String> {
    let mut spawns: Vec<Spawn> = Vec::new();
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
                    Command::SpawnExecution {
                        task,
                        value,
                        parent_run,
                    } => spawns.push(Spawn {
                        task: *task,
                        parent_run: *parent_run,
                        value: value.clone(),
                    }),
                    _ => {}
                }
                apply(&mut state, cmd);
            }
            step += 1;
            continue;
        }

        // Tier 2: the caller renders. Every planned `If`/`Switch`/`Loop` run
        // that is at its own render state and unrendered, in creation order, in
        // ONE step -- see this function's doc for why all of them rather than
        // one and why this comes before the worker, and `renders_at` for why a
        // `Loop` is asked at CREATED and the other two at RUNNING.
        let unrendered: Vec<RunIx> = (0..state.runs.len())
            .filter(|&ix| {
                expression_of(plan, &state, ix).is_some()
                    && state.runs[ix].history.last() == Some(&renders_at(plan, state.runs[ix].task))
                    && state.runs[ix].rendered.is_none()
            })
            .collect();
        if !unrendered.is_empty() {
            for ix in unrendered {
                let source = expression_of(plan, &state, ix).expect("filtered on Some above");
                match render(&source) {
                    Ok(value) => state.runs[ix].rendered = Some(value),
                    // A render that fails FAILS the run, and that is measured
                    // rather than chosen: `valids/if-condition-fail`'s condition
                    // is `{{ inputs.run }}` on a flow with no such input, and
                    // Kestra's golden for it is one run, `if`, `CREATED RUNNING
                    // FAILED`, with the attempt `FAILED` too.
                    Err(_) => {
                        apply(
                            &mut state,
                            &Command::SetRunState {
                                run: ix,
                                state: RunState::Failed,
                            },
                        );
                        ended[ix] = Some(step);
                    }
                }
            }
            step += 1;
            continue;
        }

        // Tier 3.
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

        // Tier 4.
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
        // flowable left `Running` forever is invisible to tier 3, which only
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

    Ok((
        Trace {
            parent_run: None,
            value: None,
            execution: state.history.iter().map(|s| s.wire().to_string()).collect(),
            runs,
            order,
            // **Still empty at Task 6, and deliberately.** `advance()` now emits
            // `Command::SpawnExecution`, but nothing executes one until W3b/W6,
            // so there is no sub-execution trace to put here and inventing a
            // stub would make 41 entries look compared on a field that is not.
            // The requests are returned beside this instead. See
            // `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank`
            // and `oracle-exec/README.md`.
            spawned: Vec::new(),
        },
        spawns,
    ))
}

/// [`simulate_with`], driven by the real [`advance`].
fn simulate(
    plan: &Plan,
    golden: &Trace,
    render: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Trace, String> {
    simulate_with(plan, golden, render, advance)
}

/// [`simulate`], also handing back the spawns the real [`advance`] asked for.
fn simulate_spawns(
    plan: &Plan,
    golden: &Trace,
    render: &dyn Fn(&str) -> Result<String, String>,
) -> Result<(Trace, Vec<Spawn>), String> {
    simulate_recording_spawns(plan, golden, render, advance)
}

/// A renderer for a plan that has nothing to render.
///
/// It refuses everything, so a flow that *does* contain an `If` or a `Switch`
/// fails that run rather than quietly taking a branch nobody chose. Every
/// caller of this passes a [`probe_plan`], which is `Runnable`s only.
fn no_render(_: &str) -> Result<String, String> {
    Err("this probe plan has nothing to render".to_string())
}

/// The unrendered expression of the task run `ix` is a run of, if that task is
/// one of the two flowables that has one.
///
/// `None` for every other kind, which is what the tier-2 filter means by "this
/// run does not need a render".
fn expression_of(plan: &Plan, state: &ExecutionState, ix: RunIx) -> Option<String> {
    expression_of_task(plan, state.runs[ix].task)
}

/// The unrendered expression a planned task holds, by [`TaskKind`].
///
/// Three kinds hold one: an `If`'s `condition`, a `Switch`'s `value` and a
/// `Loop`'s `values`. Task 6 added the third, and it is not a fourth tier —
/// the same tier renders it, at a different run state (see [`renders_at`]).
fn expression_of_task(plan: &Plan, task: usize) -> Option<String> {
    match plan.tasks.get(task).map(|t| &t.kind) {
        Some(TaskKind::If { condition, .. }) => Some(condition.clone()),
        Some(TaskKind::Switch { value, .. }) => Some(value.clone()),
        Some(TaskKind::Loop { values, .. }) => Some(values.clone()),
        _ => None,
    }
}

/// The run state at which this task's expression is rendered, measured.
///
/// **`CREATED` for a `Loop`, `RUNNING` for an `If` or a `Switch`.** An `If` is
/// started and then asked; a `Loop` is asked and then started, or not started
/// at all: `valids/loop-empty`'s `1_each` reads `CREATED SUCCESS` with no
/// `RUNNING` and no attempt, and `valids/loop-null`'s reads `CREATED FAILED`,
/// so on this artifact a `Loop` resolves its values before it runs. Rendering
/// a `Loop` at `RUNNING` instead would make both of those traces
/// unreachable — and `executor::flowable::loop_::open` is where the same rule
/// lives on the other side of the seam.
fn renders_at(plan: &Plan, task: usize) -> RunState {
    match plan.tasks.get(task).map(|t| &t.kind) {
        Some(TaskKind::Loop { .. }) => RunState::Created,
        _ => RunState::Running,
    }
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

/// The whole of `corpus.json`, read once.
///
/// The ratchet needs one field of it — the `inputs` `harvest.py` posted when it
/// executed the flow — because the caller cannot render `{{ inputs.param }}`
/// without them, and a golden captured with `param: true` is a trace of the
/// `then` branch. Reading the values from the same file that says which flows
/// ran keeps the render and the golden on one provenance.
fn corpus_json() -> &'static BTreeMap<String, serde_json::Value> {
    static CORPUS: std::sync::OnceLock<BTreeMap<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    CORPUS.get_or_init(|| {
        let raw = std::fs::read_to_string(format!("{ORACLE}/corpus.json"))
            .unwrap_or_else(|e| panic!("cannot read {ORACLE}/corpus.json: {e}"));
        let entries: Vec<serde_json::Value> =
            serde_json::from_str(&raw).expect("corpus.json is a list of objects");
        entries
            .into_iter()
            .map(|e| {
                (
                    e["id"]
                        .as_str()
                        .expect("corpus entry has an id")
                        .to_string(),
                    e,
                )
            })
            .collect()
    })
}

/// The caller's render context for one corpus entry: `inputs` and `vars`, and
/// deliberately nothing else.
///
/// **This is the whole of what the oracle's caller can render**, and the limit
/// is the honest one rather than a convenience. Kestra renders through a
/// `RunContext` that also reaches task outputs, the loop `item`, `execution`,
/// `taskrun`, `trigger`, `labels` and the KV store; an expression over any of
/// those fails to render here and the run is `FAILED`, which is visible in the
/// trace rather than silently taking a branch. `oracle-exec/README.md` names
/// the entries that costs.
///
/// `inputs` comes from `corpus.json` — the values `harvest.py` posted for the
/// very execution the golden was captured from — and `vars` from the flow's own
/// `variables:` block.
///
/// **One further divergence, recorded rather than fixed**: an input whose value
/// is present but `null` renders as `""` here, which `is_truthy` reads as
/// falsy, where Kestra's renderer would refuse the expression and FAIL the run.
/// No corpus flow posts a null input value, so nothing compared reaches it.
fn render_context(corpus_id: &str, source: &str) -> RenderContext {
    let flow = egret_workflow::spec::parse(source).ok();

    let mut inputs = match corpus_json()
        .get(corpus_id)
        .and_then(|e| e.get("inputs"))
        .cloned()
    {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    // The flow's own declared DEFAULTS, for every input the harvest did not
    // post. Kestra applies them before it renders anything, so leaving them out
    // made SEVEN planned `If` conditions across FIVE entries fail to render on
    // flows whose input was never missing -- `valids/exit` posts `inputs: {}`
    // and declares `state` with `defaults: END`.
    //
    // The source text is parsed as JSON for the types where Kestra's coercion
    // would produce a non-string, so a `BOOL` input's `defaults: true` arrives
    // as a boolean and not as `"true"` -- `{{ inputs.failed == false }}` is in
    // six corpus flows and compares against a boolean. **It is keyed on the
    // declared `type:` and not on whether the text happens to parse**, which
    // round 0 got wrong: a `STRING` or `SELECT` default written `true`, `123`
    // or `null` would silently have become a bool, a number or a null. Zero
    // corpus flows are that shape today, and Tasks 6-8 widen this population
    // from 26 expressions to about 42.
    if let Some(f) = &flow {
        for input in &f.inputs {
            if inputs.contains_key(&input.id) {
                continue;
            }
            let Some(default) = &input.defaults else {
                continue;
            };
            let raw = default.source();
            let typed = match input.r#type {
                InputType::Int
                | InputType::Float
                | InputType::Boolean
                | InputType::Json
                | InputType::Array
                | InputType::Multiselect => serde_json::from_str(raw).ok(),
                _ => None,
            };
            inputs.insert(
                input.id.clone(),
                typed.unwrap_or_else(|| serde_json::Value::String(raw.to_string())),
            );
        }
    }

    let mut cx = RenderContext::new();
    cx.insert("inputs", serde_json::Value::Object(inputs));
    cx.insert(
        "vars",
        flow.and_then(|f| serde_json::to_value(f.variables).ok())
            .unwrap_or(serde_json::Value::Null),
    );
    cx
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
            Ok(plan) => {
                let cx = render_context(&id, &source);
                let render = move |expr: &str| render_str(expr, &cx).map_err(|e| e.to_string());
                simulate(&plan, &golden, &render).map(Outcome::Trace)
            }
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

    let eager =
        simulate_with(&plan, &golden, &no_render, eager_machine).expect("eager must settle");
    let lazy = simulate_with(&plan, &golden, &no_render, lazy_machine).expect("lazy must settle");

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
    let eager = simulate_with(&plan, &golden_feed, &no_render, eager_machine).expect("settles");
    let lazy = simulate_with(&plan, &golden_feed, &no_render, lazy_machine).expect("settles");
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

/// **The pairs the golden has no relation for: 13, across 3 entries.**
///
/// Derived from `golden.json` on every run, never listed. It is the same 13
/// `normalise.py` reports as dropped-for-flipping across the whole harvest, in
/// the same three entries — `oracle-exec/stability.json` names them, and all
/// three flows reached `golden.json`, so there is nothing to reconcile.
///
/// **This is the golden-side figure and it is NOT the bound on what
/// [`Trace::restrict_order_to`] removes** — an earlier version of this doc
/// claimed it was, and that claim was false in two ways at once: the number was
/// written down as 12 beside a map that sums to 13, and the test never called
/// the projection at all. What the projection actually removes is measured by
/// [`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`].
#[test]
fn the_golden_has_no_relation_for_exactly_these_pairs() {
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
        "the whole population of pairs the golden declines to assert -- 13, which \
         is exactly the 13 `stability.json` records as dropped-for-flipping, in \
         the same three flows"
    );
    assert_eq!(dropped.values().sum::<usize>(), 13, "and they sum to 13");
}

/// **What the restriction actually removes, driven through the real
/// `advance()`: 22 pairs across 6 entries — and egret never says `after`.**
///
/// The bound the test above was wrongly claimed to be. It calls
/// [`Trace::restrict_order_to`] on every comparable entry, exactly as
/// [`executor_matches_golden`] does, and asserts the per-entry map. The number
/// the ratchet prints is this one, so a widening of the restriction's reach
/// moves an assertion rather than a log line.
///
/// **22 and not 13, and the gap is not a hole.** Three entries are the golden's
/// own dropped pairs, on traces whose runs line up. The other three —
/// `disable-simple`, `parallel-disabled-tasks`, `sequential-with-disabled`, all
/// `disabled:` flows no W3a task claims — produce a *different run set* from
/// Kestra's, so their pair indices do not denote the same pairs at all and the
/// projection removes whatever does not collide. Such an entry can never bank:
/// its `runs` differ, which is compared first and verbatim. The test asserts
/// that separation rather than asserting it away, so an entry that starts
/// having pairs removed while its runs DO match shows up as a changed map.
///
/// **And the `after` assertion is what makes the restriction equivalent to the
/// stronger rule.** egret's relation domain is measured, not assumed: over all
/// comparable entries it produces 501 ordered pairs and **zero** `after`. With a
/// two-valued domain, a pair Kestra dropped is a pair Kestra was seen producing
/// *both* values for, so "remove the pair" and "accept either relation Kestra
/// was observed to produce" are the same rule. That equivalence is why the
/// restriction is not a weakening — and it holds only while `after` stays
/// impossible, which is a property of creation indices being monotonic in
/// `taskRunList` order. Task 6's spawned executions were named as the thing
/// that could break it; they do not, because a `Loop` creates no run for its
/// body at all. It is asserted here rather than argued in a comment, which is
/// what let that prediction be checked rather than repeated.
#[test]
fn what_the_order_restriction_removes_is_derived_and_egret_never_says_after() {
    let mut removed: BTreeMap<String, usize> = BTreeMap::new();
    let mut runs_differ: Vec<String> = Vec::new();
    let mut total_pairs = 0usize;
    let mut afters: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let expected = Trace::from(entry);
        let Outcome::Trace(mut actual) = evaluate(entry) else {
            continue;
        };
        total_pairs += actual.order.len();
        if actual.order.iter().any(|(_, _, rel)| rel == "after") {
            afters.push(entry.id.clone());
        }
        let same_runs = actual
            .runs
            .iter()
            .map(|r| &r.task)
            .eq(expected.runs.iter().map(|r| &r.task));
        let dropped = actual.restrict_order_to(&expected);
        if dropped > 0 {
            removed.insert(entry.id.clone(), dropped);
            if !same_runs {
                runs_differ.push(entry.id.clone());
            }
        }
    }

    println!("restriction removed {removed:?}; egret produced {total_pairs} ordered pair(s)");
    assert_eq!(
        removed,
        BTreeMap::from([
            ("valids/dag".to_string(), 1),
            ("valids/disable-simple".to_string(), 2),
            ("valids/if".to_string(), 9),
            ("valids/if-with-only-disabled-tasks".to_string(), 1),
            ("valids/parallel-disabled-tasks".to_string(), 7),
            ("valids/parallel-nested".to_string(), 3),
            ("valids/sequential-with-disabled".to_string(), 2),
        ]),
        "this is the number the ratchet prints; assert it so a widening of the \
         restriction's reach moves a test and not a log line"
    );
    assert_eq!(removed.values().sum::<usize>(), 25);
    assert_eq!(
        runs_differ,
        [
            "valids/disable-simple",
            "valids/if-with-only-disabled-tasks",
            "valids/parallel-disabled-tasks",
            "valids/sequential-with-disabled"
        ],
        "every entry beyond the golden's own 3 dropped-pair flows has a run set \
         that DIFFERS from Kestra's, so its pair indices denote different pairs \
         and it can never bank -- `runs` is compared first and verbatim. If an \
         entry whose runs MATCH starts losing pairs, this list is what says so"
    );

    assert_eq!(
        afters,
        Vec::<String>::new(),
        "egret's relation domain must stay {{before, overlapping}}: a pair Kestra \
         dropped is one it was seen producing BOTH values for, so dropping it \
         equals accepting either -- and that equivalence needs `after` to be \
         unreachable. It is unreachable because a run's left endpoint is its \
         creation step and creation is monotonic in run order. **Task 6 was \
         named here as what could break it, and it does not**: a `Loop` creates \
         no run for its body at all -- each value is a separate sub-execution -- \
         so there is no new run whose creation could land out of order"
    );
    assert_eq!(
        total_pairs, 555,
        "the population the `after` claim is measured over. 557 at Task 5; the \
         two lost are `valids/loop-empty`'s and `valids/loop-null`'s, which both \
         banked -- `loop-null`'s `Loop` FAILS at `CREATED`, so its second root \
         task is never created and the flow has one run and no pair at all"
    );

    // The cost, demonstrated on a BANKED entry rather than conceded in prose:
    // on the 3 pairs the golden dropped, `valids/parallel-nested` may report
    // anything at all and still match.
    let entry = golden()
        .into_iter()
        .find(|e| e.id == "valids/parallel-nested")
        .expect("a banked entry with dropped pairs");
    let expected = Trace::from(&entry);
    let Outcome::Trace(mut corrupted) = evaluate(&entry) else {
        panic!("valids/parallel-nested produces a trace")
    };
    let keep: BTreeSet<(usize, usize)> = expected.order.iter().map(|(i, j, _)| (*i, *j)).collect();
    let mut corrupted_any = false;
    for (i, j, rel) in &mut corrupted.order {
        if !keep.contains(&(*i, *j)) {
            *rel = "after".to_string();
            corrupted_any = true;
        }
    }
    assert!(
        corrupted_any,
        "there are 3 such pairs; if none, this proves nothing"
    );
    corrupted.restrict_order_to(&expected);
    assert_eq!(
        corrupted, expected,
        "THE COST: on a pair the golden dropped, egret may report anything -- here \
         the impossible `after` -- and a banked entry still matches. That is what \
         `the oracle has no opinion here` buys and what it costs"
    );
}

/// **Tier 3: the environment finishes an in-flight task, and the state machine
/// is asked again while one is in flight.**
///
/// Both halves are asserted. Deleting tier 3 leaves the run `Running` for ever
/// and the simulation reports STUCK; deleting the `saw_in_flight` half would
/// let the old top-of-loop order pass, which is the defect this replaced.
#[test]
fn the_worker_finishes_one_task_and_the_machine_is_asked_while_it_is_in_flight() {
    let plan = probe_plan(&["a"]);
    let golden = probe_golden(&["a"]);
    let mut saw_in_flight = false;

    let trace = simulate_with(&plan, &golden, &no_render, |p, st, now| {
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

/// **Tier 2: the caller renders, and a render that FAILS fails the run.**
///
/// Tier 2 is new in Task 5 and is exactly the kind of environment change that
/// has to carry its own guard: `advance()` is pure and cannot render, so
/// without a caller that supplies `TaskRun::rendered` an `If` waits for ever
/// and the simulation reports `STUCK`. All three arms below turn into `STUCK`
/// if the tier is deleted, which is what makes this the deletion guard and not
/// only a wiring test.
///
/// The falsy arm uses `"0"` rather than `"false"` on purpose: it is the value
/// that separates Kestra's truthiness from every predicate a Rust reader would
/// reach for, so the harness pins the same rule the unit tests do.
///
/// The failing arm is measured rather than invented. `valids/if-condition-fail`
/// renders `{{ inputs.run }}` on a flow that declares no such input, and
/// Kestra's golden for it is one run, `CREATED RUNNING FAILED`, with the
/// attempt `FAILED` too. That entry is banked by this arm.
///
/// **What pins the tier's SHAPE is a different test, measured.** Two variants —
/// rendering one run per step instead of all of them, and letting the worker
/// move before the render — leave **the ratchet unmoved at 181**, so they are
/// invisible *here*. Neither is invisible to the oracle **binary**: both fail
/// [`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`]
/// by putting `valids/if` into its `runs_differ` list, because its run order
/// stops matching Kestra's. Reading the ratchet's count and calling the
/// batching "free" is the mistake a review caught here.
///
/// **And this test's falsy arm is load-bearing beyond its own name.** No banked
/// entry takes an `else` branch at all — measured: `chosen = then_`
/// unconditionally leaves `executor_matches_golden` passing — so the `"0"` arm
/// below is the only assertion in this binary that dies to *every* wrong
/// truthiness predicate, including `parse::<bool>().unwrap_or(true)`, which
/// even `an_if_takes_then_when_true_and_else_when_false` survives.
#[test]
fn the_caller_renders_and_a_failed_render_fails_the_run() {
    let source = "id: x\nnamespace: io.kestra.tests\ntasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{ inputs.p }}\"\n    then:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n    else:\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n";
    let plan = build_plan(source).expect("plans");
    let golden = probe_golden(&["if", "a", "b"]);

    let truthy = simulate_with(&plan, &golden, &|_| Ok("true".to_string()), advance)
        .expect("a rendered If must settle -- STUCK here means the render tier is gone");
    assert_eq!(
        truthy
            .runs
            .iter()
            .map(|r| r.task.as_str())
            .collect::<Vec<_>>(),
        ["if", "a"]
    );

    let falsy = simulate_with(&plan, &golden, &|_| Ok("0".to_string()), advance)
        .expect("a rendered If must settle");
    assert_eq!(
        falsy
            .runs
            .iter()
            .map(|r| r.task.as_str())
            .collect::<Vec<_>>(),
        ["if", "b"],
        "`0` is FALSY in Kestra -- measured live, and the whole reason the \
         predicate is not `parse::<bool>()`"
    );

    let broken = simulate_with(
        &plan,
        &golden,
        &|_| Err("Missing variable: inputs.p".to_string()),
        advance,
    )
    .expect("a failed render is a FAILED run, not a stuck simulation");
    assert_eq!(
        broken
            .runs
            .iter()
            .map(|r| r.task.as_str())
            .collect::<Vec<_>>(),
        ["if"],
        "the render failed before any branch was chosen, so no child exists"
    );
    assert_eq!(broken.runs[0].state, ["CREATED", "RUNNING", "FAILED"]);
    assert_eq!(
        broken.runs[0].attempts,
        vec![vec![
            "CREATED".to_string(),
            "RUNNING".to_string(),
            "FAILED".to_string()
        ]],
        "the attempt carries the failure too, which is what \
         `valids/if-condition-fail`'s golden records"
    );
    assert_eq!(broken.execution, ["CREATED", "RUNNING", "FAILED"]);
}

/// **Tier 4: time moves only to a deadline something is waiting for.**
///
/// The machine refuses to start its run until `now` has reached the
/// `not_before` it scheduled. Without tier 4 the clock never leaves `Tick(0)`,
/// the machine never acts, and the simulation reports STUCK — so a retry could
/// never be reproduced by any implementation.
#[test]
fn time_moves_only_to_a_deadline_something_is_waiting_for() {
    let plan = probe_plan(&["a"]);
    let golden = probe_golden(&["a"]);
    const DUE: Tick = Tick(7);
    let mut seen: Vec<Tick> = Vec::new();

    let trace = simulate_with(&plan, &golden, &no_render, |p, st, now| {
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
            // Waiting. Only tier 4 can get us out of here.
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
/// A run left `Created` is invisible to tier 3, which only completes
/// `Runnable`s that are `Running`; it reaches tier 4, finds no deadline, and
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

    let err = simulate_with(&plan, &golden, &no_render, |p, st, _| {
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

    let idle = simulate_with(&plan, &golden, &no_render, |_, _, _| Vec::new())
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
/// Four buckets, and every run terminal is in exactly one:
///
/// - **fed** — the run's task is a `Runnable` in the plan, so its terminal is
///   supplied and cannot be compared.
/// - **spawner** — the run's task is a `Loop`. It is planned and driven here,
///   and its terminal is **fed anyway**, because it is a function of
///   sub-executions this oracle does not run. Counted apart from `fed` so the
///   two shapes of feed stay distinguishable, and apart from `flowable` so
///   nothing reads a `Loop`'s state as compared. Task 6's, and its 55 runs are
///   the price of the sub-execution hole.
/// - **flowable** — the run's task is one this executor drives itself AND
///   decides for itself (`Sequential`, `Parallel`, `Dag`, `If`, `Switch`),
///   whose state is compared.
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
    // Task 6's fourth bucket: a `Loop` is planned and driven here, and its
    // terminal is fed anyway, because it is a function of sub-executions this
    // oracle does not run. It is counted apart from `fed` so that the
    // worker-shaped feed and the spawner-shaped one cannot be confused, and
    // apart from `flowable` so that nothing reads a Loop's state as compared.
    let mut spawner = 0usize;
    // Of the fed and spawner runs, how many live inside a SUB-EXECUTION? egret
    // simulates none of those, so their terminals are neither supplied nor
    // compared -- they are a classification of Kestra's runs by egret's plan and
    // nothing more. Counting them apart is what keeps the bucket's own
    // definition true of every run it holds.
    let (mut fed_in_a_sub, mut spawner_in_a_sub) = (0usize, 0usize);
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
        let mut stack: Vec<(&Trace, bool)> = vec![(&trace, true)];
        while let Some((t, top_level)) = stack.pop() {
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
                        | TaskKind::Dag { .. }
                        | TaskKind::If { .. }
                        | TaskKind::Switch { .. },
                    ) => flowable += 1,
                    Some(TaskKind::Loop { .. }) => {
                        spawner += 1;
                        if !top_level {
                            spawner_in_a_sub += 1;
                        }
                    }
                    Some(TaskKind::Runnable { type_id }) => {
                        fed += 1;
                        if !top_level {
                            fed_in_a_sub += 1;
                        }
                        if !run.state.iter().any(|s| s == "SUBMITTED") {
                            fed_without_submitted += 1;
                            *over_fed.entry(type_id.clone()).or_default() += 1;
                        }
                    }
                }
            }
            stack.extend(t.spawned.iter().map(|sub| (sub, false)));
        }
    }

    let total = fed + spawner + flowable + unplanned;
    println!(
        "golden run terminals: {total} total = {fed} fed + {spawner} spawner (fed too) + \
         {flowable} flowable + \
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
    // The bucket names are true of the TOP-LEVEL runs and of no other, and that
    // is stated as a number rather than left to a reader. `fed` says "its
    // terminal is supplied"; for a run inside a sub-execution nothing is
    // supplied, because egret never creates the run at all.
    println!(
        "of the fed, {fed_in_a_sub} live inside a sub-execution egret never \
         simulates; of the spawner, {spawner_in_a_sub}"
    );
    assert_eq!(
        (fed_in_a_sub, spawner_in_a_sub),
        (189, 15),
        "189 of the fed runs and 15 of the spawner runs are inside a \
         sub-execution. Nothing is fed to them and nothing is compared about \
         them -- egret produces no sub-execution -- so `oracle-exec/README.md`'s \
         bucket descriptions hold for the other {} and {}, and this pair is what \
         says so",
        fed - fed_in_a_sub,
        spawner - spawner_in_a_sub
    );
    assert_eq!(
        (fed, spawner, flowable, unplanned, fed_without_submitted),
        (565, 55, 108, 112, 79),
        "these five numbers are written into oracle-exec/README.md under `What the \
         oracle feeds in`. They move as Tasks 4-8 teach `build_plan` more flowables, \
         and when they do the README has to move with them -- which is the point of \
         asserting them here rather than quoting them there. Task 6 moved every one \
         of them, and `unplanned` most: 343 -> 112, because planning the `Loop` is \
         what gives every task inside a loop BODY a `PlanTask` at all."
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
    let flow = bucket("io.kestra.plugin.core.flow.");
    let execution = bucket("io.kestra.plugin.core.execution.");
    assert_eq!(
        (over_fed.len(), flow, execution),
        (11, (6, 54), (4, 23)),
        "11 task types are over-submitted, down from Task 5's 12: `core.flow.Loop` \
         left the set in the commit that gave it a TaskKind, which is the whole \
         point of asserting it. SIX are still flowables, and their 89 runs fell to \
         54 -- planning the `Loop` is what gave the tasks inside every loop BODY a \
         `PlanTask` at all, so `If`, `Switch` and `Sequential` runs inside \
         sub-executions stopped being `unplanned` and became recognised. That \
         movement is much larger than this bucket: `unplanned` went 343 -> 112 in \
         the same commit. It does NOT all go the helpful way -- \
         `core.flow.Subflow` went 22 -> 25 for exactly the reason Task 4 and Task \
         5 each found one level up, `valids/loop-with-subflow`'s `Subflow` being \
         inside a loop body. FOUR are `io.kestra.plugin.core.execution.*` (23 \
         runs), unmoved. The family is Kestra's `ExecutionUpdatableTask` \
         (`Labels`, `SetVariables`, `UnsetVariables`, `Exit`; \
         `core/src/main/java/io/kestra/plugin/core/execution/Labels.java:79`), \
         neither a `RunnableTask` nor a flowable and claimed by NO W3a task -- nor \
         does any task claim `core.flow.Subflow`, `core.flow.Pause`, \
         `core.flow.AllowFailure`, `core.flow.WorkingDirectory`, \
         `core.flow.WaitFor` or `core.flow.LoopUntil`, all 54 of the flow bucket \
         between them. See oracle-exec/README.md, `States and constructs no W3a \
         task claims`."
    );
    // The arithmetic, asserted beside the parts rather than written next to
    // them: this package has now put a total beside a derived map four times
    // and got it wrong once.
    assert_eq!(
        (flow.0 + execution.0 + 1, flow.1 + execution.1 + 2),
        (over_fed.len(), fed_without_submitted),
        "the three buckets -- core.flow.*, core.execution.*, and the single \
         `Log` remainder asserted below -- must account for every over-submitted \
         TYPE and every over-submitted RUN: {} + {} + 1 types, {} + {} + 2 runs",
        flow.0,
        execution.0,
        flow.1,
        execution.1
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

/// **What the caller can render, derived: 24 of the 26 planned branch
/// expressions, and the 2 it cannot are the 2 Kestra could not either.**
///
/// [`render_context`] is deliberately narrow — `inputs` (the values
/// `corpus.json` records `harvest.py` as having posted, plus the flow's own
/// declared defaults) and `vars`, and nothing else — while Kestra's
/// `RunContext` also reaches task outputs, the loop `item`, `execution`,
/// `taskrun`, `trigger` and the KV store. A narrow context is not free: an
/// expression it cannot resolve FAILS the run, which is a wrong trace for a
/// right-looking reason.
///
/// So the cost is derived rather than argued. Over every planned `If` and
/// `Switch` in every comparable entry, exactly **two** expressions fail — and
/// both are ones the **artifact** failed on too: this test reads each failing
/// task's own run out of `golden.json` and requires it to end `FAILED`. A
/// third failure, on a task Kestra ran to `SUCCESS`, would be an entry egret
/// gets wrong because of the harness rather than because of `advance()`, and
/// that is precisely what this is here to surface.
///
/// The `defaults` half is why the number is 2 and not 9. Five entries post
/// `inputs: {}` and declare their input with a `defaults:` — `valids/exit`,
/// `valids/exit-cancelled`, `valids/exit-killed`, `valids/exit-nested` (two
/// `If`s) and `valids/parallel-fail-with-flowable` (two `If`s), **seven**
/// expressions between them. Without the filling step all seven failed as well,
/// for a reason that had nothing to do with the flow; 7 + 2 = 9. And
/// `valids/parallel-fail-with-flowable` — the flow Task 4's whole
/// `fail_the_rest` rule was argued from — is banked *because* of it.
#[test]
fn the_caller_can_render_every_expression_except_the_ones_named() {
    let mut reachable = 0usize;
    let mut in_a_loop_body: Vec<(String, String)> = Vec::new();
    let mut mentions_item = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    // The breakdown, by the two things that moved: `Loop` gained a `values:`
    // expression of its own AND made every expression inside a loop body
    // planned. A single total would hide which of the two did what.
    let (mut branch_reachable, mut branch_in_body) = (0usize, 0usize);
    let (mut values_reachable, mut values_in_body) = (0usize, 0usize);

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        let Ok(plan) = build_plan(&source) else {
            continue;
        };
        let cx = render_context(&entry.id, &source);
        for (ix, task) in plan.tasks.iter().enumerate() {
            let Some(expression) = expression_of_task(&plan, ix) else {
                continue;
            };
            // A loop body's task never gets a run in THIS execution, so this
            // caller never renders its expression -- see the doc above.
            let is_values = matches!(task.kind, TaskKind::Loop { .. });
            if plan.is_loop_body(ix) {
                in_a_loop_body.push((entry.id.clone(), task.id.clone()));
                if is_values {
                    values_in_body += 1;
                } else {
                    branch_in_body += 1;
                }
                if expression.contains("item") {
                    mentions_item += 1;
                }
                continue;
            }
            if is_values {
                values_reachable += 1;
            } else {
                branch_reachable += 1;
            }
            reachable += 1;
            if render_str(&expression, &cx).is_err() {
                failed.push((entry.id.clone(), task.id.clone()));
            }
        }
    }

    println!(
        "expressions: {reachable} reachable ({branch_reachable} branch + \
         {values_reachable} loop values) + {} in a loop body ({branch_in_body} \
         branch + {values_in_body} loop values); {mentions_item} name `item`",
        in_a_loop_body.len()
    );
    assert_eq!(
        (
            branch_reachable,
            values_reachable,
            branch_in_body,
            values_in_body
        ),
        (26, 40, 12, 6),
        "the population: every planned `If` condition, `Switch` value and `Loop` \
         `values:` in the comparable corpus, split two ways -- by kind, and by \
         whether a run for it can exist in THIS execution. Task 5 measured 26 \
         branch expressions and predicted `16 more across 15 entries` once `Loop` \
         landed; **the measured figure is 12, and every one of them is inside a \
         loop body**, so the REACHABLE branch population is still exactly 26 and \
         nothing Task 5 asserted about it moved."
    );
    // The arithmetic, beside the parts rather than written next to them.
    assert_eq!(
        (
            branch_reachable + values_reachable,
            branch_in_body + values_in_body
        ),
        (reachable, in_a_loop_body.len()),
        "26 + 40 reachable and 12 + 6 in a loop body, totalling 84 expressions"
    );
    assert_eq!(
        mentions_item, 10,
        "10 of the 18 loop-body expressions name `item`, NOT all 18 -- the other \
         eight are six nested `Loop` `values:` literals and two `If` conditions \
         over `inputs:` that this context could render perfectly well. That is \
         the measurement behind the ruling: the reason none of the 18 is rendered \
         here is that NO RUN FOR IT EXISTS in this execution, not that the \
         context is too narrow for it. Supplying an invented `item` would change \
         nothing this oracle can see and would put a made-up value behind a \
         branch decision; Kestra renders these inside the sub-execution, with \
         `item` from `loopRun`, and the sub-executions are exactly what W3a does \
         not run."
    );

    // Kestra could not render these two either, read from the golden rather
    // than remembered: a FAILED run is the right answer and both entries are
    // banked BY it.
    let kestras_own: Vec<(String, String)> = [
        ("valids/if-condition-fail", "if"),
        ("valids/variables-invalid", "parent-seq"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
    .collect();
    // And these two are blocked TWICE OVER, which the first version of this test
    // got wrong by naming only the first cause.
    //
    //   1. the harness: `values: "{{ outputs.write.uri }}"` needs a task output,
    //      which `render_context` does not carry and Kestra's `RunContext` does;
    //   2. `advance()` itself: that expression renders to an **ION file URI**,
    //      which is Kestra's THIRD documented `values:` shape (`Loop.java:210-214`)
    //      and is handled by `FlowableUtils.resolveLoopValuesUri` /
    //      `Loop.initFromUri` BEFORE `resolveValues` is reached.
    //      `iteration_values` implements only `resolveValues`'s String arm, so
    //      with a perfect `outputs` context both entries would still fail --
    //      inside the executor, not inside the harness.
    //
    // The second cause is asserted below rather than described, because an
    // attribution committed in a test is the kind that gets believed.
    let blocked_twice_over: Vec<(String, String)> = [
        ("valids/loop-outputs-auto", "loop"),
        ("valids/loop-values-from-uri", "loop"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
    .collect();

    let mut expected: Vec<(String, String)> = kestras_own.to_vec();
    expected.extend(blocked_twice_over.iter().cloned());
    expected.sort();
    let mut got = failed.clone();
    got.sort();
    assert_eq!(
        got,
        expected,
        "{} of {reachable} reachable expressions render; a FIFTH failure would be \
         a trace egret gets wrong because of this harness, not because of \
         `advance()`",
        reachable - failed.len()
    );

    for (id, task) in &kestras_own {
        let entry = golden()
            .into_iter()
            .find(|e| &e.id == id)
            .expect("the failing entry is in the golden");
        let run = entry
            .runs
            .iter()
            .find(|r| &r.task == task)
            .unwrap_or_else(|| panic!("{id} has no run for {task}"));
        assert_eq!(
            run.state.last().map(String::as_str),
            Some("FAILED"),
            "{id}'s `{task}` must be a task KESTRA failed to render too -- if it \
             succeeded there, it belongs in the harness list instead"
        );
    }
    // Cause 2, on its own: a rendered ION file URI is not JSON, so
    // `iteration_values` refuses it whatever the render context holds. This is
    // the shape `loop_::iteration_values` records as a NAMED GAP.
    let uri = "kestra:///io/kestra/tests/loop-values-from-uri/executions/               4uJl0m/tasks/write/6mNq1p/123456.ion";
    assert!(
        egret_workflow::executor::iteration_values(uri).is_err(),
        "an ION file URI is Kestra's third `values:` shape and egret implements \
         only the second; if this ever succeeds, the gap is closed and these two \
         entries are blocked by the render context alone"
    );

    for (id, task) in &blocked_twice_over {
        let entry = golden()
            .into_iter()
            .find(|e| &e.id == id)
            .expect("the failing entry is in the golden");
        let run = entry
            .runs
            .iter()
            .find(|r| &r.task == task)
            .unwrap_or_else(|| panic!("{id} has no run for {task}"));
        assert_eq!(
            run.state.last().map(String::as_str),
            Some("SUCCESS"),
            "{id}'s `{task}` is here BECAUSE Kestra ran it and egret cannot -- \
             for TWO independent reasons, one of them inside `advance()`. If \
             Kestra failed it too, it belongs in the other list"
        );
    }
}

/// **An input's declared `type:` decides how its `defaults:` is typed, not
/// whether the text happens to parse as JSON.**
///
/// [`render_context`] fills the flow's own declared defaults for inputs the
/// harvest did not post, and it has to produce the value Kestra's coercion
/// would: a `BOOL` default of `true` must reach the renderer as a **boolean**,
/// because `{{ inputs.failed == false }}` appears in six corpus flows and
/// compares against one. Round 0 got there by parsing every default as JSON,
/// which is right for `BOOL` and silently wrong for a `STRING` or `SELECT`
/// whose default is written `true`, `123` or `null`.
///
/// Zero corpus flows are that shape today — which is exactly why this test
/// exists rather than a corpus entry: without it the type gate is a rule
/// nothing can see fail, and Tasks 6-8 widen the population of rendered
/// expressions from 26 to roughly 42.
#[test]
fn an_inputs_declared_type_decides_how_its_default_is_typed() {
    let source = "id: x\nnamespace: io.kestra.tests\ninputs:\n  - id: s\n    type: STRING\n    defaults: true\n  - id: b\n    type: BOOL\n    defaults: true\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n";
    // The id is not in `corpus.json`, so `inputs` starts empty and every value
    // below comes from the flow's own `defaults:`.
    let cx = render_context("not/a/corpus/id", source);

    assert_eq!(
        render_str("{{ inputs.s == 'true' }}", &cx).expect("renders"),
        "true",
        "a STRING input's `defaults: true` is the STRING \"true\" -- parsing it \
         as JSON would make it a boolean and this comparison false"
    );
    assert_eq!(
        render_str("{{ inputs.b == true }}", &cx).expect("renders"),
        "true",
        "a BOOL input's `defaults: true` is the BOOLEAN true -- leaving it a \
         string would make this comparison false, and would break the six \
         corpus flows that write `{{ inputs.failed == false }}`"
    );
}

// --------------------------------------------------------------------------
// the sub-executions, and the exact size of the hole they leave
// --------------------------------------------------------------------------

/// **The 41 golden entries that carry a sub-execution, and what egret gets
/// right about them anyway.**
///
/// Nothing executes [`Command::SpawnExecution`] until W3b/W6, so `simulate`'s
/// side of `spawned` is always empty and every one of these entries mismatches
/// on that field whatever else it reproduces. That is the hole
/// `oracle-exec/README.md` records with its count, and this is the count: it is
/// derived here rather than written there, so a task that closes part of it
/// cannot leave the prose behind.
///
/// **An uncompared golden that looks compared is worse than no golden**, which
/// is why the *marking* is a test and not a comment. Anyone reading
/// `golden.json` sees a populated `spawned` on 41 entries and would reasonably
/// assume egret is judged on it; this says in one place that it is not, names
/// how many, and fails if the population changes.
#[test]
fn the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank() {
    let baseline = baseline();
    let mut with_spawned: Vec<String> = Vec::new();
    let mut only_spawned: Vec<String> = Vec::new();
    let mut sub_executions = 0usize;

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        if entry.spawned.is_empty() {
            continue;
        }
        with_spawned.push(entry.id.clone());
        let mut stack: Vec<&GoldenSpawned> = entry.spawned.iter().collect();
        while let Some(sub) = stack.pop() {
            sub_executions += 1;
            stack.extend(sub.spawned.iter());
        }

        let Outcome::Trace(mut actual) = evaluate(entry) else {
            continue;
        };
        let mut expected = Trace::from(entry);
        actual.restrict_order_to(&expected);
        // Blank BOTH sides' `spawned` and compare what is left. Blanking one
        // side would compare an empty vector against a populated one, which is
        // the mismatch this test exists to look past.
        expected.spawned.clear();
        if actual == expected {
            only_spawned.push(entry.id.clone());
        }
    }

    assert_eq!(
        (with_spawned.len(), sub_executions),
        (41, 142),
        "the golden's sub-execution population: 41 entries carrying 142 \
         sub-executions between them. `oracle-exec/README.md` quotes both."
    );
    assert!(
        with_spawned.iter().all(|id| baseline.contains(id)),
        "every entry carrying a sub-execution must still be baselined -- egret \
         produces no `spawned` at all, so one of these matching would mean the \
         comparison stopped reading the field: {with_spawned:?}"
    );
    println!(
        "spawned hole: {} of {} entries differ ONLY on `spawned`: {only_spawned:?}",
        only_spawned.len(),
        with_spawned.len()
    );
    assert_eq!(
        only_spawned.len(),
        33,
        "the number of entries whose ONLY remaining mismatch is the uncompared \
         `spawned` field. Written into oracle-exec/baseline.txt's header and \
         README.md; asserted here so it cannot drift from either."
    );

    // The remainder, by name rather than by subtraction, each with the reason
    // it differs on something else as well. A count alone would let one of
    // these quietly become an entry egret gets WRONG.
    let rest: Vec<&String> = with_spawned
        .iter()
        .filter(|id| !only_spawned.contains(id))
        .collect();
    assert_eq!(
        rest,
        [
            // The `Loop` whose FIRST iteration failed: Kestra stopped, and
            // `errors:` at flow level ran two more tasks. Task 7's.
            "valids/all-flowable",
            // A `disabled: true` task, which no W3a task claims.
            "valids/disable-flowable",
            // `values: "{{ outputs.write.uri }}"` -- the caller's context has no
            // `outputs`, so the render fails and the `Loop` fails with it where
            // Kestra ran three iterations. The harness, not `advance()`; see
            // `the_caller_can_render_every_expression_except_the_ones_named`.
            "valids/loop-outputs-auto",
            "valids/loop-values-from-uri",
            // `RETRY_FAILED_TASK`/`CREATE_NEW_EXECUTION`: these four carry a
            // REPLACEMENT execution rather than a loop iteration
            // (`parent_run: null`), and they are Task 7's whole subject.
            "valids/retry-new-execution-flow-attempts",
            "valids/retry-new-execution-flow-duration",
            "valids/retry-new-execution-task-attempts",
            "valids/retry-new-execution-task-duration",
        ]
        .iter()
        .collect::<Vec<_>>(),
        "33 + 8 = 41: the entries that differ on more than `spawned`, by name"
    );
}

/// **No run is ever created for a task inside a `Loop`, over the whole
/// corpus.**
///
/// A `Loop` does not iterate inside its execution: each value produces a
/// separate sub-execution, and the body's tasks never appear in the parent's
/// `taskRunList`. That is the plan's own Task 6 assertion, here at corpus scale
/// — and it is worth a test rather than a comment because `advance()` gets it
/// right *by construction* today (`flowable::descend` has no arm that walks a
/// `Loop`'s list), and "by construction" is a claim about today's `descend`.
///
/// The control is the second half: the same walk over Kestra's own golden,
/// which must find the same thing. If Kestra ever DID put a loop body task in a
/// parent's `taskRunList`, `advance()` refusing to create one would be the
/// defect and this test would be asserting it.
#[test]
fn no_run_is_ever_created_for_a_task_inside_a_loop() {
    let mut loop_bodies = 0usize;
    let mut entries_with_a_body = 0usize;

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        let Ok(plan) = build_plan(&source) else {
            continue;
        };
        let bodies: Vec<usize> = (0..plan.tasks.len())
            .filter(|&ix| plan.is_loop_body(ix))
            .collect();
        if bodies.is_empty() {
            continue;
        }
        entries_with_a_body += 1;
        loop_bodies += bodies.len();
        let body_ids: BTreeSet<&str> = bodies
            .iter()
            .map(|&ix| plan.tasks[ix].id.as_str())
            .collect();

        // egret's side.
        let golden_trace = Trace::from(entry);
        let cx = render_context(&entry.id, &source);
        let render = |expr: &str| render_str(expr, &cx).map_err(|e| e.to_string());
        if let Ok(actual) = simulate(&plan, &golden_trace, &render) {
            let created: Vec<&str> = actual
                .runs
                .iter()
                .map(|r| r.task.as_str())
                .filter(|id| body_ids.contains(id))
                .collect();
            assert!(
                created.is_empty(),
                "{}: a loop body must never get a run in the parent execution, \
                 and these did: {created:?}",
                entry.id
            );
        }

        // Kestra's side -- the control. The golden's TOP-LEVEL runs must not
        // hold one either; the body tasks live in `spawned`, which is exactly
        // the field egret cannot produce.
        let in_kestras_list: Vec<&str> = entry
            .runs
            .iter()
            .map(|r| r.task.as_str())
            .filter(|id| body_ids.contains(id))
            .collect();
        assert!(
            in_kestras_list.is_empty(),
            "{}: Kestra's own taskRunList holds a loop body task {in_kestras_list:?} \
             -- if that is real, `advance()` NOT creating one is the defect and \
             this test has it backwards",
            entry.id
        );
    }

    assert_eq!(
        (entries_with_a_body, loop_bodies),
        (39, 82),
        "the population: 39 comparable entries hold a `Loop`, with 82 tasks \
         inside a loop body between them. Both are derived; if they move, the \
         `Loop` slice of the corpus has changed"
    );
}

/// **Every first batch egret spawns is a prefix of the iterations Kestra
/// actually ran** — the part of the sub-execution hole that CAN be compared,
/// compared.
///
/// [`Command::SpawnExecution`] is emitted and never executed, so `Trace::spawned`
/// stays empty and the 41 entries that carry a sub-execution mismatch on it.
/// That leaves the command itself asserted only by a unit test on one flow,
/// which is thin for the whole of a task. This is the corpus-scale version: for
/// every `Loop` run egret starts, the `(parent_run, value)` pairs it asks for
/// must be the **first** `min(limit, size)` of the ones Kestra's `loopRun.index`
/// ordering recorded for the same run.
///
/// A prefix and not an equality, and the reason is measured rather than
/// conceded: `advance()` emits the first batch and there is no second, because
/// scheduling one needs an iteration outcome nothing here can represent. Kestra
/// ran 3 iterations of `valids/loop-serial` and egret asks for 1; it ran 1 of
/// `valids/all-flowable`'s 3 values because the first failed, and egret asks
/// for 1 there too. Both are prefixes, and the second is the one that would
/// break if the default limit were wrong.
#[test]
fn every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran() {
    let mut compared_runs = 0usize;
    let mut compared_spawns = 0usize;
    let mut exact = 0usize;
    let mut egret_only: Vec<String> = Vec::new();
    let mut divergent: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        let Ok(plan) = build_plan(&source) else {
            continue;
        };
        let golden_trace = Trace::from(entry);
        let cx = render_context(&entry.id, &source);
        let render = |expr: &str| render_str(expr, &cx).map_err(|e| e.to_string());
        let Ok((actual, spawns)) = simulate_spawns(&plan, &golden_trace, &render) else {
            continue;
        };
        if spawns.is_empty() {
            continue;
        }

        // Both sides, grouped by the run the iteration belongs to. The golden's
        // list is already in `loopRun.index` order (see normalise.py), which is
        // what makes "prefix" mean "first batch" and not "some subset".
        let mut mine: BTreeMap<RunIx, Vec<String>> = BTreeMap::new();
        for s in &spawns {
            // The command names the LOOP, and the run it names as parent is a
            // run of that same loop. A body index here would read as plausible
            // and put the sub-execution under the wrong task -- and
            // `valids/loop-serial-multiple-tasks` shows a body index could not
            // be right anyway, since its sub-executions ran BOTH body tasks.
            assert!(
                matches!(
                    plan.tasks.get(s.task).map(|t| &t.kind),
                    Some(TaskKind::Loop { .. })
                ),
                "{}: SpawnExecution must name the Loop itself, not a body task: {s:?}",
                entry.id
            );
            assert_eq!(
                actual.runs.get(s.parent_run).map(|r| r.task.as_str()),
                Some(plan.tasks[s.task].id.as_str()),
                "{}: the spawn's `task` must be the task its `parent_run` is a \
                 run of",
                entry.id
            );
            mine.entry(s.parent_run).or_default().push(s.value.clone());
        }
        let mut theirs: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for sub in &entry.spawned {
            if let (Some(parent), Some(value)) = (sub.parent_run, canonical_value(&sub.value)) {
                theirs.entry(parent).or_default().push(value);
            }
        }

        for (run, values) in &mine {
            match theirs.get(run) {
                None => egret_only.push(format!("{}#{run}", entry.id)),
                Some(kestras) => {
                    compared_runs += 1;
                    compared_spawns += values.len();
                    if kestras.len() == values.len() {
                        exact += 1;
                    }
                    if !kestras.starts_with(values.as_slice()) {
                        divergent.push(format!(
                            "{}#{run}: egret {values:?} is not a prefix of Kestra's {kestras:?}",
                            entry.id
                        ));
                    }
                }
            }
        }
    }

    assert_eq!(divergent, Vec::<String>::new(), "a divergent first batch");
    assert_eq!(
        egret_only,
        Vec::<String>::new(),
        "egret asked for an iteration of a run Kestra recorded none for"
    );
    println!(
        "first batches: {compared_runs} loop run(s), {compared_spawns} spawn(s) \
         compared, {exact} of them the WHOLE of what Kestra ran"
    );
    assert_eq!(
        (compared_runs, compared_spawns, exact),
        (36, 52, 16),
        "the reach of this comparison, derived: 36 loop runs, 52 spawn requests \
         compared against Kestra's own `loopRun` values, and 16 runs where the \
         first batch is the whole of what Kestra ran. 52 of the golden's 142 \
         sub-executions therefore have their `(parent_run, value)` compared and \
         90 do not -- and NONE of the 142 has its TRACE compared. That split is \
         the honest statement of the hole"
    );
}

/// **`concurrencyLimit` defaults to 1, and the corpus can tell** — the one
/// measurement that separates it from `Parallel::concurrent`'s default of 0.
///
/// The two fields look alike and their defaults mean opposite things, so the
/// number is not taken from either reading. This derives it from Kestra's own
/// traces: for every `Loop` in the comparable corpus that writes no
/// `concurrencyLimit`, compare the number of values it resolves against the
/// number of sub-executions Kestra actually recorded for it. If the default
/// were 0 (unlimited), Kestra would have started every value at once and the
/// second number could never be smaller than the first.
///
/// It is smaller for several, and they are named. That also says exactly what
/// would break: under a default of 0, egret would ask for `size` iterations on
/// those runs where Kestra recorded fewer, and
/// [`every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`] would fail
/// on each. So the default is guarded by a corpus assertion and not only by a
/// line of Java.
#[test]
fn the_default_concurrency_limit_is_one_and_the_corpus_can_tell() {
    let mut defaulted = 0usize;
    let mut refutes_unlimited: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        let Ok(plan) = build_plan(&source) else {
            continue;
        };
        for (ix, task) in plan.tasks.iter().enumerate() {
            let TaskKind::Loop {
                values,
                concurrency_limit,
                ..
            } = &task.kind
            else {
                continue;
            };
            // Only the loops whose limit is the DEFAULT, and only the ones a run
            // exists for at the top level -- a body loop's iterations live in a
            // sub-execution this comparison cannot see.
            if *concurrency_limit != 1 || plan.is_loop_body(ix) {
                continue;
            }
            let Ok(resolved) = egret_workflow::executor::iteration_values(values) else {
                continue;
            };
            let Some(run) = entry.runs.iter().position(|r| r.task == task.id) else {
                continue;
            };
            let ran = entry
                .spawned
                .iter()
                .filter(|s| s.parent_run == Some(run))
                .count();
            if resolved.is_empty() {
                continue;
            }
            defaulted += 1;
            if ran < resolved.len() {
                refutes_unlimited.push(format!(
                    "{} `{}`: {} value(s), {ran} sub-execution(s)",
                    entry.id,
                    task.id,
                    resolved.len()
                ));
            }
        }
    }

    println!("default-limit loops: {defaulted}; refuting unlimited: {refutes_unlimited:?}");
    assert_eq!(
        refutes_unlimited,
        [
            "valids/all-flowable `1-each`: 3 value(s), 1 sub-execution(s)",
            "valids/loop-failed `loop`: 3 value(s), 1 sub-execution(s)",
            "valids/loop-outputs-failed-render `loop`: 3 value(s), 1 sub-execution(s)",
            "valids/restart-loop `loop`: 3 value(s), 1 sub-execution(s)",
            "valids/restart-parent-loop `each`: 3 value(s), 1 sub-execution(s)",
        ],
        "these are the runs where Kestra started FEWER iterations than the flow \
         has values, which an unlimited default cannot produce. Under \
         `concurrency_limit == 0` egret would ask for every value on each of them \
         and `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` \
         would fail five times"
    );
    assert_eq!(
        defaulted, 24,
        "the population: top-level `Loop`s in the comparable corpus that write no \
         `concurrencyLimit` and resolve at least one value"
    );
}

/// **Planning the `Loop` did not move a single terminal from compared to fed.
/// It relabelled 40 that were ALREADY fed, and removed 40 wrong `SUBMITTED`
/// emissions with them.**
///
/// This is the correction to Task 6's own first account, which described the
/// `spawner` bucket as a concession. It is not one. At Tasks 3-5 a `Loop` was
/// planned as an opaque [`TaskKind::Runnable`], so `golden_outcome` already
/// supplied its terminal — and `advance()` also emitted `SUBMITTED` on it,
/// which Kestra never does for a flowable. So the trade Task 6 actually made is
/// a relabelling that *removes* an error, and the honest way to say that is to
/// derive what the old builder did rather than to remember it.
///
/// The reconstruction is Task 5's classification rebuilt from today's plan:
/// a task inside a loop body was `unplanned` (its parent was opaque), and a
/// `Loop` was a `Runnable`. **It is validated by reproducing all four of Task
/// 5's committed figures** — `424 fed / 73 flowable / 343 unplanned`, 114
/// over-submitted — which is what makes the 40 a measurement rather than an
/// inference.
#[test]
fn planning_the_loop_removed_forty_spurious_submitted_emissions() {
    let mut over_fed: BTreeMap<String, usize> = BTreeMap::new();
    let (mut fed, mut flowable, mut unplanned) = (0usize, 0usize, 0usize);

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        let Ok(plan) = build_plan(&corpus_source(&entry.id)) else {
            let mut stack: Vec<&Trace> = vec![&trace];
            while let Some(t) = stack.pop() {
                unplanned += t.runs.len();
                stack.extend(t.spawned.iter());
            }
            continue;
        };
        let mut stack: Vec<&Trace> = vec![&trace];
        while let Some(t) = stack.pop() {
            for run in t.runs.iter() {
                // Task 5's builder had no `Loop` arm, so a task inside a loop
                // body had no `PlanTask` and a `Loop` itself was opaque.
                let kind = match plan.tasks.iter().position(|p| p.id == run.task) {
                    Some(i) if !plan.is_loop_body(i) => Some(&plan.tasks[i].kind),
                    _ => None,
                };
                let submitted = run.state.iter().any(|s| s == "SUBMITTED");
                match kind {
                    None => unplanned += 1,
                    Some(TaskKind::Loop { .. }) => {
                        fed += 1;
                        if !submitted {
                            *over_fed
                                .entry("io.kestra.plugin.core.flow.Loop".to_string())
                                .or_default() += 1;
                        }
                    }
                    Some(TaskKind::Runnable { type_id }) => {
                        fed += 1;
                        if !submitted {
                            *over_fed.entry(type_id.clone()).or_default() += 1;
                        }
                    }
                    Some(_) => flowable += 1,
                }
            }
            stack.extend(t.spawned.iter());
        }
    }

    assert_eq!(
        (fed, flowable, unplanned, over_fed.values().sum::<usize>()),
        (424, 73, 343, 114),
        "the reconstruction must reproduce Task 5's four committed figures; if it \
         does not, nothing below is a measurement of anything"
    );
    assert_eq!(
        over_fed.get("io.kestra.plugin.core.flow.Loop").copied(),
        Some(40),
        "`core.flow.Loop` contributed 40 over-submitted runs before it was \
         planned. Those 40 were already FED -- their terminals came from \
         `golden_outcome` then and come from it now -- and they carried a \
         `SUBMITTED` Kestra never emits for a flowable. Task 6 moved them to the \
         `spawner` bucket and dropped the emission"
    );
    // The net movement of the flow bucket is NOT 40, and the difference is the
    // finding this package keeps re-finding: implementing a flowable enlarges
    // the over-feed for whatever nests inside it. 40 left; 5 arrived.
    let flow_then: usize = over_fed
        .iter()
        .filter(|(t, _)| t.starts_with("io.kestra.plugin.core.flow."))
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(
        (flow_then, over_fed.len()),
        (89, 12),
        "Task 5's flow bucket, reconstructed: 7 types and 89 runs"
    );
}

/// **The `Loop` completion tier is load-bearing for exactly 35 entries, and
/// without it every one of them is STUCK rather than baselined.**
///
/// Task 6's headline decision — feeding a `Loop`'s terminal in tier 3 — rests
/// on one number, and it sat in prose beside a derivation until this test. The
/// population is derived: the comparable entries whose simulation puts a `Loop`
/// run into `RUNNING`, which is precisely the set that would wait for ever if
/// [`in_flight`] stopped picking a `Loop`.
///
/// **35, not the 37 the first report said.** 39 comparable entries hold a
/// `Loop`; two never start one because they have no values to iterate
/// (`valids/loop-empty`, `valids/loop-null`) and two more never start one
/// because their `values:` cannot be resolved at all
/// (`valids/loop-outputs-auto`, `valids/loop-values-from-uri` — see
/// [`the_caller_can_render_every_expression_except_the_ones_named`]).
/// 39 − 2 − 2 = 35.
///
/// Why that settles the decision rather than softening it: a `STUCK` execution
/// is reported through the ratchet's **panic** arm, which
/// `oracle-exec/baseline.txt` does not and cannot cover. So the alternative to
/// feeding was not "compare these 35" — it was a red suite on 35 entries that
/// are honestly baselined today.
#[test]
fn the_loop_completion_tier_is_load_bearing_for_exactly_these_entries() {
    let baseline = baseline();
    let mut starts: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        let Ok(plan) = build_plan(&source) else {
            continue;
        };
        let cx = render_context(&entry.id, &source);
        let render = |expr: &str| render_str(expr, &cx).map_err(|e| e.to_string());
        let Ok(actual) = simulate(&plan, &Trace::from(entry), &render) else {
            continue;
        };
        let started = actual.runs.iter().any(|r| {
            plan.tasks
                .iter()
                .any(|p| p.id == r.task && matches!(p.kind, TaskKind::Loop { .. }))
                && r.state.iter().any(|s| s == "RUNNING")
        });
        if started {
            starts.push(entry.id.clone());
        }
    }

    println!("loop-completion tier carries {} entries", starts.len());
    assert_eq!(
        starts.len(),
        35,
        "the entries that reach tier 3 with a `Loop` in flight. Every one of them \
         goes STUCK if the tier stops completing a `Loop` -- M63 is that mutation \
         and it measured the same 35 broken"
    );
    assert!(
        starts.iter().all(|id| baseline.contains(id)),
        "all 35 are baselined today, which is the whole point: STUCK is NOT a \
         baselined state, so the alternative to feeding is a red suite rather \
         than a wider baseline"
    );
}
