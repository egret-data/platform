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
//! **A step of this ratchet can also LOSE entries**, and W3b Task 5 is the one
//! that did: re-pointing the oracle at the real driver banked 16 and lost 16,
//! and `oracle-exec/baseline.txt`'s header names all 32 with a cause each. A net
//! figure would have hidden the whole of what the step cost.
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
//! crash here". Not settling inside the driver's step bound is reported the same
//! way, and for the same reason: an implementation that spins is a broken
//! instrument, not a wrong answer.
//!
//! # What the oracle SUPPLIES, and where it is no longer circular
//!
//! **W3b Task 5 removed the feed.** Through W3a and W3b Tasks 1-4 this file
//! held a four-tier harness that supplied one thing out of `golden.json`: the
//! last element of the `state` sequence of a run whose `PlanTask` is `Runnable`
//! — and, from W3a Task 6, of a `Loop` — plus the last of each of that run's
//! attempts. **426** of the golden's 840 run terminals were fed, the comparison
//! was circular for exactly that field, and a normalisation bug in it was
//! invisible in both directions.
//!
//! *(This said **630** until W4 Task 6's fourth sweep pass. 630 is
//! `575 fed + 55 spawner`, the whole of those two buckets, and 204 of them are
//! inside sub-executions egret never created — nothing was supplied to them and
//! nothing was compared about them. `oracle-exec/README.md`'s coverage
//! statement derived 426 from this file's own table and listed 630 as a number
//! it had itself published wrong; the retired figure went on standing here, in
//! the module doc of the file that holds the derivation.)*
//!
//! Nothing on egret's side reads `golden.json` now. [`driver::run`] starts a
//! real execution with `worker::start_execution`, drives it with
//! `worker::tick`, and reads the trace back out of Postgres; a run's terminal
//! state comes from a handler that ran.
//! [`a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to`] is
//! the demonstration, and it carries the negative control W3a's first attempt
//! at the same demonstration lacked.
//!
//! **Three things the driver still supplies**, all properties of the
//! environment rather than of the execution's semantics, and none read from the
//! golden — see `oracle-exec/driver.rs`'s module doc:
//!
//! 1. the `inputs` `harvest.py` POSTed, out of `corpus.json`;
//! 2. the passage of time, by moving `executions.created_at` back to the
//!    earliest `not_before` still ahead — the harness's tier 4 against a real
//!    column;
//! 3. `core.flow.Sleep` does not really wait (`Sleep::capped(ZERO)`), which
//!    validates identically and changes nothing the golden records.
//!
//! **A stall is a wrong answer here, not a broken instrument.** The harness
//! completed every `Runnable` from the golden, so an execution that could not
//! finish meant the state machine was spinning. With real handlers a flow whose
//! task type has no handler stalls — `Registry::run` refuses an unknown type
//! rather than inventing a state — and that is what the shipping worker does.
//! The driver returns the partial trace; only the step bound and a panic are
//! `Broken`.
//! [`every_mismatching_entry_is_attributed_to_the_field_it_differs_on`] counts
//! and names the entries that end unfinished.
//!
//! **The render surface is the worker's, not this file's.** W3a's harness
//! rendered an `If`'s condition, a `Switch`'s value, a `Loop`'s `values:` and a
//! `runIf:` itself, from `inputs` and `vars` only. `worker::render` does it now,
//! inside `claim::settle`, over a much wider surface — see that module for what
//! it reaches and what it still refuses.
//!
//! # `corpus.json` and `golden.json` must describe the same flows
//!
//! Only `golden.json` is compared, so a corpus flow that quietly acquires no
//! golden entry would be **silently never tested**. That is the one failure the
//! ratchet cannot see, so [`every_corpus_flow_is_compared_or_excluded_by_name`]
//! binds the two and prints the excluded counts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use egret_workflow::executor::{build_plan, RunState, TaskKind};
use serde::Deserialize;

mod common;

/// **The real driver behind this ratchet** — W3b Task 5. See its module doc for
/// what it still supplies (the posted inputs, the passage of time, and a capped
/// `Sleep`) and for what it stopped supplying: a run's terminal state.
#[path = "../oracle-exec/driver.rs"]
mod driver;

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
    /// [`step_relation_is_strict_on_both_sides`] still pins. Every pair the golden
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
    /// egret planned the flow and the driver drove it to a standstill.
    Trace(Trace),
    /// egret refused the flow at plan time.
    Refused(String),
    /// The DRIVER failed: a panic, or the step bound. Never covered by the
    /// baseline — see this module's doc. An execution that merely stalls is a
    /// `Trace`, not this.
    Broken(String),
}

/// How many steps one execution may take before the run is declared broken.
///
/// **The four tiers this derivation is written over were W3a's harness, and
/// W3b Task 5 deleted them.** The number survives them, and this is why it is
/// still the right one: `driver::STEP_BOUND` is `4 * MEASURED_MAX_HISTORY` too,
/// and its own doc re-derives the bound over the driver's two step kinds (a
/// `tick` that ran something, and a clock move). The derivation below is kept
/// because `the_step_bound_dominates_the_widest_golden_entry` asserts the three
/// measured numbers it quotes, and those are facts about `golden.json` rather
/// than about either driver.
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
/// - **tier 2** (Task 5, the caller's render; a third kind, `Loop`, at Task 6;
///   a second *slot*, `runIf:`, at W3c) either writes a history entry —
///   the failed-render arm applies a `SetRunState` — or writes none and sets
///   `rendered` or `rendered_run_if` on at least one run. Each is written once
///   per run and never cleared, so the second case is bounded by
///   `2 × state.runs.len()`, which is bounded by twice the history entries the
///   trace holds (every run has one). **W3c doubled this term and did not touch
///   the factor**, because tier 2's share of the sum was never the binding one:
///   the factor of 4 buys `4 × 765` steps in total, and the widest golden entry
///   holds **94** runs counting its sub-executions (`valids/loop-nested`) and
///   **13** at the top level (`valids/full`) — both re-derived by
///   `the_step_bound_dominates_the_widest_golden_entry` rather than written
///   here alone.
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

/// `normalise.py`'s predicate, over the execution's LOGICAL clock instead of
/// timestamps.
///
/// The same predicate and the same endpoints: creation on the left, termination
/// on the right, strict on both sides. The endpoints come from
/// `task_runs.created_seq` / `ended_seq` (migration 0136), because a
/// `timestamptz` cannot carry the relation at all — every row a `settle` writes
/// shares one `now()`, since `now()` in Postgres is TRANSACTION time.
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

/// The inputs `harvest.py` POSTed when it captured this entry's golden.
///
/// The one field of `corpus.json` the driver needs, and it is an ENVIRONMENT
/// fact rather than a result: a golden captured with `param: true` is a trace
/// of the `then` branch, so posting the same values is what makes the two sides
/// the same execution. Reading them from the same file that says which flows
/// ran keeps the run and the golden on one provenance.
fn corpus_inputs(corpus_id: &str) -> serde_json::Value {
    corpus_json()
        .get(corpus_id)
        .and_then(|e| e.get("inputs"))
        .cloned()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

/// **egret's side of the whole comparable corpus, driven ONCE per test
/// process, through the shipping worker.**
///
/// Every test below that needs egret's answer reads this map. It is one shared
/// run rather than one per test because each entry is a real execution in
/// Postgres — a claim loop, a handler per task, a transaction per result — and
/// re-running 294 of them per assertion would buy nothing: `driver::run` is a
/// function of the flow source and the posted inputs, and neither moves between
/// tests.
///
/// **The database is emptied first**, because the claim is *global* by design
/// (a worker serves every workspace, which is the whole point of `SKIP
/// LOCKED`), so a leftover execution from an earlier process would be claimed
/// by this one's ticks and would consume its steps. `workflow.executions` is
/// written by nothing but the worker and these tests — checked, not assumed.
///
/// # The `TRUNCATE`, and the ruling about it (W3b Task 6)
///
/// The plan ruled that this statement "truncates `workflow.executions` in the
/// shared `egret_test` database" and had to be scoped or given a private
/// database. **Measured, that premise is false**: `common::db()` goes through
/// `egret_test_db::url()`, which does not hand back `DATABASE_URL`'s database at
/// all — it clones a migrated template into `egret_t_<pid as 32 hex>`, one per
/// test *process*, and drops it on exit. A probe printed
/// `DATABASE_URL=…/egret_test  current_database()=egret_t_000000000000000000000000000185c6`.
/// So the private database the ruling asked for is already taken, and has been
/// since before Task 5; the destructive statement never reaches `egret_test`.
///
/// What Task 6 changed is that this is now **structural rather than
/// incidental**. [`is_process_clone`] is checked immediately before the
/// `TRUNCATE`, so the statement cannot run against anything but this process's
/// own clone even if a future edit points `common::db()` somewhere else — which
/// is the change that would otherwise reintroduce exactly the destruction the
/// ruling was written about.
/// **`egret.sql` reaches zero corpus entries, derived rather than quoted.**
///
/// The spec (§3.1) says the programme spec's handler list — `core.log`,
/// `egret.sql` — predates the corpus, and that `egret.sql` has no oracle
/// coverage at all. That is the sentence a green ratchet must not be read
/// against, so it is a **measurement** here rather than a claim in a README:
/// Kestra has no `egret.*` task type, so no golden entry can name one, and no
/// entry this file compares can ever exercise `worker::handlers::sql`.
///
/// **The `Log` count is the positive control**, and it is the whole reason this
/// test is worth anything. A zero is only a measurement if the instrument can
/// move: a scanner that parsed nothing — a changed file layout, a `type:`
/// spelling this walk does not recognise — would report zero `egret.*` flows and
/// look exactly like the truth. It cannot also report 106 flows naming a type it
/// failed to parse.
///
/// The control number is **108**, not the 106 a first derivation gave: two
/// flows write the type as a list item (`- type: io.kestra.plugin.core.log.Log`,
/// in `invalids/invalid` and `invalids/system-labels`) and a first scanner that
/// required `type:` to begin its line missed both. The scanner below strips a
/// leading `- ` for that reason, and the discrepancy is recorded rather than
/// silently corrected because it is the exact failure this control exists to
/// catch — a scanner that under-reads looks like a true zero.
///
/// **W4 Task 5's population is folded in** rather than left as the shell
/// pipeline the coverage statement used to quote for it: no corpus flow names a
/// script, container, shell or command task of *any* vendor either, because
/// Kestra keeps those in plugin jars this corpus never reaches. The filter that
/// answers that has a positive control of its own, since a corpus-wide zero
/// from a predicate that always says `false` is not a measurement.
///
/// Totals asserted beside the parts, all three populations.
#[test]
fn no_corpus_entry_can_exercise_the_egret_sql_handler() {
    fn task_types(source: &str) -> Vec<String> {
        source
            .lines()
            .filter_map(|line| {
                let t = line.trim().trim_start_matches("- ").trim();
                let v = t.strip_prefix("type:")?.trim();
                Some(v.trim_matches(|c| c == '"' || c == '\'').to_string())
            })
            .collect()
    }

    // W4 Task 5's population, folded in here rather than left in a shell
    // pipeline the coverage statement quoted: Kestra's OWN script, shell and
    // container tasks. `egret.container` and `egret.script.python` are covered
    // by the `egret.` prefix below — this is the other half of the sentence,
    // that no Kestra flow in this corpus runs a process either, because those
    // plugins live in jars (`plugin-script-*`, `plugin-docker`) the corpus
    // never reaches. Same control, same reason.
    fn is_process_task(t: &str) -> bool {
        let t = t.to_ascii_lowercase();
        [
            "script",
            "container",
            "shell",
            "command",
            "docker",
            "bash",
            "python",
            "node",
        ]
        .iter()
        .any(|needle| t.contains(needle))
    }

    // The predicate's own positive control, because a corpus-wide zero from a
    // predicate that answers `false` to everything is not a measurement. The
    // 108 `Log` flows control the SCANNER; this controls the FILTER, and they
    // are different instruments — the `egret.` prefix above has the same shape
    // and the same need, which its `egret.container` row supplies.
    assert!(
        is_process_task("io.kestra.plugin.scripts.python.Commands")
            && is_process_task("io.kestra.plugin.docker.Run")
            && is_process_task("egret.script.python")
            && is_process_task("egret.container")
            && !is_process_task("io.kestra.plugin.core.log.Log"),
        "the process-task filter must be able to say yes, and must not say it \
         to the control's own type"
    );

    let corpus: Vec<CorpusEntry> = corpus();
    let mut corpus_total = 0usize;
    let (mut corpus_egret, mut corpus_log, mut corpus_process) = (0usize, 0usize, 0usize);
    for entry in &corpus {
        corpus_total += 1;
        let types = task_types(&corpus_source(&entry.id));
        if types.iter().any(|t| t.starts_with("egret.")) {
            corpus_egret += 1;
        }
        if types.iter().any(|t| is_process_task(t)) {
            corpus_process += 1;
        }
        if types.iter().any(|t| t == "io.kestra.plugin.core.log.Log") {
            corpus_log += 1;
        }
    }

    let mut golden_total = 0usize;
    let (mut golden_egret, mut golden_log) = (0usize, 0usize);
    for entry in golden() {
        golden_total += 1;
        let types = task_types(&corpus_source(&entry.id));
        if types.iter().any(|t| t.starts_with("egret.")) {
            golden_egret += 1;
        }
        if types.iter().any(|t| t == "io.kestra.plugin.core.log.Log") {
            golden_log += 1;
        }
    }

    assert_eq!(
        (corpus_total, corpus_egret, corpus_process, corpus_log),
        // 345 -> 351 and 0 -> 6 process tasks: the six `working-directory-real-*`
        // flows (WorkingDirectory Task 8) each name `io.kestra.plugin.scripts.shell.Commands`;
        // none names a `egret.*` type and none names `core.log.Log`, so 0 and 108 hold.
        (351, 0, 6, 108),
        "every corpus flow, how many name an `egret.*` task type, how many name \
         a script/container/shell/command type of ANY vendor, and the control: \
         how many name `core.log.Log`"
    );
    // golden 294 -> 301 at WorkingDirectory Task 8 (the six `working-directory-real-*`
    // flows and `retry-failed-task-duration`): none names an `egret.*` type or
    // `core.log.Log`, so the 0 and the 97 hold.
    assert_eq!(
        (golden_total, golden_egret, golden_log),
        (301, 0, 97),
        "the same three over the population this file actually compares. The middle \
         number is what `oracle-exec/README.md`'s coverage statement rests on: the \
         ratchet being green says NOTHING about `worker::handlers::sql`, whose only \
         instruments are `tests/sql_starrocks_live.rs` and its own unit tests"
    );
}

/// Every corpus flow, parsed by the product's own model, plus the ones that do
/// not parse — **counted, never skipped.**
///
/// `trigger::flow`'s own corpus test walks the directory and `continue`s on a
/// flow `serde_norway` refuses, which is silent: a flow that failed to parse
/// *and* declared a trigger would shrink every census below and look exactly
/// like a smaller corpus. This returns both halves so the caller can assert
/// `parsed + unparsed == 351` and check what the unparsed ones contain.
type ParsedCorpus = (
    Vec<(String, egret_workflow::models::Flow)>,
    Vec<(String, String)>,
);

fn parsed_corpus() -> ParsedCorpus {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for entry in corpus() {
        let text = corpus_source(&entry.id);
        match serde_norway::from_str::<egret_workflow::models::Flow>(&text) {
            Ok(flow) => ok.push((entry.id.clone(), flow)),
            Err(e) => bad.push((entry.id.clone(), e.to_string())),
        }
    }
    (ok, bad)
}

/// Does this corpus flow declare a task of type `ty`?
///
/// Textual, and deliberately the same shape as
/// [`no_corpus_entry_can_exercise_the_egret_sql_handler`]'s scanner, whose
/// `core.log.Log` control this reuses: a scanner that parsed nothing would
/// report zero of everything and look identical to the truth.
fn declares_type(source: &str, ty: &str) -> bool {
    source.lines().any(|line| {
        line.trim()
            .trim_start_matches("- ")
            .trim()
            .strip_prefix("type:")
            .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'') == ty)
            .unwrap_or(false)
    })
}

/// **The sixteen entries the W6 ratchet movement speaks for, and the four
/// constructs it does not speak for at all.**
///
/// This is the derivation behind the W6 section of `oracle-exec/README.md`'s
/// coverage statement, and it exists because **sixteen banked entries can make
/// four triggers look measured when none of them is**. Everything the statement
/// prints is computed here, with the total beside its parts, because a figure is
/// safe only if a running program compares it.
///
/// # The twelve are not a list, they are a predicate — and the predicate has a trap
///
/// The twelve `Subflow` banks are derived as *golden entries declaring a
/// `Subflow` task, verdict `runs`, matching*. Read carelessly that is *matching
/// ∧ declares a `Subflow`*, which is **thirteen**: `invalids/recursive-flow` is
/// a flow Kestra REFUSED, egret refuses it too, and it has matched since long
/// before this package. Counting it would attribute a thirteenth bank to
/// `Subflow` executing. It is asserted separately, by name, for exactly that
/// reason — *a fixture answers the question you built it for and stays silent
/// about the one you then generalise to.*
///
/// # The four constructs, and the blindness is FOUR different mechanisms
///
/// Derived below, per construct, rather than summarised as "the oracle cannot
/// see it":
///
/// 1. **webhook** — the harness never sends the request. `harvest.py` contains
///    the string `executions/webhook` exactly **once**, as the *reason*
///    `valids/webhook-inputs` is excluded from the corpus. The trigger is not
///    invisible; it is never invoked.
/// 2. **schedule** — Kestra's scheduler fires it (the compose file runs
///    `server standalone`), and the execution it creates hangs off **nothing**:
///    `parent_id` NULL and no trigger link either. `descendants()` walks
///    `parent_id` from one root, so a schedule-fired execution is not a
///    descendant of anything the harness posted and cannot be attributed to an
///    entry at all.
/// 3. **flow trigger** — fires, and the execution records its cause in
///    `trigger.variables.executionId` (egret's `trigger_execution_id` column
///    stands in for that field; the artifact-side name is the JSON path, and
///    whether Kestra generates a COLUMN for it is unmeasured — `parent_id` is
///    generated and this oracle has never named a second). **No arm of
///    `descendants()` reads it.** Same verdict as (2), different mechanism:
///    here the link exists and is not walked; there there is no link to walk.
/// 4. **backfill** — no population. **Zero** live declarations in 351 flows;
///    the only two occurrences are commented out, and they are named here so
///    the zero is a measurement and not a scanner that reads nothing.
///
/// The control that makes the zero mean something is the **142** sub-executions
/// the golden does hold: this oracle records sub-executions when Kestra links
/// them by `parent_id`, so "no trigger-fired execution anywhere in the golden"
/// is a statement about the link, not about an instrument that records nothing.
///
/// # The sharpest single row: `valids/task-flow`
///
/// One of the sixteen banked entries **declares a `Schedule` trigger**, and it
/// banked because `Subflow` executes. The design spec's §1 recorded the cron row
/// as *1 baselined declaration*; it is **0** now, and not one thing egret does
/// with a schedule caused that. An attribution by construct that counted
/// declarations would have credited the scheduler with a bank.
#[test]
fn the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none() {
    // ---- the split every figure below is stated against --------------------
    let golden = golden();
    let baselined = baseline();
    let ids: BTreeSet<String> = golden.iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids.len(), golden.len(), "golden ids must be unique");
    assert!(
        baselined.is_subset(&ids),
        "every baseline line must name a golden entry"
    );
    let matching: BTreeSet<String> = ids.difference(&baselined).cloned().collect();
    assert_eq!(
        (golden.len(), matching.len(), baselined.len()),
        // 294 -> 301 golden, 104 -> 103 baselined (WorkingDirectory Task 8 banked
        // `valids/working-directory-taskrun`). The non-baselined side is 190 -> 198:
        // the 7 entries the harvest added (six `working-directory-real-*` and
        // `retry-failed-task-duration`), plus the banked one. Of those 8, seven
        // match and `retry-failed-task-duration` is the one regression that
        // `executor_matches_golden` reports (a pre-existing timing mismatch, see
        // the report); 103 + 198 = 301.
        // 103 -> 104 baselined, 198 -> 197 matching at WorkingDirectory Task 9:
        // `valids/retry-failed-task-duration` is baselined with its cause, so its
        // mismatch is now attributed rather than reported; 104 + 197 = 301.
        (301, 197, 104),
        "301 = matching + baselined, the total beside its parts. It read \
         167/127 through W8 Task 2; W8 Task 3 banked eight `Pause` entries, W8 \
         Task 4 banked `valids/waitfor-nested`, W8 Task 5 banked seven \
         `allowFailure:`/`allowWarning:` entries, W8 Task 6 banked the two \
         `sla:` `EXECUTION_ASSERTION`s, W9a Task 11 banked \
         `valids/loop-values-from-uri` and no-silent-stall Task 1 banked \
         `invalids/invalid-task` (an unknown type is refused now, as Kestra \
         refuses it)"
    );

    // ---- the sixteen, by name and by derivation ----------------------------
    const TWELVE: [&str; 12] = [
        "valids/flow-concurrency-parallel-subflow-kill",
        "valids/flow-concurrency-parallel-subflow-kill-child",
        "valids/flow-concurrency-subflow",
        "valids/loop-with-subflow",
        "valids/subflow-child",
        "valids/subflow-inherited-labels-parent",
        "valids/subflow-nullable-input-parent",
        "valids/subflow-parent",
        "valids/subflow-parent-no-wait",
        "valids/subflow-parent-of-failed",
        "valids/task-flow",
        "valids/task-flow-inherited-labels",
    ];
    const FOUR: [&str; 4] = [
        "valids/retry-new-execution-flow-attempts",
        "valids/retry-new-execution-flow-duration",
        "valids/retry-new-execution-task-attempts",
        "valids/retry-new-execution-task-duration",
    ];
    let twelve: BTreeSet<String> = TWELVE.iter().map(|s| s.to_string()).collect();
    let four: BTreeSet<String> = FOUR.iter().map(|s| s.to_string()).collect();
    let sixteen: BTreeSet<String> = twelve.union(&four).cloned().collect();
    assert_eq!(
        (twelve.len(), four.len(), sixteen.len()),
        (12, 4, 16),
        "16 = 12 + 4, and no id is counted in both halves"
    );
    assert!(
        sixteen.is_subset(&matching),
        "all sixteen must be on the MATCHING side, or the ratchet does not \
         speak for them: {:?}",
        sixteen.difference(&matching).collect::<Vec<_>>()
    );

    const SUBFLOW: &str = "io.kestra.plugin.core.flow.Subflow";
    let (mut sub_runs_matching, mut sub_refused_matching, mut sub_baselined) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let (mut cne_matching, mut cne_baselined) = (BTreeSet::new(), BTreeSet::new());
    let mut log_control = 0usize;
    for entry in &golden {
        let source = corpus_source(&entry.id);
        if declares_type(&source, "io.kestra.plugin.core.log.Log") {
            log_control += 1;
        }
        if declares_type(&source, SUBFLOW) {
            match (baselined.contains(&entry.id), entry.verdict) {
                (true, _) => sub_baselined.insert(entry.id.clone()),
                (false, Verdict::Runs) => sub_runs_matching.insert(entry.id.clone()),
                (false, Verdict::Refused) => sub_refused_matching.insert(entry.id.clone()),
            };
        }
        if source.contains("CREATE_NEW_EXECUTION") {
            if baselined.contains(&entry.id) {
                cne_baselined.insert(entry.id.clone());
            } else {
                cne_matching.insert(entry.id.clone());
            }
        }
    }
    assert_eq!(
        log_control, 97,
        "the scanner's positive control over the GOLDEN population -- the same \
         97 `core.log.Log` entries `no_corpus_entry_can_exercise_the_egret_sql_handler` \
         counts. A scanner that read nothing would report every census below as \
         zero and look exactly like the truth"
    );
    // **Twelve until W8 Task 5, fourteen now** -- and the set is asserted
    // rather than the count precisely so that a fifteenth cannot arrive
    // silently. Both new members are `Subflow`s whose `allow*` flags move the
    // terminal their failing child transmits -- to `SUCCESS` for
    // `…-warning-…`, to `WARNING` for `…-failure-…` -- so they joined the
    // matching side without anything about `Subflow` itself changing. The
    // coverage statement's 16 is W6 Task 3's and stays 16: neither is one of
    // its twelve.
    let fourteen: BTreeSet<String> = twelve
        .union(
            &[
                "valids/task-allow-warning-executable-flow".to_string(),
                "valids/task-allow-failure-executable-flow".to_string(),
            ]
            .into(),
        )
        .cloned()
        .collect();
    assert_eq!(
        sub_runs_matching, fourteen,
        "the `Subflow` banks, DERIVED: every golden entry that declares a \
         `Subflow`, that Kestra ran, and that egret now reproduces. If this set \
         grows, the coverage statement's 16 is stale and must be re-derived -- \
         which is the point of asserting the set rather than the count"
    );
    assert_eq!(
        sub_refused_matching,
        ["invalids/recursive-flow"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        "THE TRAP, asserted rather than avoided: one `Subflow`-declaring entry \
         matches and is NOT a W6 bank. Kestra refused it, egret refuses it too, \
         and it has matched since long before this package -- so `matching AND \
         declares a Subflow` is 15, and the fourteen above is the `runs` half"
    );
    assert_eq!(
        (
            sub_runs_matching.len() + sub_refused_matching.len() + sub_baselined.len(),
            sub_runs_matching.len(),
            sub_refused_matching.len(),
            sub_baselined.len()
        ),
        (23, 14, 1, 8),
        "23 `Subflow`-declaring golden entries = 14 reproduced + 1 refusal that \
         always matched + 8 still baselined. It read `12 + 1 + 10` until W8 \
         Task 5 moved both `valids/task-allow-{{failure,warning}}-executable-flow` \
         across on their `allow*` flags. The design spec's 22 was the BASELINED count before \
         this package (22 = 12 + 10), which is a different population from this \
         one"
    );
    assert_eq!(
        cne_matching, four,
        "the four `CREATE_NEW_EXECUTION` banks, derived the same way"
    );
    assert!(
        cne_baselined.is_empty(),
        "and no `CREATE_NEW_EXECUTION` entry is left baselined: {cne_baselined:?}"
    );

    // ---- the four constructs -----------------------------------------------
    let (parsed, unparsed) = parsed_corpus();
    assert_eq!(
        parsed.len() + unparsed.len(),
        // 345 -> 351: the six `working-directory-real-*` flows (WorkingDirectory Task 8).
        351,
        "every corpus flow is parsed or named, never skipped"
    );
    // W13b Task 2: exactly ONE flow that declares triggers no longer parses --
    // `valids/flow-trigger-fire-once-true-flow-listen`, whose `window:
    // {fireOnce: true}` `models::Window` refuses as the pinned artifact does
    // (422). It is named, and its one flow-trigger declaration is added back
    // to the census below by hand, rather than letting it silently shrink it.
    const FIRE_ONCE: &str = "valids/flow-trigger-fire-once-true-flow-listen";
    let unparsed_with_triggers: Vec<&(String, String)> = unparsed
        .iter()
        .filter(|(id, _)| corpus_source(id).contains("triggers:"))
        .collect();
    assert!(
        unparsed_with_triggers.len() == 1
            && unparsed_with_triggers[0].0 == FIRE_ONCE
            && unparsed_with_triggers[0]
                .1
                .contains("unknown field `fireOnce`"),
        "a flow that does not parse AND declares a `triggers:` block would \
         shrink the census below and look exactly like a smaller corpus, so the \
         census is only honest while the fireOnce flow is the only one, refused \
         for `fireOnce`: {unparsed:?}"
    );

    const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
    const FLOW_TRIGGER: &str = "io.kestra.plugin.core.trigger.Flow";
    const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
    let mut census: BTreeMap<&str, (BTreeSet<String>, usize)> = BTreeMap::new();
    let mut declaring_flows: BTreeSet<String> = BTreeSet::new();
    for (id, flow) in &parsed {
        for t in &flow.triggers {
            let ty = t.r#type.as_str();
            let Some(key) = [SCHEDULE, FLOW_TRIGGER, WEBHOOK]
                .into_iter()
                .find(|k| *k == ty)
            else {
                continue;
            };
            // The product's own classifier must agree with the type string --
            // two derivations of one fact, and a `Leaf` here would mean the
            // scheduler never sees a trigger this census says it does.
            let typed = matches!(
                (key, &t.kind),
                (SCHEDULE, egret_workflow::models::TriggerKind::Schedule(_))
                    | (FLOW_TRIGGER, egret_workflow::models::TriggerKind::Flow(_))
                    | (WEBHOOK, egret_workflow::models::TriggerKind::Webhook(_))
            );
            assert!(
                typed,
                "{id}/{} declares {ty} and the model classified it as something \
                 else -- the census and the scheduler would disagree",
                t.id
            );
            let e = census.entry(key).or_default();
            e.0.insert(id.clone());
            e.1 += 1;
            declaring_flows.insert(id.clone());
        }
    }
    let flows_and_declarations = |k: &str| {
        let (f, d) = census.get(k).cloned().unwrap_or_default();
        (f.len(), d)
    };
    assert_eq!(
        (
            flows_and_declarations(SCHEDULE),
            flows_and_declarations(FLOW_TRIGGER),
            flows_and_declarations(WEBHOOK),
        ),
        ((7, 8), (16, 20), (15, 15)),
        "(flows, declarations) per construct, over the flows that PARSE. The \
         fireOnce flow above is the 17th flow-trigger flow and the 21st \
         declaration; 17 and 21 are the same two numbers \
         `trigger::flow::tests::the_corpus_flow_triggers_split_twenty_to_one` \
         derives from the directory -- two independent routes to one census"
    );

    // The fireOnce flow declares a flow trigger; it is counted here by hand,
    // as promised above, so the figures below still describe all 351 flows
    // (345 + the six `working-directory-real-*` flows of WorkingDirectory Task 8).
    declaring_flows.insert(FIRE_ONCE.to_string());

    let trigger_golden: BTreeSet<String> = declaring_flows.intersection(&ids).cloned().collect();
    let trigger_matching: BTreeSet<String> =
        trigger_golden.intersection(&matching).cloned().collect();
    let trigger_baselined: BTreeSet<String> =
        trigger_golden.intersection(&baselined).cloned().collect();
    assert_eq!(
        (
            declaring_flows.len(),
            trigger_golden.len(),
            trigger_matching.len(),
            trigger_baselined.len()
        ),
        (38, 31, 28, 3),
        "corpus flows declaring one of the three (38 = 7 + 17 + 15 with \
         `invalids/duplicate` declaring two kinds), how many the oracle compares \
         at all, and the split. **28 MATCHING entries declare a trigger this \
         package built, and their matching is evidence the trigger was not \
         evaluated on either side** -- which is the inversion this statement \
         exists to prevent a reader making"
    );
    let uncompared: BTreeSet<String> = declaring_flows.difference(&ids).cloned().collect();
    assert_eq!(
        (
            uncompared.len(),
            uncompared.contains("valids/webhook-inputs")
        ),
        (7, true),
        "and the other 7 have no golden entry at all -- they are in the 51 this \
         oracle never compares. One of them is `valids/webhook-inputs`, which is \
         there for exactly the reason this construct is invisible: it can only be \
         started by the webhook POST the harness never sends. 38 = 31 + 7"
    );
    assert_eq!(
        trigger_baselined,
        [
            "valids/webhook-outputs",
            "valids/webhook-plaintext",
            "valids/webhook-wait"
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<BTreeSet<_>>(),
        "the three baselined trigger-declaring entries, all webhook, all \
         baselined for reasons that are not the trigger (the design spec's §1 \
         webhook row: 3, unchanged by this package)"
    );

    // Not one trigger-fired execution anywhere in the golden.
    fn sub_executions(spawned: &[GoldenSpawned], loopish: &mut usize, replayish: &mut usize) {
        for s in spawned {
            if s.parent_run.is_none() && s.value.is_null() {
                *replayish += 1;
            } else {
                *loopish += 1;
            }
            sub_executions(&s.spawned, loopish, replayish);
        }
    }
    let (mut loopish, mut replayish, mut carriers) = (0usize, 0usize, 0usize);
    for entry in &golden {
        let (mut l, mut r) = (0usize, 0usize);
        sub_executions(&entry.spawned, &mut l, &mut r);
        if l + r > 0 {
            carriers += 1;
        }
        if trigger_golden.contains(&entry.id) {
            assert_eq!(
                (l, r),
                (0, 0),
                "{} declares a trigger and its golden entry holds {} \
                 sub-executions. Every one of the 31 holds exactly ONE \
                 execution -- the harness's own POST -- so nothing a trigger \
                 fired reached the golden",
                entry.id,
                l + r
            );
        }
        loopish += l;
        replayish += r;
    }
    assert_eq!(
        (loopish + replayish, loopish, replayish, carriers),
        (142, 134, 8, 41),
        "THE CONTROL for the zero above: 142 sub-executions over 41 entries = \
         134 carrying a loop index + 8 carrying none (the `CREATE_NEW_EXECUTION` \
         replays). This oracle records a sub-execution when Kestra links it by \
         `parent_id`, so the zero is a statement about the LINK and not about an \
         instrument that records nothing"
    );

    // `valids/task-flow`: banked, and it declares a schedule.
    let schedule_flows = census.get(SCHEDULE).cloned().unwrap_or_default().0;
    assert!(
        schedule_flows.contains("valids/task-flow") && sixteen.contains("valids/task-flow"),
        "one of the sixteen declares a `Schedule` trigger and banked because \
         `Subflow` executes. The cron row's `1 baselined declaration` is 0 now \
         for a reason that has nothing to do with a schedule"
    );
    assert!(
        schedule_flows
            .intersection(&sixteen)
            .cloned()
            .collect::<BTreeSet<_>>()
            .len()
            == 1
            && census
                .get(FLOW_TRIGGER)
                .cloned()
                .unwrap_or_default()
                .0
                .is_disjoint(&sixteen)
            && census
                .get(WEBHOOK)
                .cloned()
                .unwrap_or_default()
                .0
                .is_disjoint(&sixteen),
        "and it is the ONLY overlap: no other banked entry declares any of the \
         three"
    );

    // ---- the webhook's own mechanism: the request is never sent ------------
    let harvest = include_str!("../oracle-exec/harvest.py");
    assert_eq!(
        harvest.matches("executions/webhook").count(),
        1,
        "`harvest.py` names the webhook execution path exactly once, and it is \
         the REASON `valids/webhook-inputs` is excluded. A second occurrence \
         would mean the harness posts one somewhere"
    );
    assert!(
        harvest
            .lines()
            .find(|l| l.contains("executions/webhook"))
            .is_some_and(|l| l.contains("webhook-inputs")),
        "and that one occurrence is the exclusion reason, not a request"
    );
    assert!(
        harvest.contains("parent_id = :'root'"),
        "the one live discovery arm keys on `parent_id`, which Kestra leaves \
         NULL for a subflow child and for anything a trigger creates -- the \
         predicate is evaluated on every settle poll of all 274 executed flows \
         and structurally cannot select a trigger-fired execution"
    );
    let normalise = include_str!("../oracle-exec/normalise.py");
    assert!(
        normalise.contains("parentId"),
        "and the normaliser builds the tree on `parentId` too, so even a \
         DISCOVERED trigger-fired execution could not be normalised into a trace"
    );

    // ---- backfill: no population at all ------------------------------------
    let (mut live, mut commented) = (Vec::new(), BTreeSet::new());
    for entry in corpus() {
        for line in corpus_source(&entry.id).lines() {
            if line.contains("backfill") {
                if line.trim_start().starts_with('#') {
                    commented.insert(entry.id.clone());
                } else {
                    live.push((entry.id.clone(), line.trim().to_string()));
                }
            }
        }
    }
    assert!(
        live.is_empty(),
        "no corpus flow declares a live `backfill:`: {live:?}"
    );
    assert_eq!(
        commented,
        ["valids/full", "valids/labels-deserialization"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        "and the two commented-out occurrences are the scanner's own control: \
         it finds what IS there, so the zero above is a measurement rather than \
         a scanner that reads nothing"
    );
}

/// **The SEEDED `correlationId` divergence has no corpus population**, and
/// every figure `worker::render`'s comment states about it is compared here
/// rather than stated.
///
/// Kestra copies the cause's `system.correlationId` **label** verbatim and fills
/// one in only when it is ABSENT, so a root that declares one hands that value —
/// `ZZZTESTROOT` in W6 Task 6's probe, not an execution id at all — to every
/// descendant. `render::Surface::load` derives the value from the chain root's
/// **id** instead, and its unconditional `labels.insert("system", …)`
/// additionally **shadows** any declared `system.*` label.
///
/// Both would show only on a flow that declares `system.correlationId`, or that
/// reads a `system.*` label it declares, and the corpus holds neither. That is
/// why the gap is recorded against the wiring seam
/// (`trigger::flow::FiredExecution::correlation_id`) rather than banked or
/// fixed: **the ratchet cannot move either way on it.**
///
/// # What this test CANNOT check, and it is why the name gained a word
///
/// It was `the_correlation_id_divergence_…`, and under that name it read as
/// covering the value `Surface::load` computes. **It does not, and the
/// whole-branch review found a live defect behind exactly that reading**: the
/// `root` walk followed `parent_execution_id` alone and answered a subflow
/// child its OWN id, two corpus flows reach it, and one of them is the target
/// of a banked entry. The assertion below that both readers are *matching*
/// golden entries cannot see that, and the reason is the shape this branch
/// keeps meeting: **both are compared only as ROOT executions**, where every
/// implementation of the walk returns the same value. The walk's instrument is
/// `a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item` in
/// `tests/worker.rs`, which drives two subflow links and carries a loop control;
/// this test is about the **label the flow declares**, and that is all its name
/// now claims.
///
/// # This census is TEXTUAL, and the first version was not — which is the finding
///
/// Taken through `models::Flow`, the label census reads **21** flows, not 22,
/// because **four corpus flows do not parse with the product's own model** —
/// all four `invalids/`, which Kestra refused too — and **one of them,
/// `invalids/system-labels`, carries two of the three `system.*` declarations
/// this test names.** A model-based census would have reported one declaration
/// where there are three, silently, and the sentence it was supporting would
/// still have read as true. Both counts are asserted below so the gap between
/// the two instruments is a number rather than a surprise; the claim is about
/// what the corpus *says*, so the text is the instrument and the model is the
/// cross-check.
///
/// The trigger census in
/// [`the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none`]
/// IS model-based and is sound for a reason it asserts rather than assumes:
/// none of the four unparsed flows declares a `triggers:` block.
#[test]
fn the_seeded_correlation_id_divergence_has_no_corpus_population() {
    let mut declaring_labels = 0usize;
    let mut system_declarations: Vec<String> = Vec::new();
    let mut correlation_lines: Vec<(String, String)> = Vec::new();
    for entry in corpus() {
        let source = corpus_source(&entry.id);
        if source.lines().any(|l| l.trim_end() == "labels:") {
            declaring_labels += 1;
        }
        for line in source.lines() {
            // A label key at map depth 1 whose name is namespaced `system.…`:
            // indented, `system.` , a name, then the colon.
            let t = line.trim_start();
            if line.starts_with(' ')
                && t.starts_with("system.")
                && t.split(':').next().is_some_and(|k| {
                    k.len() > "system.".len() && !k.contains(' ') && !k.contains('{')
                })
            {
                system_declarations.push(format!("{}:{}", entry.id, t.trim()));
            }
            // `labels.system.correlationId`, WITHOUT assuming the spacing: one
            // of the two corpus reads is `{{labels.system.correlationId}}` and
            // the other `{{ labels.system.correlationId }}`, so a grep for
            // either spelling finds exactly half of this population.
            if line.contains("correlationId") {
                correlation_lines.push((entry.id.clone(), line.trim().to_string()));
            }
        }
    }
    assert_eq!(
        declaring_labels, 22,
        "the control: 22 corpus flows declare a top-level `labels:` block, so a \
         zero below is a measurement and not a scanner that sees no labels"
    );
    system_declarations.sort();
    assert_eq!(
        system_declarations,
        vec![
            "invalids/system-labels:system.id: id",
            "invalids/system-labels:system.label: system_key",
            "valids/minimal:system.readOnly: \"true\"",
        ],
        "three `system.*` label declarations exist, in two flows -- named, \
         because `none of them is system.correlationId` is only worth saying \
         beside the ones that ARE there"
    );
    assert!(
        !system_declarations
            .iter()
            .any(|d| d.contains("system.correlationId")),
        "and none seeds `system.correlationId`, which is the whole reason the \
         divergence is unreachable here: {system_declarations:?}"
    );

    let mut readers: Vec<String> = correlation_lines.iter().map(|(id, _)| id.clone()).collect();
    readers.sort();
    assert_eq!(
        readers,
        vec![
            "valids/follow-dependencies-partial-child",
            "valids/subflow-grand-child"
        ],
        "the only two corpus flows whose text names `correlationId` are the two \
         `render.rs` names as the measurement its `system` label exists for"
    );
    assert!(
        correlation_lines
            .iter()
            .all(|(_, line)| line.contains("labels.system.correlationId") && line.contains("{{")),
        "and both READ the label through an expression rather than declaring \
         one: {correlation_lines:?}"
    );
    let ids: BTreeSet<String> = golden().into_iter().map(|e| e.id).collect();
    let baselined = baseline();
    for id in &readers {
        assert!(
            ids.contains(id) && !baselined.contains(id),
            "{id} reads the label and must be a MATCHING golden entry -- which \
             says exactly one thing, and the message here used to say more: the \
             oracle drives both of these flows as ROOT executions, where every \
             implementation of `Surface::load`'s walk answers the same value. \
             So this pins that the label RENDERS and the entry reproduces, and \
             it is silent about the walk. Reaching the walk needs a chain, which \
             `a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item` \
             drives"
        );
    }

    // The instrument gap, as a number: what the product's own model sees.
    let (parsed, unparsed) = parsed_corpus();
    assert_eq!(
        parsed.len() + unparsed.len(),
        // 345 -> 351: the six `working-directory-real-*` flows (WorkingDirectory Task 8).
        351,
        "every corpus flow is parsed or named, never skipped"
    );
    let model_labels = parsed.iter().filter(|(_, f)| !f.labels.is_empty()).count();
    let model_system = parsed
        .iter()
        .flat_map(|(id, f)| f.labels.iter().map(move |l| (id, l)))
        .filter(|(_, l)| l.key.starts_with("system."))
        .count();
    let unparsed_ids: BTreeSet<&str> = unparsed.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        (
            unparsed.len(),
            model_labels,
            model_system,
            unparsed_ids.contains("invalids/system-labels")
        ),
        (5, 21, 1, true),
        "the model-based census, beside the textual one: FIVE flows `models::Flow` \
         refuses -- four `invalids/`, plus (since W13b Task 2) the fireOnce flow, \
         which declares no label -- so it sees 21 label-declaring flows of 22 and \
         ONE `system.*` declaration of three -- because the flow holding the other \
         two is one of the four. 22 - 21 = 1 flow and 3 - 1 = 2 declarations, and \
         both differences are that one flow. Unparsed: {unparsed_ids:?}"
    );
}

/// Is `name` a database `egret-test-db` created for one test process?
///
/// The rule is that crate's `CLONE_PATTERN`, restated where the destructive
/// statement is: the literal prefix `egret_t_` and **exactly** 32 hex digits.
/// Anchored on both ends on purpose — `LIKE 'egret_t_%'` is what once dropped
/// the shared `egret_test` in this repo, because `_` is a single-character
/// wildcard in `LIKE` and the pattern matches the very database it was written
/// to protect.
fn is_process_clone(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("egret_t_") else {
        return false;
    };
    rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The guard above, driven in **both** directions.
///
/// It exists because a guard nobody has seen fail is a guard nobody has seen:
/// `is_process_clone` is checked against a database that is always a clone
/// today, so on the live path it can only ever return `true`, and a mutation
/// that made it return `true` unconditionally would change nothing anybody
/// could observe. Here it is fed the names that must be refused, `egret_test`
/// first — the shared database the ruling is about, and the one an earlier
/// helper in this repo actually dropped.
#[test]
fn the_truncate_is_confined_to_this_process_own_clone() {
    assert!(
        is_process_clone("egret_t_000000000000000000000000000185c6"),
        "the measured shape of a real clone, taken from a probe rather than \
         from the pattern"
    );
    for refused in [
        "egret_test",
        "egret",
        "egret_t_",
        "egret_t_0",
        "egret_t_000000000000000000000000000185c",
        "egret_t_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "xegret_t_000000000000000000000000000185c6",
    ] {
        assert!(
            !is_process_clone(refused),
            "{refused:?} is not this process's clone and must not be TRUNCATEd"
        );
    }
}

/// **Every test that calls this runs on the MULTI-THREAD runtime** (W9a
/// Task 11), because whichever of them gets here first drives the whole
/// corpus on its own runtime: a `Loop` whose `values:` is a `kestra:///` file
/// reads it on a render thread that blocks on the runtime handle, which only
/// the multi-thread runtime -- production's -- serves
/// (`storage::scope::Snapshot::read_bytes` refuses a current-thread one).
/// `valids/loop-values-from-uri` and `valids/loop-outputs-auto` are those
/// two entries.
async fn egret_side() -> &'static Side {
    static SIDE: tokio::sync::OnceCell<Side> = tokio::sync::OnceCell::const_new();
    SIDE.get_or_init(|| async {
        let db = common::db()
            .await
            .expect("the oracle now drives a REAL executor and needs DATABASE_URL");
        let here: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&db.pool)
            .await
            .expect("read the current database");
        assert!(
            is_process_clone(&here),
            "refusing to TRUNCATE {here:?}: this statement is only ever safe against THIS PROCESS's \
             own `egret_test_db` clone, which is named `egret_t_` plus the pid as 32 hex digits and \
             is dropped when the process exits. A shared database would not flake under a \
             concurrent run — it would lose rows that run had written."
        );
        sqlx::query("TRUNCATE workflow.executions CASCADE")
            .execute(&db.pool)
            .await
            .expect("empty the execution tables");
        let (ws, user) = common::seed_workspace(&db).await;
        // W6 Task 1: the corpus goes into `workflow.flows` BEFORE any entry
        // runs, because ten corpus flows name a target that sorts later than
        // they do. See `driver::publish_corpus`.
        driver::publish_corpus(&db.pool, ws, user).await;

        let mut out = Side::default();
        for entry in golden() {
            let guard_before = egret_workflow::worker::CREATE_BESIDE_A_CREATED_SIBLING
                .load(std::sync::atomic::Ordering::Relaxed);
            let source = corpus_source(&entry.id);
            let inputs = corpus_inputs(&entry.id);
            let pool = db.pool.clone();
            // Spawned, so that a panic anywhere under `driver::run` becomes one
            // entry's `Broken` rather than a test crash that takes the other
            // 293 with it. `catch_unwind` cannot wrap a future that holds a
            // `&PgPool` across an await; a join handle can.
            let joined = tokio::spawn(async move {
                let reg = driver::registry(&pool);
                driver::run(&pool, ws, &reg, &source, inputs).await
            })
            .await;
            let outcome = match joined {
                Ok(driver::Ran::Trace(t)) => Outcome::Trace(*t),
                Ok(driver::Ran::Refused(msg)) => Outcome::Refused(msg),
                Ok(driver::Ran::Broken(msg)) => Outcome::Broken(msg),
                Err(e) if e.is_panic() => {
                    Outcome::Broken(format!("PANIC: {}", panic_message(e.into_panic())))
                }
                Err(e) => Outcome::Broken(format!("the driver task failed: {e}")),
            };
            let guard_after = egret_workflow::worker::CREATE_BESIDE_A_CREATED_SIBLING
                .load(std::sync::atomic::Ordering::Relaxed);
            if guard_after > guard_before {
                out.guard
                    .insert(entry.id.clone(), guard_after - guard_before);
            }
            out.outcomes.insert(entry.id.clone(), outcome);
        }
        out
    })
    .await
}

/// egret's whole side of the corpus: the trace per entry, plus the diagnostic
/// counters the run produced.
#[derive(Default)]
struct Side {
    outcomes: BTreeMap<String, Outcome>,
    /// Per entry, how many times `advance()` was asked to create a run beside a
    /// sibling still `CREATED` — the state Kestra guards against and W3a
    /// deleted the guard for. See
    /// [`egret_workflow::worker::CREATE_BESIDE_A_CREATED_SIBLING`].
    guard: BTreeMap<String, usize>,
}

/// egret's answer for one entry.
fn evaluate<'a>(side: &'a Side, entry: &GoldenEntry) -> &'a Outcome {
    side.outcomes
        .get(&entry.id)
        .unwrap_or_else(|| panic!("{} was not driven", entry.id))
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
#[tokio::test(flavor = "multi_thread")]
async fn executor_matches_golden() {
    let side = egret_side().await;
    let golden = golden();
    let baseline = baseline();

    let mut mismatched: BTreeSet<String> = BTreeSet::new();
    let mut panicked: Vec<String> = Vec::new();
    let mut detail: BTreeMap<String, String> = BTreeMap::new();
    let mut uncompared_pairs = 0usize;

    for entry in &golden {
        match (entry.verdict, evaluate(side, entry)) {
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
            (Verdict::Runs, Outcome::Trace(actual)) => {
                let mut actual = actual.clone();
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
         the_golden_has_no_relation_for_exactly_these_pairs)",
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
// W6 Task 4: one test per entry the replay banks
// --------------------------------------------------------------------------

/// The chain a `CREATE_NEW_EXECUTION` retry produces, asserted **shape first**
/// so a failure says which link broke.
///
/// `executor_matches_golden` already compares these four entries whole; it
/// reports `expected … / actual …` over an entire nested trace, which for a
/// three-execution chain is a paragraph a reader has to diff by eye. These
/// assert the three facts the chain turns on, in order — the number of links,
/// the key each link carries, and where the chain STOPS — and only then fall
/// back to the whole-trace equality that the ratchet itself asserts.
///
/// **The last of the three is the one that needed W6 Task 4's migration.** A
/// chain that never stops is what egret would produce with the metadata
/// missing: `next_retry_at` read `(1, Tick(0))` for this behaviour, so
/// `maxAttempts` compared 1 against 3 for ever and `maxDuration` measured from
/// each execution's own creation, which never accumulates.
async fn a_replay_chain_of_three(id: &str, policy: &str) {
    let side = egret_side().await;
    assert!(
        !baseline().contains(id),
        "{id} is still in baseline.txt; this test asserts what it banks"
    );
    let entry = golden()
        .into_iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("{id} is in the golden"));
    let expected = Trace::from(&entry);
    let Outcome::Trace(actual) = evaluate(side, &entry) else {
        panic!("{id}: the driver did not produce a trace");
    };
    let mut actual = actual.clone();
    actual.restrict_order_to(&expected);

    // Link 1: this execution ends RETRIED and carries exactly one child.
    let mut node = &actual;
    let mut depth = 0usize;
    while let Some(child) = node.spawned.first() {
        assert_eq!(
            node.execution.last().map(String::as_str),
            Some("RETRIED"),
            "{id} ({policy}): an execution that starts a replay must end              RETRIED, and this one at depth {depth} did not: {:?}",
            node.execution
        );
        assert_eq!(
            node.spawned.len(),
            1,
            "{id} ({policy}): a `CREATE_NEW_EXECUTION` retry chains one \
             replacement at a time -- the artifact never fans one out (0 of \
             the 40 NULL-`loopIndex` descendants in `traces.json` shares a \
             parent with another). Depth {depth} has {}",
            node.spawned.len()
        );
        assert_eq!(
            (child.parent_run, child.value.as_deref()),
            (None, None),
            "{id} ({policy}): a replay carries neither a parent run nor a loop \
             value -- that combination is what migration 0138 made \
             representable and what `read_trace_inner`'s decode had to widen \
             for. Depth {depth}"
        );
        node = child;
        depth += 1;
    }

    // Link 2: where it stops.
    assert_eq!(
        depth, 2,
        "{id} ({policy}): the chain is three executions -- the original and \
         two replays. {depth} replay(s) is a policy that stopped at the wrong \
         attempt, and an unbounded one is the driver's step bound rather than \
         this assertion"
    );
    assert_eq!(
        node.execution.last().map(String::as_str),
        Some("FAILED"),
        "{id} ({policy}): the LAST execution of the chain exhausts the policy, \
         so it ends FAILED with no RETRIED and no child. This is the assertion \
         the metadata exists for: {:?}",
        node.execution
    );

    // And the whole trace, which is what the ratchet compares.
    assert_eq!(
        actual,
        expected,
        "{id} ({policy}): the chain shape is right and something else \
         differs.\n  expected {}\n  actual   {}",
        expected.display(),
        actual.display()
    );
}

/// `maxAttempts: 3` on the FLOW — `ExecutionService.nextRetryDate(retry,
/// execution)`, which reads `execution.getMetadata().getAttemptNumber()`.
#[tokio::test]
async fn valids_retry_new_execution_flow_attempts() {
    a_replay_chain_of_three(
        "valids/retry-new-execution-flow-attempts",
        "flow-level, maxAttempts: 3, interval: PT1S",
    )
    .await;
}

/// `maxDuration: PT6S` on the FLOW, with `interval: PT2S` — the other guard in
/// the same method, measured from `getOriginalCreatedDate()`.
///
/// **This is the entry that says the offset accumulates.** Three executions at
/// two-second intervals put the fourth retry's date past the original creation
/// plus six seconds; an origin that reset each time would put it at two.
#[tokio::test]
async fn valids_retry_new_execution_flow_duration() {
    a_replay_chain_of_three(
        "valids/retry-new-execution-flow-duration",
        "flow-level, maxDuration: PT6S, interval: PT2S",
    )
    .await;
}

/// `maxAttempts: 3` on the TASK — a different Kestra method
/// (`TaskRun.nextRetryDate(retry, execution)`, `TaskRun.java:310`) reading the
/// same two metadata fields, which is why the corpus carries both.
#[tokio::test]
async fn valids_retry_new_execution_task_attempts() {
    a_replay_chain_of_three(
        "valids/retry-new-execution-task-attempts",
        "task-level, maxAttempts: 3, interval: PT1S",
    )
    .await;
}

/// `maxDuration: PT6S` on the TASK, with `interval: PT2S`.
#[tokio::test]
async fn valids_retry_new_execution_task_duration() {
    a_replay_chain_of_three(
        "valids/retry-new-execution-task-duration",
        "task-level, maxDuration: PT6S, interval: PT2S",
    )
    .await;
}

/// The whole population of ordered pairs the golden declines to assert — 13,
/// across three flows.
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
            ("valids/dag".to_string(), 2),
            ("valids/finally-parallel".to_string(), 1),
            ("valids/if".to_string(), 10),
            ("valids/parallel-nested".to_string(), 5),
        ]),
        "the whole population of pairs the golden declines to assert -- 18, which \
         is exactly the 18 `stability.json` records as dropped-for-flipping, in \
         these four flows. 13 in three flows until the WorkingDirectory Task 8 \
         re-harvest, which moved `dag` 1 -> 2, `if` 9 -> 10, `parallel-nested` \
         3 -> 5 and added `finally-parallel` 1: the same instability, measured \
         again on five new rounds, not a change in egret"
    );
    assert_eq!(dropped.values().sum::<usize>(), 18, "and they sum to 18");
}

/// **The 227 ordered pairs inside `spawned` are asserted now, and this is how
/// many of them egret actually produces.**
///
/// Until W3b Task 5 this test was the second of the three holes
/// `oracle-exec/README.md` published: of the 1104 kept ordered pairs, only the
/// **877** at the top level were ever asserted, because egret's `Trace::spawned`
/// was `Vec::new()` unconditionally and `Trace::restrict_order_to` descends
/// through `self.spawned.iter_mut().zip(&golden.spawned)` — a `zip` with an
/// empty left side, which never iterates. The old assertion was that **no**
/// comparable entry produced a `spawned` at all, and it was the thing that made
/// the 227 provably unreachable.
///
/// The driver runs the sub-executions, so the question changes: the golden-side
/// split cannot move and is still asserted, and beside it is how many entries
/// egret now produces a sub-execution trace for — which is what brings those
/// pairs into scope.
#[tokio::test(flavor = "multi_thread")]
async fn the_ordered_pairs_inside_spawned_are_reachable_now() {
    let side = egret_side().await;
    fn count(node: &Trace, depth: usize, top: &mut usize, nested: &mut usize) {
        if depth == 0 {
            *top += node.order.len();
        } else {
            *nested += node.order.len();
        }
        for sub in &node.spawned {
            count(sub, depth + 1, top, nested);
        }
    }

    let (mut top, mut nested) = (0usize, 0usize);
    let (mut egret_top, mut egret_nested) = (0usize, 0usize);
    let mut egret_spawned: Vec<String> = Vec::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let expected = Trace::from(entry);
        count(&expected, 0, &mut top, &mut nested);
        if let Outcome::Trace(actual) = evaluate(side, entry) {
            let mut actual = actual.clone();
            actual.restrict_order_to(&expected);
            count(&actual, 0, &mut egret_top, &mut egret_nested);
            if !actual.spawned.is_empty() {
                egret_spawned.push(entry.id.clone());
            }
        }
    }

    println!(
        "kept ordered pairs: {top} at the top level, {nested} inside `spawned`; \
         egret produces {egret_top} + {egret_nested} over {} entries with a \
         `spawned`",
        egret_spawned.len()
    );
    assert_eq!(
        (top, nested, top + nested),
        (889, 227, 1116),
        "the golden-side split, which nothing egret does can move. 1104 is the \
         number of pairs the harvest KEPT and 877 the number the harness could \
         assert; the difference was the sub-execution hole measured in the unit \
         the order table is written in"
    );
    assert_eq!(
        egret_spawned.len(),
        ENTRIES_WHERE_EGRET_PRODUCES_A_SPAWNED,
        "the hole's other side: how many comparable entries egret now produces \
         a sub-execution trace for. It was **0** for the whole of W3a and W3b \
         Tasks 1-4, which is why those 227 pairs were unreachable rather than \
         merely unasserted"
    );
    assert_eq!(
        (egret_top, egret_nested),
        (EGRET_TOP_LEVEL_PAIRS, EGRET_NESTED_PAIRS),
        "and how many pairs egret produces on each side after the projection. \
         The nested figure was 0 by construction before this task"
    );
}

/// 39 through W9a Task 10; **41 since W9a Task 11** -- every entry with a
/// `spawned`: the two whose `Loop` iterates an ION file were the last two.
///
/// **38 since no-silent-stall Task 1**: three entries with a `spawned` are
/// refused at start now (`RUNS_REFUSED`), so egret produces no trace for them
/// at all.
// 38 -> 40 at WorkingDirectory Task 8: `valids/working-directory-taskrun` and
// `-nested`, the two entries whose one `Loop` body is a WorkingDirectory, which
// egret now plans and runs (measured by the census, not by hand).
const ENTRIES_WHERE_EGRET_PRODUCES_A_SPAWNED: usize = 40;
// +15 at W6 Task 3: the `Subflow` entries that used to stall at their first
// `Subflow` run now finish, so the runs after it exist and have endpoints.
// The nested figure does not move -- a `Subflow` child is linked by
// `trigger_execution_id` and is deliberately invisible to `spawned`.
/// **580 through W8 Task 2, 587 after W8 Task 3, 613 now.** Task 3's eight
/// entries carried 7 top-level ordered pairs between them; W8 Task 4's +26 is
/// not a bank at all but a CONSTRUCT: `LoopUntil`'s nine entries used to stall
/// at their loop run, and a run that never terminates has no right endpoint and
/// so contributes no pair. Their bodies run now, banked or not. Neither task
/// moves the nested figure -- no `LoopUntil` entry carries a `spawned`.
///
/// **625 since W9a Task 11**: `valids/loop-values-from-uri`'s one pair and
/// `valids/loop-outputs-auto`'s three, whose `Loop` used to fail at `CREATED`
/// and now runs (the second still fails at `end`, on the `Loop`'s own
/// `outputs:`, which egret does not produce).
///
/// **623 and 121 since no-silent-stall Task 1**: the entries it refuses at
/// start (`RUNS_REFUSED`) produce no trace, so the pairs they produced are
/// gone from both sides.
/// **704 since the AllowFailure flowable**: the 81 top-level pairs of its three targets.
/// **718 and 125 since WorkingDirectory Task 8**: egret runs the six harvested
/// `working-directory-real-*` flows and `retry-failed-task-duration` (their
/// pairs are on the top level and inside them), and `working-directory-taskrun`
/// and `-nested` are now run and compared (see `the_ordered_pairs_inside_spawned`).
const EGRET_TOP_LEVEL_PAIRS: usize = 718;
const EGRET_NESTED_PAIRS: usize = 125;

/// **KEPT is not ASSERTED, and the line between them is the baseline rather
/// than the nesting** — W3b Task 7.
///
/// W3a published *"877 asserted, 227 never"* and the 227 were right for a
/// structural reason: [`Trace::restrict_order_to`] descends through
/// `self.spawned.iter_mut().zip(&golden.spawned)` and egret's `spawned` was
/// `Vec::new()`, so that `zip` never iterated. W3b Task 5 fills `spawned`, so
/// the reason is gone — and the replacement is **not** 1104, because *compared*
/// and *asserted* are different claims. A pair on a **baselined** entry is
/// compared and cannot be falsified: the entry mismatches whatever that pair
/// says, so changing it changes nothing the ratchet reports.
///
/// That is the same class of over-claim W3a's coverage statement made and the
/// whole-branch review caught — *"the kept 1104 are the whole of what this
/// oracle asserts about serialisation"*, when 227 of them could not fail.
///
/// **Measured rather than reasoned**, by flipping `before` ⇄ `overlapping` in
/// `golden.json` and reading the ratchet, with the file restored from a
/// snapshot and its md5 (`5773748d22d521e6a65afc5e2e2663d1`) verified after
/// every run:
///
/// | flipped | pairs | ratchet |
/// |---|---|---|
/// | every **nested** pair — W3a's own experiment | 227 | **FAILS**: 150 mismatching, 7 REGRESSIONs |
/// | every pair on a **baselined** entry | 747 | green: 143 mismatching, 0 banked |
/// | every pair on a **matching** entry | 357 | **FAILS**: 184 mismatching, 41 REGRESSIONs |
///
/// The first row is the one to read against W3a's, which recorded that flipping
/// all 227 left this crate green.
///
/// **This table had a stale twin under it for a whole package, and it was in
/// this file.** It read 750 / 354 / 144 / 6 / 40 — the values before the
/// whole-branch review's fix round banked `valids/replay-loop` — fifty lines
/// above an `assert_eq!` that has been deriving 747 and 357 the whole time.
/// W4 Task 6's sweep corrected the copy in `oracle-exec/README.md` and listed
/// the copy in `runnable.txt` as retired, in the same commit that edited **this
/// file**, and did not check the copy it was editing. The rule that missed it
/// was *"every figure a test asserts was already right"*: true, and too narrow.
/// **A value a test compares is safe; prose that merely sits beside one is not,
/// even inside a test file** — a doc comment and an assertion *message* are
/// prose, and the message under the `(622, 125, 747)` assertion said *"flipping
/// all 750 … at 144"* while the tuple beside it said otherwise.
///
/// This test derives the split from `golden.json` and `baseline.txt` alone —
/// no driver — so it is cheap enough to run everywhere and cannot drift from
/// the ratchet's own population.
#[test]
fn kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line() {
    fn count(node: &GoldenEntry, top: &mut usize, nested: &mut usize) {
        *top += node.order.len();
        fn deeper(subs: &[GoldenSpawned], nested: &mut usize) {
            for s in subs {
                *nested += s.order.len();
                deeper(&s.spawned, nested);
            }
        }
        deeper(&node.spawned, nested);
    }

    let baseline = baseline();
    let (mut m_top, mut m_nested) = (0usize, 0usize);
    let (mut b_top, mut b_nested) = (0usize, 0usize);
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let (mut top, mut nested) = (0usize, 0usize);
        count(entry, &mut top, &mut nested);
        if baseline.contains(&entry.id) {
            b_top += top;
            b_nested += nested;
        } else {
            m_top += top;
            m_nested += nested;
        }
    }

    println!(
        "kept ordered pairs: {} asserted ({m_top} top + {m_nested} nested), \
         {} not ({b_top} top + {b_nested} nested)",
        m_top + m_nested,
        b_top + b_nested
    );
    assert_eq!(
        (m_top, m_nested, m_top + m_nested),
        // + the 81 top-level pairs of the three AllowFailure targets (45 + 15 + 21),
        // which now live on matching entries.
        (394, 103, 497),
        "the pairs a change to which the ratchet REPORTS: they live on entries \
         that match, so any change to one makes its entry a REGRESSION. W6 Task \
         3 moved 3 of them across the line, all top-level: the 12 `Subflow` \
         entries it banked carry 3 ordered pairs between them. W9a Task 11 moved \
         1 more: `valids/loop-values-from-uri`'s `write` before `loop`"
    );
    assert_eq!(
        (b_top, b_nested, b_top + b_nested),
        // 579 + 125 = 704 until the AllowFailure flowable banked its three targets,
        // whose 81 top-level pairs now live on matching entries.
        // 623 -> 619 baselined pairs at WorkingDirectory Task 8: banking
        // `valids/working-directory-taskrun` moved its pairs to the matching side,
        // and the re-harvest changed the pairs of four flows (see
        // `the_golden_has_no_relation_for_exactly_these_pairs`).
        (495, 124, 619),
        "and the pairs a change to which it does not: they live on baselined \
         entries, which mismatch whatever the pair says. Flipping all 704 leaves \
         the ratchet green and unmoved -- at 131 when W6 Task 3 flipped them, at \
         127 since Task 4's four banks, at 109 since W8 banked eighteen, and at \
         108 since W9a Task 11 banked `valids/loop-values-from-uri`; the \
         claim is `unmoved`, and the number beside it is a moment. **This message \
         said 744 while the tuple three lines above it said 705** -- the exact \
         shape this test's own doc warns about, found by W8 Task 7's sweep"
    );
    assert_eq!(
        (
            m_top + b_top,
            m_nested + b_nested,
            m_top + m_nested + b_top + b_nested
        ),
        (889, 227, 1116),
        "the total asserted beside the parts, and the same 877 / 227 / 1104 \
         `the_ordered_pairs_inside_spawned_are_reachable_now` derives from the \
         other direction"
    );
}

/// **The baselining argument is not about the ordered pairs; it is about every
/// field, and this is the counterpart the coverage statement did not have.**
///
/// `executor_matches_golden` reports exactly two things about an entry: a
/// **non-baselined** mismatch (a REGRESSION) and a **baselined** match
/// (PROGRESS NOT BANKED). So on a baselined entry every golden field is
/// un-falsifiable in precisely the way
/// [`kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line`]
/// shows the pairs to be: change it, the entry still mismatches, the ratchet
/// still says nothing. That statement published *"three quarters of the kept
/// ordered pairs cannot fail"* and left the reader to assume the pairs were
/// special. They are not; they are the family somebody happened to measure.
///
/// Four families, each derived from `golden.json` and `baseline.txt` alone —
/// no driver, so this is cheap and cannot drift from the ratchet's population:
///
/// - **run terminals**, the last state of every run, sub-executions included —
///   the population the deleted `the_feed_absorbs_a_measured_share_of_the_golden_run_terminals`
///   used to classify (retired with W3a's harness — see the README's pointer
///   table), and the one the fed count is a share of;
/// - **run-history tokens**, every state in every one of those runs;
/// - **sub-executions**, the nodes under `spawned` at any depth;
/// - **executions**, i.e. the golden `runs` entries plus those sub-executions,
///   which is the population whose `execution` history is compared;
/// - **execution-history tokens**, every state in those execution histories.
///   Kept apart from the run-history family because "history tokens" naming two
///   different populations is how a number ends up meaning two things.
///
/// The total is asserted beside the parts in each, and the split is the same
/// line the pairs' is: an entry in `baseline.txt` mismatches whatever any of
/// these says.
#[test]
fn the_baselining_argument_covers_every_field_and_not_only_the_ordered_pairs() {
    #[derive(Default)]
    struct Counts {
        terminals: usize,
        sub_executions: usize,
        executions: usize,
        /// Every state in every RUN's history, which is the field family the
        /// `runs` comparison is made of.
        history: usize,
        /// And every state in every EXECUTION's history, which is a different
        /// family and a different comparison.
        execution_history: usize,
    }
    fn walk(execution: &[String], runs: &[GoldenRun], spawned: &[GoldenSpawned], c: &mut Counts) {
        c.executions += 1;
        c.execution_history += execution.len();
        for r in runs {
            if !r.state.is_empty() {
                c.terminals += 1;
            }
            c.history += r.state.len();
        }
        for sub in spawned {
            c.sub_executions += 1;
            walk(&sub.execution, &sub.runs, &sub.spawned, c);
        }
    }

    let baseline = baseline();
    let (mut banked, mut held) = (Counts::default(), Counts::default());
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let c = if baseline.contains(&entry.id) {
            &mut held
        } else {
            &mut banked
        };
        walk(&entry.execution, &entry.runs, &entry.spawned, c);
    }

    println!(
        "un-falsifiable because their entry is baselined: {} of {} run terminals, \
         {} of {} sub-executions, {} of {} executions, {} of {} run-history \
         tokens, {} of {} execution-history tokens",
        held.terminals,
        held.terminals + banked.terminals,
        held.sub_executions,
        held.sub_executions + banked.sub_executions,
        held.executions,
        held.executions + banked.executions,
        held.history,
        held.history + banked.history,
        held.execution_history,
        held.execution_history + banked.execution_history,
    );

    assert_eq!(
        (
            held.terminals,
            banked.terminals,
            held.terminals + banked.terminals
        ),
        (
            TERMINALS_ON_BASELINED_ENTRIES,
            858 - TERMINALS_ON_BASELINED_ENTRIES,
            858
        ),
        "run terminals. 858 is the same population the fed table classifies, so \
         the two sections are talking about one set of runs"
    );
    assert_eq!(
        (
            held.sub_executions,
            banked.sub_executions,
            held.sub_executions + banked.sub_executions
        ),
        (
            SUB_EXECUTIONS_ON_BASELINED_ENTRIES,
            142 - SUB_EXECUTIONS_ON_BASELINED_ENTRIES,
            142
        ),
        "sub-executions, and 142 is the count \
         `the_sub_executions_are_compared_and_this_is_what_they_cost` derives \
         from the other direction"
    );
    assert_eq!(
        (
            held.executions,
            banked.executions,
            held.executions + banked.executions
        ),
        (
            EXECUTIONS_ON_BASELINED_ENTRIES,
            // 408 -> 415: the seven harvested entries add seven top-level executions.
            415 - EXECUTIONS_ON_BASELINED_ENTRIES,
            415
        ),
        "executions: the 266 `runs` entries plus the 142 sub-executions they \
         carry, and the baselined side is the 143 baselined `runs` entries plus \
         the sub-executions under them"
    );
    assert_eq!(
        (held.history, banked.history, held.history + banked.history),
        (
            HISTORY_TOKENS_ON_BASELINED_ENTRIES,
            // 3355 -> 3419: the harvested entries' run histories (64 tokens).
            3419 - HISTORY_TOKENS_ON_BASELINED_ENTRIES,
            3419
        ),
        "every state token in every RUN history -- the family the `runs` \
         comparison is made of"
    );
    assert_eq!(
        (
            held.execution_history,
            banked.execution_history,
            held.execution_history + banked.execution_history
        ),
        (
            EXECUTION_HISTORY_TOKENS_ON_BASELINED_ENTRIES,
            // 1462 -> 1489: the harvested flows' execution histories, 27 tokens
            // (six WorkingDirectory flows at three each, and the retry at nine).
            1489 - EXECUTION_HISTORY_TOKENS_ON_BASELINED_ENTRIES,
            1489
        ),
        "and every state token in every EXECUTION history, which is a different \
         family with its own comparison -- carried separately so `history \
         tokens` cannot mean two things"
    );
    // The cross-check that stops this from being four numbers nobody joined up:
    // the executions are the entries plus their sub-executions, both halves of
    // which are derived elsewhere in this file.
    assert_eq!(
        held.executions + banked.executions,
        // 266 -> 273 `runs` entries at WorkingDirectory Task 8 (the seven harvested
        // entries); the 142 sub-executions are unchanged.
        273 + 142,
        "415 = the comparable `runs` entries + the sub-executions they carry"
    );
}

/// Whether an execution ended mid-flight — no terminal state on its history.
///
/// **A function rather than three lines inline**, so the emptiness assertion
/// above has a positive control. `matched_while_stalled` is empty today and a
/// mutation that moves the collection back below the `continue` therefore kills
/// nothing: an assertion over an empty population answers the same way whichever
/// population it is over. That is the exact shape this programme calls a guard
/// nobody has seen, so the classifier is driven directly by
/// [`a_stalled_execution_that_also_matched_would_be_counted`].
fn is_stalled(execution: &[String]) -> bool {
    !execution
        .last()
        .is_some_and(|s| TERMINAL.contains(&s.as_str()))
}

/// The states this classifier reads as "the execution ended".
///
/// **`RETRIED` was missing and its absence was a defect, not a conservatism** —
/// W6 Task 4. `RunState::Retried`'s own doc says *"terminal"*, Kestra's
/// `State.isTerminated()` lists it, `worker::claim::settle` dispatches on
/// `is_terminal()` and therefore reports a `RETRIED` execution to its parent,
/// and `worker::spawn::terminated_count` counts one — so every other reader of
/// the word in this repository already agreed, and this one list did not. It
/// put the four `retry-new-execution-*` root executions into
/// [`ENTRIES_LEFT_UNFINISHED`], whose assertion says every member is *a task
/// type this worker has no handler for*: those four declare only
/// `io.kestra.plugin.core.execution.Fail`, which **has** a handler, and they
/// ran it.
///
/// It is spelled as a list beside the classifier rather than inline so that
/// this doc has somewhere to live and so the positive control
/// ([`a_stalled_execution_that_also_matched_would_be_counted`]) iterates the
/// same list the classifier reads — a control that hard-coded its own copy
/// could pass while the classifier used a different one.
///
/// `SKIPPED` and `RESUBMITTED` are in Kestra's list and not here;
/// **`CANCELLED` joined it at W8 Task 6**.
/// [`the_terminal_list_covers_every_execution_terminal_the_golden_holds`]
/// derives why, and it has now corrected this doc twice. The first answer said
/// all three ended no golden EXECUTION history; `CANCELLED` ends **three**, and
/// the second answer was that this classifier reads EGRET's traces, where
/// `RunState` had no `Cancelled`, so the arm was one nothing could take. W8
/// Task 6 added the variant and with it a producer, so the arm is reachable and
/// the omission would have called three entries stalled — the guard in that
/// test is what said so, in the commit that added the variant.
const TERMINAL: &[&str] = &[
    "SUCCESS",
    "FAILED",
    "KILLED",
    "WARNING",
    "RETRIED",
    "CANCELLED",
];

/// **The positive control for `matched_while_stalled`.**
///
/// The over-acceptance that count exists to bound is *an execution egret left
/// mid-flight that nevertheless matched the golden* — which is only reachable if
/// Kestra's own trace also ended mid-flight. No such entry exists, so the
/// assertion is over an empty set, and an empty set cannot tell a working
/// classifier from a broken one. This drives the classifier on both answers.
#[test]
fn a_stalled_execution_that_also_matched_would_be_counted() {
    let ended = |s: &str| vec!["CREATED".to_string(), "RUNNING".to_string(), s.to_string()];
    for terminal in TERMINAL {
        assert!(
            !is_stalled(&ended(terminal)),
            "{terminal} is a terminal state and must not count as stalled"
        );
    }
    assert!(
        TERMINAL.contains(&"RETRIED"),
        "the arm W6 Task 4 added, asserted by name: a `CREATE_NEW_EXECUTION` \
         retry ends its execution RETRIED and starts a replacement, which is \
         the opposite of stalling. Iterating TERMINAL above cannot say this on \
         its own -- an empty list would pass that loop"
    );
    assert!(
        is_stalled(&[
            "CREATED".to_string(),
            "RUNNING".to_string(),
            "RETRYING".to_string()
        ]),
        "RETRYING is the OTHER retry behaviour and is NOT terminal: the run is \
         waiting for its next attempt inside this execution. This is the \
         two-sided half of the RETRIED arm -- a classifier that took any state \
         starting `RETR` would pass every other assertion here"
    );
    assert!(
        is_stalled(&["CREATED".to_string(), "RUNNING".to_string()]),
        "an execution still RUNNING is stalled -- this is the arm that would be \
         collected, and with the collection below the `continue` a MATCHING \
         entry in this state was excluded by construction"
    );
    assert!(
        is_stalled(&[]),
        "and so is one with no history at all, which is what a driver failure \
         before the first settle would leave"
    );
}

/// **What [`TERMINAL`] leaves out, derived from the golden rather than from
/// Kestra's enum — and the derivation corrected the first answer.**
///
/// Kestra's `State.isTerminated()` also lists `SKIPPED`, `CANCELLED` and
/// `RESUBMITTED`. This test was written asserting that none of the three ever
/// ends an EXECUTION history in this golden, and **`CANCELLED` does**: three
/// entries end in it (`valids/exit-cancelled`,
/// `valids/flow-concurrency-sla-assert-cancel`, `valids/sla-execution-condition`),
/// which is exactly the kind of remembered fact this programme keeps getting
/// wrong and the reason the population is derived here instead.
///
/// **It IS in [`TERMINAL`] since W8 Task 6, and the guard that put it there is
/// the point of this paragraph.** [`is_stalled`] classifies EGRET's traces,
/// never the golden's, so while `RunState` had no `Cancelled` an arm for it was
/// one nothing could take — and that was asserted here rather than argued, as
/// *"if `RunState` ever gains the variant, this test fails and `TERMINAL` has
/// to grow with it"*. It gained the variant, this test failed, and `TERMINAL`
/// grew. The assertion below is now the other direction: every state that ends
/// a golden execution is one egret can produce and one this classifier knows.
#[test]
fn the_terminal_list_covers_every_execution_terminal_the_golden_holds() {
    fn walk(node: &Trace, out: &mut BTreeSet<String>) {
        if let Some(last) = node.execution.last() {
            out.insert(last.clone());
        }
        for sub in &node.spawned {
            walk(sub, out);
        }
    }
    let mut seen = BTreeSet::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        walk(&Trace::from(entry), &mut seen);
    }
    println!("execution terminals in the golden: {seen:?}");
    assert_eq!(
        seen,
        BTreeSet::from([
            "CANCELLED".to_string(),
            "FAILED".to_string(),
            "KILLED".to_string(),
            "RETRIED".to_string(),
            "SUCCESS".to_string(),
            "WARNING".to_string(),
        ]),
        "every state a golden EXECUTION history ends in. `RETRIED` is in it, \
         which is what makes `is_stalled`'s omission of it a defect rather \
         than an unused arm"
    );
    // **Every state the golden ends an execution in is now one `RunState` can
    // hold**, which was false for `CANCELLED` until W8 Task 6 and is the reason
    // the filter this loop used to carry is gone.
    for state in seen.iter() {
        assert!(
            RunState::ALL.iter().any(|s| s.wire() == state.as_str()),
            "{state} ends a golden execution and `RunState` cannot hold it; \
             `is_stalled` reads EGRET's traces, so an arm for it would be one \
             nothing could take and it must NOT be added to TERMINAL until a \
             producer exists"
        );
        assert!(
            TERMINAL.contains(&state.as_str()),
            "{state} ends a golden execution, egret can produce it, and \
             `is_stalled` would call it stalled"
        );
    }
    for state in TERMINAL {
        assert!(
            seen.contains(*state),
            "{state} is in TERMINAL and ends no golden execution: an arm \
             nothing can take"
        );
    }
    for unused in ["SKIPPED", "RESUBMITTED"] {
        assert!(
            !seen.contains(unused),
            "{unused} is terminal in Kestra's enum; if the golden now ends an \
             execution in it, TERMINAL has to grow and this list has to say so"
        );
    }
}

/// The counts above, named so the coverage statement and this file cannot
/// disagree about them.
// **All five fell when W6 Task 3 banked 12 entries**, and they fell together
// because they are five projections of one set: the runs, sub-executions,
// executions and history tokens that live on a baselined entry. The 12 carry 16
// run terminals, 2 sub-executions (`valids/loop-with-subflow`'s), 14 executions
// (12 entries + those 2), 48 run-history tokens and 42 execution-history tokens
// between them, and every one of those is falsifiable now instead of held.
//
// **All five fell again at W6 Task 4, by the same arithmetic over 4 entries**:
// 12 run terminals, 8 sub-executions (two replays each), 12 executions (4
// entries + those 8), 56 run-history tokens and 44 execution-history tokens.
// The shape of a `retry-new-execution-*` entry is what makes those numbers
// bigger per entry than Task 3's: three executions of one flow, each with one
// run, and the run histories are five states long
// (`CREATED SUBMITTED RUNNING FAILED RETRIED`).
/// **433 through W8 Task 2, 419 after W8 Task 3, 412 after W8 Task 4, 396 since
/// W8 Task 5** -- Task 3's 14 run terminals, the 7 `valids/waitfor-nested`
/// carries, and W8 Task 5's **16** over its seven banked entries, which leave
/// the un-falsifiable half for the reported half. 840 is unchanged, because
/// banking moves a run across the line rather than creating one.
///
/// **W8 Task 6 banked two entries and did NOT move this number**, which is the
/// only figure in this group it left alone: both of its entries have `"runs":
/// []`, so they carried no run terminal to move. A task whose banks are
/// invisible here is worth a sentence, because the next reader will otherwise
/// read the unmoved figure as a stale one.
///
/// **W9a Task 11 moved all five**, by banking `valids/loop-values-from-uri`
/// (its `Loop` iterates an ION file now): 5 run terminals (`write`, `loop` and
/// the three iterations' `item`), 3 sub-executions, 4 executions, 19
/// run-history tokens and 12 execution-history tokens.
/// **368 since the AllowFailure flowable banked three entries** (23 run terminals:
/// 10 + 6 + 7).
/// **365 since WorkingDirectory Task 8 banked `valids/working-directory-taskrun`**
/// (its three run terminals: `parallel`, `workingDir`, `log-taskrun`), and the
/// run terminals of the golden are 858 = 840 + the 18 the harvest added.
/// **366 since `valids/retry-failed-task-duration` was baselined** (its one
/// run terminal, `fail`, after three attempts).
const TERMINALS_ON_BASELINED_ENTRIES: usize = 366;
/// 43 from W6 Task 4 to W9a Task 10; **40 since W9a Task 11** -- the three
/// iterations of `valids/loop-values-from-uri`.
// 40 -> 39 at WorkingDirectory Task 8: `working-directory-taskrun` has one
// sub-execution, and it is banked.
const SUB_EXECUTIONS_ON_BASELINED_ENTRIES: usize = 39;
/// **165 through W8 Task 2, 157 after W8 Task 3, 156 after W8 Task 4, 149 after
/// W8 Task 5, 147 now** -- the eight `Pause` entries W8 Task 3 banked, the one
/// `LoopUntil` entry W8 Task 4 banked, W8 Task 5's seven and W8 Task 6's two.
/// None of the eighteen carries a sub-execution --
/// `SUB_EXECUTIONS_ON_BASELINED_ENTRIES` has not moved since W6 Task 4 -- so
/// each fall is exactly the entry count.
///
/// **143 since W9a Task 11**: `valids/loop-values-from-uri` and its three
/// iterations -- the first bank since W6 Task 4 to carry sub-executions.
/// **140 since the AllowFailure flowable** banked three entries that carry no
/// sub-execution.
// 140 -> 138 at WorkingDirectory Task 8: `working-directory-taskrun` (one top-level
// execution and one sub-execution) is banked.
// 138 -> 139: `valids/retry-failed-task-duration` is one top-level execution with no
// sub-executions, baselined at WorkingDirectory Task 9.
const EXECUTIONS_ON_BASELINED_ENTRIES: usize = 139;
/// **1703 through W8 Task 2, 1647 after W8 Task 3, 1610 after W8 Task 4, 1528
/// now** -- 56 run-history tokens on Task 3's eight, 37 on
/// `valids/waitfor-nested` (whose seven runs include three loops reading
/// `CREATED RUNNING PAUSED RUNNING PAUSED RUNNING SUCCESS`), and **82** on W8
/// Task 5's seven, which is high per entry because three of them retry: a
/// `task-allow-*-runnable` run history is thirteen tokens long.
///
/// **1509 since W9a Task 11**: 19 on `valids/loop-values-from-uri`.
/// **1414 since the AllowFailure flowable**: 95 on its three targets (23 runs).
/// **1405 since WorkingDirectory Task 8**: nine on `valids/working-directory-taskrun`'s
/// three runs (`CREATED RUNNING SUCCESS` each).
/// **1419 since `valids/retry-failed-task-duration` was baselined**: 14 on its one
/// run (`CREATED SUBMITTED RUNNING FAILED RETRYING`, three times, then `FAILED`).
const HISTORY_TOKENS_ON_BASELINED_ENTRIES: usize = 1419;
/// **665 through W8 Task 2, 617 after W8 Task 3, 562 after W8 Task 4, 526 after
/// W8 Task 5, 522 now** -- 48 execution-history tokens on Task 3's eight, i.e.
/// six apiece (every one of them reads `CREATED RUNNING PAUSED RESTARTED
/// RUNNING <terminal>`), 55 on `valids/waitfor-nested` alone (whose execution
/// alternates `PAUSED`/`RUNNING` 26 times), **36** on W8 Task 5's seven, and
/// **4** on W8 Task 6's two -- two apiece, because `["CREATED","CANCELLED"]` is
/// the shortest execution history in the golden: the violation fires before the
/// execution ever reaches `RUNNING`.
///
/// **510 since W9a Task 11**: three apiece on `valids/loop-values-from-uri`
/// and its three iterations.
/// **497 since the AllowFailure flowable**: 13 on its three targets (3 + 7 + 3).
/// **491 since WorkingDirectory Task 8**: six on `working-directory-taskrun`'s two
/// executions (`CREATED RUNNING SUCCESS` each).
/// **500 since `valids/retry-failed-task-duration` was baselined**: nine on its
/// one execution.
const EXECUTION_HISTORY_TOKENS_ON_BASELINED_ENTRIES: usize = 500;

/// **What the order restriction actually removes, driven through the real
/// driver — and egret still never says `after`.**
///
/// [`Trace::restrict_order_to`] drops the pairs the golden has no opinion
/// about. The `after` half is what makes that removal equivalent to *accepting
/// either relation Kestra was seen to produce* rather than to weakening the
/// rule: a pair the golden dropped is one Kestra was observed producing BOTH
/// values for, and with egret's domain measured at `{before, overlapping}` the
/// two statements coincide. It holds only while `after` is unreachable, which
/// is a property of a run's left endpoint being its creation step and creation
/// being monotonic in run order.
#[tokio::test(flavor = "multi_thread")]
async fn what_the_order_restriction_removes_is_derived_and_egret_never_says_after() {
    let side = egret_side().await;
    let mut removed: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_pairs = 0usize;
    let mut afters: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let expected = Trace::from(entry);
        let Outcome::Trace(actual) = evaluate(side, entry) else {
            continue;
        };
        let mut actual = actual.clone();
        fn walk(node: &Trace, total: &mut usize, afters: &mut bool) {
            *total += node.order.len();
            if node.order.iter().any(|(_, _, rel)| rel == "after") {
                *afters = true;
            }
            for sub in &node.spawned {
                walk(sub, total, afters);
            }
        }
        let mut said_after = false;
        walk(&actual, &mut total_pairs, &mut said_after);
        if said_after {
            afters.push(entry.id.clone());
        }
        let dropped = actual.restrict_order_to(&expected);
        if dropped > 0 {
            removed.insert(entry.id.clone(), dropped);
        }
    }

    println!("restriction removed {removed:?}; egret produced {total_pairs} ordered pair(s)");
    assert_eq!(
        afters,
        Vec::<String>::new(),
        "egret's relation domain must stay {{before, overlapping}} -- and this \
         now walks the SUB-EXECUTIONS too, which it could not before W3b Task 5 \
         because there were none. Task 6 named spawned executions as the thing \
         that could break it; they do not"
    );
    // **The entry count too, and W4 Task 6's review is why.** The coverage
    // statement's sentence is "80 pairs across 6 entries"; the 80 was asserted
    // here and the 6 was only `println!`ed above, which by that sweep's own
    // generalisation makes it the figure in that sentence that goes stale
    // next -- it is prose sitting beside a derivation rather than a value the
    // derivation compares. Asserted as a pair with the names, because a count
    // that moves without saying which entry left is the shape `baseline.txt`'s
    // header keeps getting wrong.
    assert_eq!(
        (removed.len(), removed.keys().cloned().collect::<Vec<_>>()),
        (
            RESTRICTION_REMOVES_ENTRIES,
            vec![
                "valids/all-flowable".to_string(),
                "valids/dag".to_string(),
                "valids/finally-parallel".to_string(),
                "valids/if".to_string(),
                "valids/parallel-nested".to_string(),
                "valids/restart-child".to_string(),
                "valids/restart-loop".to_string(),
                // W6 Task 3: three `Subflow` parents that used to stall at
                // their `Subflow` run and now run past it, producing pairs the
                // golden has no counterpart for -- exactly because they run a
                // successor Kestra never reached. All three are registered in
                // `STILL_OVER_ACCEPTED_AT_RUNTIME`, which is where the reason
                // lives; this list is where it is visible.
                "valids/restart-parent".to_string(),
                "valids/restart-parent-loop".to_string(),
                "valids/subflow-secret-output-parent".to_string(),
                // WorkingDirectory Task 8, from the re-harvested golden:
                // `working-directory-invalid-runif`, egret's extra FAILED run for
                // `s2`, has no counterpart in Kestra's list.
                "valids/working-directory-invalid-runif".to_string(),
            ]
        ),
        "the entries the restriction removes a pair from, by name and by count"
    );
    assert_eq!(
        removed.values().sum::<usize>(),
        RESTRICTION_REMOVES,
        "this is the number the ratchet prints; assert it so a widening of the \
         restriction's reach moves a test and not a log line. It was 15 under \
         W3a's harness, over top-level pairs only"
    );
    assert_eq!(
        total_pairs, EGRET_PAIRS_BEFORE_RESTRICTION,
        "the population the `after` claim is measured over, sub-executions \
         included"
    );
}

/// 80 through W5; **85 since W6 Task 3**, and the five come from the three
/// entries it added to the list above: `valids/restart-parent` 2,
/// `valids/restart-parent-loop` 2 and `valids/subflow-secret-output-parent` 1.
/// Each is a pair between a run Kestra has and a run it does not, produced
/// because the parent got past a `Subflow` whose child SUCCEEDS here and FAILED
/// there.
// 85 -> 92 at WorkingDirectory Task 8: the two entries above, as measured by the
// `restriction removed` line the test prints.
const RESTRICTION_REMOVES: usize = 92;
/// How many entries those pairs come from — the other half of the coverage
/// statement's *"80 pairs across 6 entries"*, which was printed and not
/// asserted until W4 Task 6. It is 85 across 9 since W6 Task 3.
// 9 -> 11 at WorkingDirectory Task 8: `valids/finally-parallel` and
// `valids/working-directory-invalid-runif` (see the list above).
const RESTRICTION_REMOVES_ENTRIES: usize = 11;
/// 771 through W5; 791 at W6 Task 3; **798 since W8 Task 3**, whose eight
/// banked `Pause` entries carry seven ordered pairs between them — the same
/// seven `EGRET_TOP_LEVEL_PAIRS` gained, counted before the restriction rather
/// than after. It was 791 because 12 `Subflow` entries that
/// stalled at their first run now finish and the runs after them have
/// endpoints.
///
/// **836 since W9a Task 11**: the four `EGRET_TOP_LEVEL_PAIRS` gained.
///
/// **829 since no-silent-stall Task 1**: the seven pairs (two top-level, five
/// nested) the entries it refuses at start used to produce.
/// **910 since the AllowFailure flowable**: the 81 pairs of its three targets.
// 910 -> 935 at WorkingDirectory Task 8: the 18 pairs the harvested flows add
// are egret's own (measured: the restriction test prints `egret produced 935`).
const EGRET_PAIRS_BEFORE_RESTRICTION: usize = 935;

/// Test names `oracle-exec/README.md` cites that **no longer exist**, each with
/// its successor or an empty string where there is none — W3b Task 7.
///
/// A stale *number* can be checked by reading the sentence beside it. A pointer
/// that resolves to nothing cannot be checked at all, and the sweep this task
/// ran found **thirteen** of them: nine deleted with W3a's harness at W3b Task
/// 5, two renamed by the same task, and two renamed on `main` by `05083cbc`
/// before this branch existed. The README's *stale twins* table carries the
/// same list for a reader; this one is what stops the two drifting.
const RETIRED_POINTERS: &[(&str, &str)] = &[
    // Renamed on `main` by `05083cbc`, before this package.
    (
        "a_null_task_list_is_an_over_refusal_that_reaches_the_http_api",
        "a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own",
    ),
    (
        "every_flow_kestra_published_plans_except_three_named_over_refusals",
        "every_flow_kestra_published_plans_except_the_named_refusals",
    ),
    // Renamed at W7b Task 7b: its table holds deliberate refusals now (the
    // 2026-10-11 Pause ruling), not only over-refusals, and more than two.
    (
        "every_flow_kestra_published_plans_except_two_named_over_refusals",
        "every_flow_kestra_published_plans_except_the_named_refusals",
    ),
    // Renamed by W3b Task 5, because the question each asks changed. Both say
    // so in their own doc comments.
    (
        "only_the_top_level_order_pairs_are_ever_asserted",
        "the_ordered_pairs_inside_spawned_are_reachable_now",
    ),
    (
        "the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank",
        "the_sub_executions_are_compared_and_this_is_what_they_cost",
    ),
    // The prefix check became a whole-set comparison once the driver ran the
    // sub-executions: a prefix was all `advance()`'s single batch could offer.
    (
        "every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran",
        "every_sub_execution_egret_runs_is_one_kestra_ran",
    ),
    // Deleted with the four-tier harness at W3b Task 5. There is no successor:
    // the tier each pinned does not exist. The render tier moved into
    // `worker::render`, which has its own tests; the feed is 0; the loop
    // completion tier is `worker::spawn`.
    ("a_skipped_run_still_gets_a_happens_before_endpoint", ""),
    (
        "an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended",
        "",
    ),
    (
        "restricting_the_order_cannot_hide_a_disagreement_on_a_kept_pair",
        "",
    ),
    ("the_caller_renders_and_a_failed_render_fails_the_run", ""),
    (
        "the_caller_can_render_every_expression_except_the_ones_named",
        "",
    ),
    (
        "the_caller_can_render_every_branch_expression_except_the_two_kestra_could_not",
        "",
    ),
    (
        "the_feed_absorbs_a_measured_share_of_the_golden_run_terminals",
        "",
    ),
    (
        "the_loop_completion_tier_is_load_bearing_for_exactly_these_entries",
        "",
    ),
];

/// Every `.rs` file under this crate's `src/` and `tests/`, concatenated.
fn crate_rust_sources() -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = String::new();
    let mut stack = vec![root.join("src"), root.join("tests")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read a crate source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push_str(&std::fs::read_to_string(&path).expect("read a source file"));
            }
        }
    }
    out
}

/// **The retired-pointer table cannot drift from the code or from the README.**
///
/// Three ways it could, each asserted: a retired name coming back as a live
/// test without the row being removed; a successor that does not exist; and a
/// row for a name the README no longer cites, which is dead weight pretending
/// to be a correction.
///
/// **The positive control is load-bearing and is not decoration.** Every "is
/// not a live `fn`" assertion below passes against an *empty* source string, so
/// a walk that read no files — a moved directory, a changed extension, a
/// `read_dir` that silently found nothing — would report the same clean result
/// as the truth. The control is the first assertion for that reason.
#[test]
fn the_readme_pointers_that_no_longer_resolve_are_recorded_and_their_successors_do() {
    let sources = crate_rust_sources();
    assert!(
        sources.contains("fn executor_matches_golden")
            && sources.contains("fn tick")
            && sources.len() > 200_000,
        "the source walk found {} bytes and must have found this crate: every \
         other assertion here is a `does not contain`, and an empty string \
         satisfies all of them",
        sources.len()
    );

    // Both files a reader is pointed at, with whitespace collapsed: the README
    // wraps long names across lines, so a literal search would miss a citation
    // and report the row as dead weight.
    let prose = format!(
        "{}\n{}",
        std::fs::read_to_string(format!("{ORACLE}/README.md")).expect("read the README"),
        std::fs::read_to_string(format!("{ORACLE}/runnable.txt")).expect("read runnable.txt"),
    );
    let flat: String = prose.split_whitespace().collect::<Vec<_>>().join("");

    let mut problems: Vec<String> = Vec::new();
    for (retired, successor) in RETIRED_POINTERS {
        if sources.contains(&format!("fn {retired}")) {
            problems.push(format!(
                "`{retired}` is a live test again; take it out of RETIRED_POINTERS \
                 and out of the README's stale-twin table"
            ));
        }
        if !successor.is_empty() && !sources.contains(&format!("fn {successor}")) {
            problems.push(format!(
                "`{retired}`'s successor `{successor}` does not exist, so the \
                 correction points at nothing too"
            ));
        }
        if !flat.contains(retired) {
            problems.push(format!(
                "neither README.md nor runnable.txt cites `{retired}` any more, so \
                 its row here is dead weight; delete the row with the last citation"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// **`handler_surface.py`'s own two checks, wired into this suite** — W3b Task
/// 7's fix round.
///
/// `--check` regenerates `handler-surface.txt` from `corpus/`, `golden.json`,
/// `baseline.txt` and `task-kinds.json` and diffs it against the committed
/// file; `--self-test` drives every guard in that generator through the real
/// code paths and requires each to fire with its **own** message, so a case
/// cannot pass because a different guard tripped.
///
/// **Both exited 1 from W3b Task 5 until this task, and nothing ran either.**
/// The artifact is a pure function of `baseline.txt`, which Task 5 moved by 32
/// entries, so `--check` reported 124 diff lines; and the self-test's *"a
/// blocker rule that fires on an entry egret already reproduces"* case stopped
/// reaching its guard, because the mutation it used (calling `OutputValues` a
/// `FlowableTask`) only produced the contradiction while those entries still
/// matched — and they stopped matching when the feed was dropped. A generator
/// whose own self-test cannot pass sat beside six green gates, which is the
/// ninth guard-nobody-has-seen-fail this programme has found. Wiring it here is
/// what makes its exit code somebody's problem.
///
/// **A missing `python3` FAILS rather than skips.** `egret.sql`'s live suite is
/// already a self-skip this file has to write a paragraph about; a second one
/// would be a check whose output does not depend on what it checks. Everything
/// this generator reads is committed — no Kestra, no docker, no third-party
/// module — so there is no environment in which skipping is the right answer.
///
/// **The `--self-test` assertion is a positive control, not a formality.** A
/// self-test that ran *no* cases exits 0 and looks exactly like one where every
/// case fired, so the count is read out of its own output and required to be a
/// whole number of whole successes.
#[test]
fn the_handler_surface_generator_checks_itself_and_its_artifact() {
    fn run(arg: &str) -> (bool, String) {
        let out = std::process::Command::new("python3")
            .current_dir(ORACLE)
            .args(["handler_surface.py", arg])
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "python3 must be available to run oracle-exec/handler_surface.py {arg}: {e}. \
                     This is not a skip: everything the generator reads is committed, so a \
                     machine that cannot run it cannot check the artifact at all"
                )
            });
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    let (ok, text) = run("--self-test");
    let fired = text
        .lines()
        .find_map(|l| {
            let rest = l.strip_prefix("[self-test] ")?;
            let (a, b) = rest.split_once(" guards fired")?.0.split_once('/')?;
            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
        })
        .unwrap_or_else(|| {
            panic!("--self-test printed no `[self-test] N/M guards fired` line:\n{text}")
        });
    assert!(
        fired.0 == fired.1 && fired.1 >= 13,
        "every guard in handler_surface.py must fire for its OWN reason, and there \
         must still be at least the 13 W3b Task 1 wrote -- a self-test that ran \
         nothing would exit 0 and print nothing to contradict it. Got {}/{}:\n{text}",
        fired.0,
        fired.1
    );
    assert!(ok, "--self-test exited non-zero:\n{text}");

    let (ok, text) = run("--check");
    assert!(
        ok,
        "handler-surface.txt no longer matches what handler_surface.py derives \
         from the committed corpus, golden and baseline. Re-run \
         `python3 handler_surface.py` and say in the commit message what moved \
         the ceiling and why:\n{text}"
    );
}

/// **Cross-check 1 of 2: `handler_surface.py`'s `RUN_STATE` against the enum it
/// copies** — W8 Task 2 fix round 1.
///
/// `RUN_STATE` is a hand-written Python copy of [`RunState::ALL`], and W8 Task 2
/// found it two variants stale. **The fix shipped in that same commit corrected
/// the copy and left the mechanism that hid the staleness completely intact**:
/// `the_handler_surface_generator_checks_itself_and_its_artifact` compares the
/// SCRIPT against its ARTIFACT, so a `RUN_STATE` that disagrees with the enum
/// regenerates a self-consistent `handler-surface.txt` and stays green. Nothing
/// in the repository compared copy against enum.
///
/// This is that comparison, and it is the cheaper half of the pair with
/// [`the_two_state_vocabularies_agree`]. It parses the literal out of the
/// script rather than importing Python, because the failure being guarded is
/// *the literal*, and re-deriving it any other way would be guarding something
/// else.
///
/// **Both directions**, because a one-directional check acquits a copy that has
/// lost a member — which is exactly the direction this defect went.
#[test]
fn handler_surfaces_run_state_copy_matches_the_enum() {
    let script = include_str!("../oracle-exec/handler_surface.py");
    let start = script.find("RUN_STATE = {").expect(
        "handler_surface.py must still define RUN_STATE; if it was renamed, this \
                 cross-check has to move with it rather than be deleted",
    );
    let end = start
        + script[start..]
            .find('}')
            .expect("RUN_STATE's literal must be brace-delimited");
    let body = &script[start..end];
    let copied: BTreeSet<&str> = body
        .split('"')
        .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
        .collect();
    let held: BTreeSet<&str> = RunState::ALL.iter().map(|s| s.wire()).collect();

    // The parser is checked before its output is trusted: a `split('"')` that
    // matched nothing would make `copied` empty and every `missing` assertion
    // below would still pass on a `held` of any size.
    assert_eq!(
        copied.len(),
        held.len(),
        "handler_surface.py's RUN_STATE holds {} state(s), RunState::ALL holds \
         {}. Copy = {copied:?}, enum = {held:?}",
        copied.len(),
        held.len()
    );
    assert!(
        copied.len() >= 9,
        "the RUN_STATE literal parsed to {copied:?}, which is too small to be \
         the real one -- the parser above has stopped matching and this test is \
         no longer checking anything"
    );
    let missing_from_copy: Vec<&&str> = held.difference(&copied).collect();
    let extra_in_copy: Vec<&&str> = copied.difference(&held).collect();
    assert!(
        missing_from_copy.is_empty() && extra_in_copy.is_empty(),
        "handler_surface.py's RUN_STATE has drifted from RunState::ALL. \
         In the enum and not the copy: {missing_from_copy:?}. In the copy and \
         not the enum: {extra_in_copy:?}. Update the literal AND re-run \
         `python3 handler_surface.py`, because the classification of which \
         entries are blocked by 'a state RunState cannot hold' moves with it"
    );
}

/// **Every `models::State` variant is accounted for — the arm by the compiler,
/// the row by the enum's own source.**
///
/// The gap [`the_two_state_vocabularies_agree`] had in fix round 1: its
/// seventeen-name list was carried by the test, so an 18th variant added to
/// `models::State` changed nothing and nothing went red. `State` has no `ALL`
/// and no exhaustive `match` anywhere in PRODUCTION — measured, not assumed:
/// with an 18th variant added, `cargo check --lib` compiles clean and the only
/// `error[E0004]` in the whole crate comes from `--tests`, i.e. from here.
///
/// # Two halves, because the first one alone over-claimed
///
/// 1. **The ARM is forced by the compiler.** `wire_of`'s `match` is exhaustive,
///    so a new variant stops this test crate compiling until somebody names it
///    and decides which side it belongs on.
/// 2. **The ROW is forced by parsing `models/state.rs`.** Nothing in stable
///    Rust can enumerate a variant a test cannot name — there is no `ALL`, no
///    derive, and `std::mem::variant_count` is nightly — so `all` below is
///    compared against the variant list parsed out of the enum's own source,
///    the same mechanism `handler_surfaces_run_state_copy_matches_the_enum`
///    uses on the Python copy, with the same parser-sanity arm so an empty
///    parse cannot acquit.
///
/// **Half 1 alone was a guard that over-claimed, and mutation is what caught
/// it.** `State::Invented => "TYPO_NOT_WHAT_SERDE_SAYS"` **compiled and passed
/// all three guards**: the arm was forced, the row was not, and the serde
/// comparison only ever runs over rows — so the variant existed, its spelling
/// was wrong, and everything was green. With half 2 the same mutation dies, and
/// so does the quieter one that adds a variant with a *correct* arm and no row.
///
/// **What this licenses, exactly**: *an added variant cannot pass unnoticed,
/// and a row's spelling cannot disagree with serde's*. It does **not** license
/// *"the arm is correct"* in any wider sense — `wire_of` is checked against
/// `serde` and nothing checks either against Kestra, so a shared error in both
/// would pass. That is written down because fix round 2 drew the wider
/// conclusion from the narrower measurement.
///
/// It is a `fn` rather than a bare `match` so that the exhaustiveness is a
/// property of a signature and not of a statement somebody can delete a line
/// from without noticing.
#[test]
fn every_models_state_variant_is_accounted_for() {
    use egret_workflow::models::State;

    /// Exhaustive on purpose. Adding a variant to `models::State` fails to
    /// compile here.
    fn wire_of(s: State) -> &'static str {
        match s {
            State::Created => "CREATED",
            State::Submitted => "SUBMITTED",
            State::Running => "RUNNING",
            State::Paused => "PAUSED",
            State::Restarted => "RESTARTED",
            State::Killing => "KILLING",
            State::Success => "SUCCESS",
            State::Warning => "WARNING",
            State::Failed => "FAILED",
            State::Killed => "KILLED",
            State::Cancelled => "CANCELLED",
            State::Queued => "QUEUED",
            State::Retrying => "RETRYING",
            State::Retried => "RETRIED",
            State::Skipped => "SKIPPED",
            State::Breakpoint => "BREAKPOINT",
            State::Resubmitted => "RESUBMITTED",
        }
    }

    let all = [
        State::Created,
        State::Submitted,
        State::Running,
        State::Paused,
        State::Restarted,
        State::Killing,
        State::Success,
        State::Warning,
        State::Failed,
        State::Killed,
        State::Cancelled,
        State::Queued,
        State::Retrying,
        State::Retried,
        State::Skipped,
        State::Breakpoint,
        State::Resubmitted,
    ];
    // The arm's string and serde's must agree, or the exhaustive match could be
    // widened with a spelling nothing else uses.
    for s in all {
        assert_eq!(
            serde_json::to_string(&s).expect("State serialises"),
            format!("\"{}\"", wire_of(s)),
            "{s:?}'s arm in `wire_of` disagrees with its serde spelling"
        );
    }
    let names: BTreeSet<&str> = all.iter().copied().map(wire_of).collect();
    assert_eq!(
        names.len(),
        all.len(),
        "two variants share a wire spelling, which would hide one of them"
    );

    // **The row, forced by the enum's own source** -- see this test's doc. The
    // compiler forces the ARM; nothing in stable Rust forces this ARRAY, so it
    // is checked against the variant list parsed out of `models/state.rs`, the
    // same mechanism `handler_surfaces_run_state_copy_matches_the_enum` uses on
    // the Python copy.
    let source = include_str!("../src/models/state.rs");
    let body = source
        .split_once("pub enum State {")
        .expect(
            "models/state.rs must still declare `pub enum State {`; if it was \
             renamed this guard has to move with it rather than be deleted",
        )
        .1
        .split_once("\n}")
        .expect("the enum body must be brace-terminated")
        .0;
    // **Every content line, not every line that ends in a comma** -- fix round
    // 4. The first version of this filter required `l.ends_with(',')`, and
    // Rust does not require a trailing comma on the LAST variant. So
    // `Invented` appended with no comma was dropped by the parser, `declared`
    // stayed at seventeen, and round 2's exact escape came back: arm forced,
    // row absent, spelling unchecked, everything green. Trailing punctuation
    // is now stripped instead of required.
    let content: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("#["))
        .collect();
    let declared: BTreeSet<String> = content
        .iter()
        .map(|l| l.trim_end_matches(',').trim().to_string())
        .collect();

    // **The parser is checked as a SET, not against a floor** -- fix round 4.
    // `declared.len() >= 9` was the wrong shape twice over: seventeen variants
    // minus one is still nine, so it structurally could not see a *dropped*
    // variant, and dropping is the **acquitting** direction -- the only one
    // that matters. What is asserted instead is that the parse is TOTAL: every
    // content line in the enum body yielded exactly one variant. A line the
    // filter silently skips makes these two disagree.
    assert_eq!(
        declared.len(),
        content.len(),
        "the enum-body parser yielded {} name(s) from {} content line(s), so it \\
         skipped or merged at least one. Lines: {content:?}. Parsed: \\
         {declared:?}",
        declared.len(),
        content.len()
    );
    let malformed: Vec<&String> = declared
        .iter()
        .filter(|v| {
            !v.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .collect();
    assert!(
        malformed.is_empty(),
        "the enum-body parser produced {malformed:?}, which are not Rust \\
         variant names -- it is matching something other than the enum body \\
         and this guard is checking nothing"
    );
    let rows: BTreeSet<String> = all.iter().map(|s| format!("{s:?}")).collect();
    assert_eq!(
        rows, declared,
        "`all` above and `models::State`'s declared variants have diverged. \
         The compiler forces an ARM in `wire_of` for a new variant but nothing \
         forces a ROW here, so a variant added with a correct arm and no row \
         would never be serde-checked -- which is exactly how a `wire_of` arm \
         spelt TYPO_NOT_WHAT_SERDE_SAYS passed in fix round 2"
    );
    assert_eq!(
        all.len(),
        17,
        "Kestra's `State.Type` has seventeen members; if this moves, the \
         artifact moved and every table quoting seventeen moves with it"
    );
}

/// **Cross-check 2 of 2: `executor::RunState` against `models::State`** — W8
/// Task 2 fix round 1.
///
/// Two state vocabularies, two independent spellings —
/// [`RunState::wire`](egret_workflow::executor::RunState::wire) hand-written,
/// `models::State` through `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` —
/// and until now **nothing compared them**. W8 Task 2 reported that as a
/// theoretical gap; it is not theoretical:
///
/// - it produced the `RUN_STATE` defect in Task 2's own commit, and
/// - a second undetected instance was already in the tree —
///   `worker/subflow.rs`'s doc asserted `State.Type.isFailed()` was
///   `FAILED || KILLED` for several packages while `models::State::is_failed`
///   sat five files away with the **correct** `matches!(self, Self::Failed)`.
///   The contradiction was falsifiable without decompiling anything, and
///   nothing went red.
///
/// Three properties, each two-sided:
///
/// 1. every `RunState` variant's wire spelling is a `models::State` that
///    round-trips to the same string;
/// 2. `RunState::is_terminal` and `models::State::is_terminated` agree on every
///    shared member — **both ways**, so neither a widened nor a narrowed
///    `is_terminal` survives;
/// 3. the variants `models::State` has and `RunState` does not are exactly the
///    ones this crate says are missing.
///
/// # The third claim did not hold in fix round 1, and this is what makes it hold
///
/// Round 1 said *"a variant added to one and forgotten in the other fails
/// here"*. **For `models::State` that was false**: adding an 18th variant
/// survived the whole lib + executor suite, because `State` has no `ALL` and no
/// exhaustive `match` anywhere, so the seventeen names below were a list the
/// test carried rather than a list the type forced.
///
/// [`every_models_state_variant_is_accounted_for`] is the missing half. **This
/// test still does not cover that direction and no longer claims to**: its
/// `all_seventeen` is a literal, so an 18th `models::State` variant is
/// invisible here, and the round-1 sentence saying it *"moves the list above"*
/// was measured false — the test reports `ok`.
///
/// **This is the shape refused for M10, and refusing it took two more rounds
/// than it should have.** Round 1 shipped a guard whose stated scope exceeded
/// its reach; round 2's replacement did it again one level down — the compiler
/// forced the arm, nothing forced the row, and a deliberately mis-spelt arm
/// passed. Each claim here is now written against the mechanism that actually
/// enforces it, with the direction it cannot see named beside it.
#[test]
fn the_two_state_vocabularies_agree() {
    use egret_workflow::models::State;

    let held: BTreeSet<String> = RunState::ALL.iter().map(|s| s.wire().to_string()).collect();

    // 1. Every RunState wire spelling is a models::State, spelled identically.
    for s in RunState::ALL {
        let json = format!("\"{}\"", s.wire());
        let parsed: State = serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!(
                "RunState::{s:?} spells itself {:?}, which `models::State` does not \
                 accept ({e}). The two vocabularies have diverged",
                s.wire()
            )
        });
        let back = serde_json::to_string(&parsed).expect("State serialises");
        assert_eq!(
            back, json,
            "RunState::{s:?} and models::State::{parsed:?} do not agree on the \
             wire spelling"
        );

        // 2. and the terminality judgement, both directions.
        assert_eq!(
            s.is_terminal(),
            parsed.is_terminated(),
            "RunState::{s:?}.is_terminal() = {}, models::State::{parsed:?}\
             .is_terminated() = {}. Kestra has ONE `State.Type.isTerminated()`; \
             two answers means one of these two files is wrong about the \
             artifact",
            s.is_terminal(),
            parsed.is_terminated()
        );
    }

    // 3. What models::State has and RunState does not, named rather than
    //    counted.
    //
    //    **It is NOT symmetric, and the claim that it was is withdrawn** --
    //    fix round 4, the third instance of a correction made where it was
    //    found and left standing everywhere else. `all_seventeen` below is a
    //    literal this test carries: gaining a variant on the `RunState` side
    //    moves `held` and fails here, and gaining one on the `models::State`
    //    side moves NOTHING here. That direction is
    //    `every_models_state_variant_is_accounted_for`'s, and only since fix
    //    round 3.
    let all_seventeen = [
        "CREATED",
        "SUBMITTED",
        "RUNNING",
        "PAUSED",
        "RESTARTED",
        "KILLING",
        "SUCCESS",
        "WARNING",
        "FAILED",
        "KILLED",
        "CANCELLED",
        "QUEUED",
        "RETRYING",
        "RETRIED",
        "SKIPPED",
        "BREAKPOINT",
        "RESUBMITTED",
    ];
    for name in all_seventeen {
        serde_json::from_str::<State>(&format!("\"{name}\"")).unwrap_or_else(|e| {
            panic!("models::State must still hold {name}, Kestra's own State.Type ({e})")
        });
    }
    let missing: BTreeSet<String> = all_seventeen
        .iter()
        .map(|s| s.to_string())
        .filter(|s| !held.contains(s))
        .collect();
    assert_eq!(
        missing,
        BTreeSet::from([
            "BREAKPOINT".to_string(),
            "KILLING".to_string(),
            "QUEUED".to_string(),
            "RESUBMITTED".to_string(),
        ]),
        "the FOUR of Kestra's seventeen that `RunState` does not hold. It was \
         six until W8 Task 5 added `Warning` and five until W8 Task 6 added \
         `Cancelled` -- which was the golden's last unrepresentable state, so \
         **none of the four left appears in the golden at all** and this set \
         cannot shrink further from this corpus. \
         Adding a variant to `RunState` moves this set. It does NOT move when \
         `models::State` gains one: `all_seventeen` above is a literal this \
         test carries, so an 18th variant is invisible here and is caught by \
         `every_models_state_variant_is_accounted_for` instead -- the claim \
         that it moved `the list above` was measured false in fix round 2"
    );
    assert_eq!(
        held.len() + missing.len(),
        17,
        "the two sets must partition Kestra's seventeen with nothing left over"
    );
}

/// The happens-before predicate, strict on both sides — `normalise.py`'s, over
/// the logical `executions.seq` instead of timestamps.
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

/// **The circular region is gone, and the demonstration could have come out the
/// other way.**
///
/// W3a's oracle fed exactly one field from the golden: the last element of the
/// `state` sequence of a run whose `PlanTask` is `Runnable`, and of each of that
/// run's attempts. Its module doc said so in as many words, and the consequence
/// was that a normalisation bug or a capture anomaly *in that one field* was
/// invisible in both directions — rewrite a matching entry's run terminal from
/// `SUCCESS` to `KILLED` and the entry went on matching, because `golden_outcome`
/// fed the same lie into egret's side.
///
/// The driver produces terminal states from a handler that ran. Nothing reads
/// `golden.json` on egret's side at all — `driver::run` takes a flow source and
/// the posted inputs and returns what Postgres holds afterwards.
///
/// **W3a's FIRST attempt at exactly this demonstration was vacuous** and said
/// so: it used `valids/minimal2`, whose golden run history carries `SUBMITTED`,
/// which `RunState` cannot represent — so that entry could not match under any
/// value and the experiment distinguished nothing. This test is built so that
/// cannot happen:
///
/// 1. it asserts the chosen entry **matches** before the corruption, so the
///    corruption is the only thing that can change the answer;
/// 2. it corrupts the run terminal AND the attempt terminal, which is exactly
///    what `golden_outcome` used to feed;
/// 3. it asserts the entry then **mismatches**;
/// 4. and it drives `valids/minimal2` through the same corruption as the
///    NEGATIVE control, showing an entry for which the experiment is vacuous —
///    it mismatches before and after, so its answer never depended on the
///    corruption.
#[tokio::test(flavor = "multi_thread")]
async fn a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to() {
    let side = egret_side().await;
    let corrupt = |entry: &GoldenEntry| {
        let mut t = Trace::from(entry);
        let run = t.runs.first_mut().expect("the entry has a run");
        *run.state.last_mut().expect("the run has a history") = "KILLED".to_string();
        for a in &mut run.attempts {
            if let Some(last) = a.last_mut() {
                *last = "KILLED".to_string();
            }
        }
        t
    };

    // The entry, chosen because it MATCHES: a single `Log`, whose terminal is
    // exactly the field that used to be fed.
    let subject = golden()
        .into_iter()
        .find(|e| e.id == "valids/minimal")
        .expect("valids/minimal is in the golden");
    let Outcome::Trace(actual) = evaluate(side, &subject) else {
        panic!("valids/minimal produces a trace");
    };
    let mut actual = actual.clone();
    let expected = Trace::from(&subject);
    actual.restrict_order_to(&expected);
    assert_eq!(
        actual, expected,
        "the experiment is only meaningful on an entry that matches -- this is \
         the assertion W3a's vacuous first attempt did not have"
    );
    assert_eq!(
        actual.runs[0].state.last().map(String::as_str),
        Some("SUCCESS"),
        "and the field about to be corrupted is the one that used to be fed"
    );
    assert_ne!(
        actual,
        corrupt(&subject),
        "rewriting the run terminal and the attempt terminal to KILLED must now \
         make the entry MISMATCH. Under W3a's harness it did not: `golden_outcome` \
         fed the same value into egret's side, so the comparison was circular for \
         this one field and a capture anomaly in it was invisible in both \
         directions"
    );

    // **The nested half, which was prose in the README and nothing in this
    // file.** The coverage statement publishes a three-row table whose second
    // row corrupts a run INSIDE a sub-execution; only the top-level row was
    // committed, so the half that says the sub-execution traces are compared
    // rather than merely populated had no instrument. It is a different claim:
    // `Trace::restrict_order_to` descends into `spawned` and the comparison
    // does too, but "the field is filled" and "a wrong value in it is caught"
    // are exactly the two things this package keeps finding apart.
    let corrupt_nested = |entry: &GoldenEntry| {
        let mut t = Trace::from(entry);
        let sub = t
            .spawned
            .first_mut()
            .expect("the entry has a sub-execution");
        let run = sub.runs.first_mut().expect("the sub-execution has a run");
        *run.state.last_mut().expect("the run has a history") = "KILLED".to_string();
        for a in &mut run.attempts {
            if let Some(last) = a.last_mut() {
                *last = "KILLED".to_string();
            }
        }
        t
    };
    let nested = golden()
        .into_iter()
        .find(|e| e.id == "valids/loop-serial")
        .expect("valids/loop-serial is in the golden");
    let Outcome::Trace(nested_actual) = evaluate(side, &nested) else {
        panic!("valids/loop-serial produces a trace");
    };
    let mut nested_actual = nested_actual.clone();
    let nested_expected = Trace::from(&nested);
    nested_actual.restrict_order_to(&nested_expected);
    assert_eq!(
        nested_actual, nested_expected,
        "the nested experiment needs a matching entry for the same reason the \
         top-level one does"
    );
    assert_eq!(
        nested_actual.spawned[0].runs[0]
            .state
            .last()
            .map(String::as_str),
        Some("SUCCESS"),
        "and the field about to be corrupted is inside a sub-execution egret \
         RAN, not one it left empty"
    );
    assert_ne!(
        nested_actual,
        corrupt_nested(&nested),
        "a corrupted terminal INSIDE a sub-execution must break the entry too. \
         Under W3a's harness egret's `spawned` was `Vec::new()` unconditionally, \
         so nothing in there was compared at all"
    );

    // The negative control: an entry for which the same experiment
    // distinguishes nothing, DERIVED rather than named -- and **the property it
    // is derived from has had to change twice, each time because the enum
    // grew**. W3a's vacuous first attempt named `valids/minimal2`, whose golden
    // carried `SUBMITTED`; W3c added `SUBMITTED` and the control became "a
    // golden whose FIRST run's history holds a state `RunState` cannot hold",
    // which was derived rather than named. **W8 Task 6 emptied that population
    // too**: `RunState` now holds all thirteen of the golden's states, so no
    // entry has that property and the `find` returned `None`.
    //
    // The property that replaces it is the one the control actually needs, and
    // it does not depend on the vocabulary at all: **an entry whose run COUNT
    // egret does not reproduce**. Corrupting one run's terminal cannot make a
    // trace with the wrong number of runs match, and cannot make it stop
    // matching, so the experiment is vacuous on it in both directions -- which
    // is the whole point of a negative control. It is derived from what egret
    // ACTUALLY produced this run, so it cannot go stale the way a named entry
    // can; if the population ever empties, the `expect` below fails loudly
    // rather than quietly acquitting.
    let vacuous = golden()
        .into_iter()
        .find(|e| {
            e.verdict == Verdict::Runs
                && !e.runs.is_empty()
                && match evaluate(side, e) {
                    Outcome::Trace(t) => t.runs.len() != e.runs.len(),
                    _ => false,
                }
        })
        .expect(
            "some golden `runs` entry has a run count egret does not reproduce; \
             if none does, every baselined entry differs only in field VALUES \
             and this control needs a new property",
        );
    println!("the vacuity control is {}", vacuous.id);
    let Outcome::Trace(control) = evaluate(side, &vacuous) else {
        panic!("{} produces a trace", vacuous.id)
    };
    let mut control = control.clone();
    let control_expected = Trace::from(&vacuous);
    control.restrict_order_to(&control_expected);
    assert_ne!(
        control, control_expected,
        "{}'s run COUNT is not the golden's, so it cannot match whatever any \
         single terminal says",
        vacuous.id
    );
    assert_ne!(
        control,
        corrupt(&vacuous),
        "and it does not match the corrupted version either, so on THIS entry \
         the experiment could not have come out the other way. That is exactly \
         why W3a's first attempt proved nothing, and it is here as the control \
         that makes the assertion above mean something"
    );
}

/// **Where every mismatching entry differs, by FIELD, derived.**
///
/// The W3b plan's rule for this task is *"attribute every moved entry"*: the
/// render surface widens, so entries baselined for reasons unrelated to
/// durability move too, and a bank whose cause nobody named is a coincidence.
/// A per-entry field breakdown is the instrument that makes the attribution
/// checkable rather than remembered — an entry that starts differing on
/// `execution` when it used to differ on `spawned` shows up here as a changed
/// map even if the ratchet's total does not move.
///
/// Four buckets, and an entry is in as many as it differs on. The totals are
/// asserted beside the parts.
#[tokio::test(flavor = "multi_thread")]
async fn every_mismatching_entry_is_attributed_to_the_field_it_differs_on() {
    let side = egret_side().await;
    let (mut execution, mut runs, mut order, mut spawned, mut refused, mut broken) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut total = 0usize;
    let mut order_only: Vec<String> = Vec::new();
    let mut unfinished: Vec<String> = Vec::new();
    let mut matched_while_stalled: Vec<String> = Vec::new();
    let mut refused_ids: Vec<(String, String)> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let expected = Trace::from(entry);
        let actual = match evaluate(side, entry) {
            Outcome::Trace(t) => {
                let mut t = t.clone();
                t.restrict_order_to(&expected);
                t
            }
            Outcome::Refused(why) => {
                refused += 1;
                refused_ids.push((entry.id.clone(), why.clone()));
                total += 1;
                continue;
            }
            Outcome::Broken(_) => {
                broken += 1;
                total += 1;
                continue;
            }
        };
        // A run left mid-flight is the shape the DRIVER produces where the
        // harness could not: `Registry::run` refuses a type it has no handler
        // for, so the execution stalls exactly as the shipping worker would.
        //
        // **This is collected BEFORE the `continue`, and that is the whole
        // point of the count.** It used to sit after it, so an execution that
        // stalled and nevertheless MATCHED was excluded by construction — which
        // is exactly the over-acceptance `ENTRIES_LEFT_UNFINISHED` exists to
        // bound. A count whose population cannot contain the case it is written
        // about is a check whose output does not depend on what it checks.
        let stalled = is_stalled(&actual.execution);
        if stalled {
            unfinished.push(entry.id.clone());
        }
        if actual == expected {
            if stalled {
                matched_while_stalled.push(entry.id.clone());
            }
            continue;
        }
        total += 1;
        let e = actual.execution != expected.execution;
        let r = actual.runs != expected.runs;
        let o = actual.order != expected.order;
        let s = actual.spawned != expected.spawned;
        execution += usize::from(e);
        runs += usize::from(r);
        order += usize::from(o);
        spawned += usize::from(s);
        if o && !e && !r && !s {
            order_only.push(entry.id.clone());
        }
    }

    println!(
        "mismatch by field: execution {execution}, runs {runs}, order {order}, \
         spawned {spawned}, refused {refused}, broken {broken}; TOTAL entries {total}"
    );
    println!("differ on `order` ALONE: {order_only:?}");
    println!(
        "egret left {} execution(s) unfinished: {unfinished:?}",
        unfinished.len()
    );

    // **The other half of the message, DERIVED over the whole remaining
    // population rather than trusted.** The four `retry-new-execution-*`
    // entries are the four this claim was measurably false for; it was false
    // for them, unchecked, for five packages, so the sentence gets a running
    // program now.
    //
    // **It runs before every assertion in this test, on purpose, and the
    // position was arrived at by measurement rather than by taste.** Dropping
    // `RETRIED` from `TERMINAL` is the one-line mutation that puts the four
    // back into `unfinished` and so drives this check. Behind the count, the
    // by-name loop, OR the `matched_while_stalled` assertion it never runs at
    // all under that mutation -- each of those fires first, and a derivation
    // that cannot fail looks exactly like one that holds. Placed here, that
    // mutation fails HERE, naming all four. Both positions were run.
    //
    // The predicate is the message's own: the entry must hold a
    // [`TaskKind::Runnable`] whose `type_id` the DRIVER's registry has no
    // handler for. `Runnable` and not any kind, or the claim would be vacuous
    // -- every flow with a `Sequential` holds a type the registry does not
    // serve, and flowables are `advance()`'s work, not a handler's.
    let handled: BTreeSet<String> = driver::registry(&driver::unconnected_pool())
        .type_ids()
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut stalled_without_an_unhandled_type: Vec<(String, Vec<String>)> = Vec::new();
    for id in &unfinished {
        let plan = build_plan(&corpus_source(id))
            .unwrap_or_else(|e| panic!("{id} is in the runs population, so it plans: {e}"));
        let leaves: Vec<String> = plan
            .tasks
            .iter()
            .filter_map(|t| match &t.kind {
                TaskKind::Runnable { type_id } => Some(type_id.clone()),
                _ => None,
            })
            .collect();
        if leaves.iter().all(|t| handled.contains(t)) {
            stalled_without_an_unhandled_type.push((id.clone(), leaves));
        }
    }
    assert!(
        stalled_without_an_unhandled_type.is_empty(),
        "these entries are counted as left mid-flight and every runnable type \
         they declare HAS a handler, so the reason this count gives for them is \
         not their reason. That was true of the four `retry-new-execution-*` \
         entries for five packages -- the message asserted it and nothing \
         derived it -- and the fix was a classifier change, not a handler. \
         {stalled_without_an_unhandled_type:?}"
    );
    // The control: the claim is not vacuous, because the driver's registry is
    // not empty and the four it was wrong about would have failed the check
    // above -- which is measured rather than reasoned, see the comment above.
    assert_eq!(
        driver::registry(&driver::unconnected_pool()).type_ids(),
        [
            // WorkingDirectory Task 8: the six script kinds `script::register` adds
            // (`ScriptKind::ALL`), which the driver now registers for the Docker runner.
            "egret.container",
            "egret.script.python",
            "io.kestra.plugin.core.debug.Return",
            "io.kestra.plugin.core.execution.Fail",
            "io.kestra.plugin.core.flow.Sleep",
            "io.kestra.plugin.core.kv.Delete",
            "io.kestra.plugin.core.kv.Get",
            "io.kestra.plugin.core.kv.GetKeys",
            "io.kestra.plugin.core.kv.PurgeKV",
            "io.kestra.plugin.core.kv.Put",
            "io.kestra.plugin.core.kv.Set",
            "io.kestra.plugin.core.log.Log",
            "io.kestra.plugin.core.namespace.DeleteFiles",
            "io.kestra.plugin.core.namespace.DownloadFiles",
            "io.kestra.plugin.core.namespace.PurgeFiles",
            "io.kestra.plugin.core.namespace.UploadFiles",
            "io.kestra.plugin.core.storage.Concat",
            "io.kestra.plugin.core.storage.DeduplicateItems",
            "io.kestra.plugin.core.storage.Delete",
            "io.kestra.plugin.core.storage.FilterItems",
            "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles",
            "io.kestra.plugin.core.storage.Reverse",
            "io.kestra.plugin.core.storage.Size",
            "io.kestra.plugin.core.storage.Split",
            "io.kestra.plugin.core.storage.Write",
            "io.kestra.plugin.scripts.python.Commands",
            "io.kestra.plugin.scripts.python.Script",
            "io.kestra.plugin.scripts.shell.Commands",
            "io.kestra.plugin.scripts.shell.Script",
        ],
        "the twenty-nine handlers the driver runs (the four core storage tasks since \
         W9a Task 11, the five whole-file ones since Task 12, the four namespace-file \
         tasks since W9b Tasks 6 and 7, the six KV tasks since W9c Task 5, and the six \
         script kinds since WorkingDirectory Task 8). If this list ever grew to cover \
         everything the corpus declares, the derivation above would pass \
         because nothing stalls rather than because the reason holds"
    );
    for id in [
        "valids/retry-new-execution-flow-attempts",
        "valids/retry-new-execution-task-duration",
    ] {
        let plan = build_plan(&corpus_source(id)).expect("it plans");
        assert!(
            plan.tasks.iter().all(|t| match &t.kind {
                TaskKind::Runnable { type_id } => handled.contains(type_id),
                _ => true,
            }),
            "{id} declares only handled runnable types, so had it still been in \
             `unfinished` the derivation above would have named it. That is \
             what makes the derivation an instrument rather than a formality"
        );
    }

    // The deliberate refusals of flows Kestra ran: the user's ruling of
    // 2026-10-11 (W7b Task 4b) for a `Pause` declaring `behavior:`, `errors:`
    // or `onPause:`, and no-silent-stall Task 1 (2026-10-12) for an
    // `AllowFailure`, a `WorkingDirectory` or a type egret does not register.
    // Each was a task egret ran as an opaque runnable no worker could serve,
    // and the execution stalled for ever. Named, so any OTHER refusal still
    // fails here, and each must be refused by one of the static refusals.
    assert_eq!(
        refused_ids
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        RUNS_REFUSED,
        "{refused_ids:?}"
    );
    for (id, why) in &refused_ids {
        assert!(
            [
                "unsupported_pause_property",
                "unsupported_allow_failure",
                "unsupported_working_directory",
                "unknown_task_type",
            ]
            .iter()
            .any(|c| why.contains(c)),
            "{id}: {why}"
        );
    }
    assert_eq!(
        (execution, runs, order, spawned, refused, broken, total),
        (
            MISMATCH_ON_EXECUTION,
            MISMATCH_ON_RUNS,
            MISMATCH_ON_ORDER,
            MISMATCH_ON_SPAWNED,
            RUNS_REFUSED.len(),
            0,
            MISMATCHING_RUNS_ENTRIES
        ),
        "the field breakdown. `refused` is exactly `RUNS_REFUSED` -- egret's \
         deliberate refusals at start, each with its code above -- and `broken` \
         is 0, which is the load-bearing half: a `broken` would be the DRIVER \
         failing rather than the flow diverging"
    );
    assert_eq!(
        order_only,
        [
            "valids/dag",
            "valids/full",
            "valids/if",
            "valids/parallel-disabled-tasks"
        ],
        "the entries whose runs, attempts and execution history are \
         byte-identical to Kestra's and which differ on the happens-before \
         relation ALONE. Three of the four are exactly the ones W3a's Task 1 \
         named as carrying `an order pair a clockless machine cannot reach`. \
         **The fourth is the finding**: W3b Task 1's Concern 3 said \
         `valids/full`'s attribution -- flow-level `outputs:` and `variables:` \
         -- was WRONG (the flow has neither) and its real cause unknown. This \
         is the cause, and it is the same one, so the entry belongs with the \
         other three rather than in a list of its own"
    );
    assert!(
        matched_while_stalled.is_empty(),
        "an execution that STALLED and nevertheless matched the golden is the \
         over-acceptance this count is written about, and it is the one case \
         the count could not see: the collection used to run after the \
         `continue`, so a matching entry was excluded by construction. It is \
         empty today and the intersection is asserted rather than assumed: \
         {matched_while_stalled:?}"
    );
    assert_eq!(
        unfinished.len(),
        ENTRIES_LEFT_UNFINISHED,
        "how many executions the driver leaves mid-flight. Every one holds a \
         task type this worker has no handler for -- `Registry::run` refuses \
         it, the tick aborts for that run, and the execution stalls exactly as \
         the shipping worker would. **That sentence is DERIVED above**, over \
         this same population and the driver's own registry, rather than \
         asserted: it was false for four entries for five packages before \
         anybody checked it. **That sentence read `Every one IS a task \
         type ...` and was false for four of them until W6 Task 4**: the \
         `retry-new-execution-*` entries declare only \
         `io.kestra.plugin.core.execution.Fail`, which has a handler and ran, \
         and they were counted because `is_stalled` did not list `RETRIED` -- a \
         classifier artefact rather than a stall. See [`TERMINAL`]. Named and \
         counted so the population cannot grow in silence: {unfinished:?}"
    );
    // **The correction, asserted rather than described.** A message that
    // *says* the four are gone is prose; this is the check.
    for id in [
        "valids/retry-new-execution-flow-attempts",
        "valids/retry-new-execution-flow-duration",
        "valids/retry-new-execution-task-attempts",
        "valids/retry-new-execution-task-duration",
    ] {
        assert!(
            !unfinished.contains(&id.to_string()),
            "{id} declares only `core.execution.Fail`, which HAS a handler. It \
             ends `RETRIED`, which is terminal, and it now spawns the replay \
             that follows it -- it cannot be an execution left mid-flight. \
             Present in: {unfinished:?}"
        );
    }
}

/// The numbers above, as named constants so this report and the task's own
/// report cannot disagree about them.
// W6 Task 3: 12 entries left the mismatching population, and each of the four
// field counters fell by however many of the 12 differed on that field.
// `spawned` fell by exactly one, `valids/loop-with-subflow`, which is the only
// one of the 12 that carries a sub-execution at all.
/// **90 through W8 Task 2, 82 after W8 Task 3, 74 after W8 Task 5, 72 now**:
/// all eight `Pause` entries W8 Task 3 banked differed on their execution
/// history (`PAUSED`/`RESTARTED` were never emitted), so every one of them left
/// this count -- and so did W8 Task 6's two, whose golden execution history
/// `["CREATED","CANCELLED"]` egret could not spell at all until
/// `RunState::Cancelled` existed.
///
/// **71 since W9a Task 11**: `valids/loop-values-from-uri` banked.
///
/// **68 since W7b Task 4b**: the three `Pause` entries of [`RUNS_REFUSED`]
/// are refused now, so they differ on no field and count as `refused`.
///
/// **26 since no-silent-stall Task 1**: the 49 entries it added to
/// [`RUNS_REFUSED`] are refused rather than run, and 42 of them differed on
/// the execution history.
// 26 -> 29 at WorkingDirectory Task 8, measured by this census over the 301-entry
// golden (the harvested `retry-failed-task-duration` is one of the entries that
// differ on the execution: egret makes 4 attempts where Kestra made 3).
const MISMATCH_ON_EXECUTION: usize = 29;
/// **112 through W8 Task 2, 104 after W8 Task 3, 103 after W8 Task 4, 96 after
/// W8 Task 5, 94 now** -- the same eight, then `valids/waitfor-nested`, then
/// the seven `allow*` entries, then W8 Task 6's two. The last pair differ on
/// `runs` for an unusual reason: their golden `runs` is **empty**, and egret
/// used to run the flow's one task, so the field differed by a whole run
/// rather than by a state.
///
/// **93 since W9a Task 11**: `valids/loop-values-from-uri` banked.
///
/// **90 since W7b Task 4b**: the three of [`RUNS_REFUSED`] then.
///
/// **41 since no-silent-stall Task 1**: the 49 it added, every one of which
/// differed on `runs`.
// 41 -> 45 at WorkingDirectory Task 8 (measured; see `MISMATCH_ON_EXECUTION`).
const MISMATCH_ON_RUNS: usize = 45;
/// **72 through W8 Task 2, 67 after W8 Task 3, 64 now**: five of Task 3's eight
/// carry an ordered pair, and W8 Task 4 closed THREE on one entry --
/// `valids/waitfor-nested`'s `loop_1_return`/`loop_2`,
/// `loop_2_return`/`loop_3` and `loop_3_return`/`loop_3_log`, every one of them
/// a pair between siblings that a bulk reset made `overlapping`. See
/// `Command::RecreateRun`.
///
/// **55 since W9a Task 11**, and so `MISMATCH_ON_SPAWNED` 15 -> 13, by the same
/// two entries: `valids/loop-values-from-uri` banked, and
/// `valids/loop-outputs-auto` -- still baselined, its `end` FAILS on the
/// `Loop`'s own `outputs:`, which egret does not produce -- now agrees on
/// `order` and `spawned`, because its `Loop` iterates the ION file instead of
/// failing at `CREATED`.
///
/// **53 since W7b Task 4b**: two of the three `Pause` entries of
/// [`RUNS_REFUSED`] carried an ordered pair.
///
/// **19 since no-silent-stall Task 1**: 34 of the 49 entries it added.
// 19 -> 20 at WorkingDirectory Task 8 (measured).
const MISMATCH_ON_ORDER: usize = 20;
/// The `runs` entries egret refuses on purpose, in golden order: each is a
/// flow Kestra ran holding a task egret cannot run, which before its refusal
/// was an opaque runnable that stalled the execution for ever.
///
/// * three since the user's ruling of 2026-10-11 (W7b Task 4b): a `Pause`
///   declaring `behavior:`, `errors:` or `onPause:`
///   (`unsupported_pause_property`);
/// * forty-five since no-silent-stall Task 1 (2026-10-12): four `AllowFailure`
///   (`unsupported_allow_failure`), and forty-one holding a type
///   `spec::registry` does not name (`unknown_task_type`: `OutputValues`,
///   `Labels`, `SetVariables`, `UnsetVariables`, `Exit`, `Assert`,
///   `PurgeLogs`). `executor`'s `PUBLISHED_BUT_REFUSED` holds each with its
///   cause; it has three more rows, flows with no golden entry.
// 49 -> 45 at WorkingDirectory Task 3 (not Task 8): its four `WorkingDirectory`
// valids are planned now, so `unsupported_working_directory` refuses none of them.
const RUNS_REFUSED: [&str; 45] = [
    "valids/after-execution",
    "valids/after-execution-error",
    "valids/after-execution-finally",
    "valids/assert",
    "valids/exit",
    "valids/exit-cancelled",
    "valids/exit-killed",
    "valids/exit-nested",
    "valids/flow-concurrency-queue-after-execution",
    "valids/flow-concurrency-queue-killed-after-execution",
    "valids/flow-with-array-outputs",
    "valids/if-in-parallel",
    "valids/labels-update-nested-value-list",
    "valids/labels-update-nested-value-map",
    "valids/labels-update-task",
    "valids/labels-update-task-deduplicate",
    "valids/labels-update-task-empty",
    "valids/npe-labels-update-task",
    "valids/output-values",
    "valids/pause-behavior",
    "valids/pause-errors-finally-after-execution",
    "valids/pause_on_pause",
    "valids/primitive-labels-flow",
    "valids/purge_logs_execution_only",
    "valids/purge_logs_full_arguments",
    "valids/purge_logs_no_arguments",
    "valids/purge_logs_trigger_only",
    "valids/purge_logs_with_batch_size",
    "valids/replay",
    "valids/retry-with-flowable-errors",
    "valids/set-variables",
    "valids/set-variables-duplicate",
    "valids/sleep-long",
    "valids/task-runif",
    "valids/task-runif-executionupdating",
    "valids/task-runif-taskswithstate",
    "valids/unset-variables",
    "valids/waitfor",
    "valids/waitfor-child-task-warning",
    "valids/waitfor-max-iterations",
    "valids/waitfor-multiple-tasks",
    "valids/waitfor-multiple-tasks-failed",
    "valids/waitfor-no-success",
    "valids/webhook-outputs",
    "valids/webhook-wait",
];
/// **11 since no-silent-stall Task 1**: two of the 49 entries it added to
/// [`RUNS_REFUSED`] carried a sub-execution.
// 11 -> 12 at WorkingDirectory Task 8 (measured).
const MISMATCH_ON_SPAWNED: usize = 12;
/// **122 through W8 Task 2, 114 after W8 Task 3, 113 now** -- the eight `Pause`
/// entries Task 3 banked and the one `LoopUntil` entry Task 4 did. The
/// `refused` half of the golden is untouched by either: no `Pause` or
/// `LoopUntil` flow is a refusal except `invalids/workingdirectory-invalid`,
/// which egret still refuses and for the same reason (`is_runnable_child`
/// answers `false` for a typed kind, and `models::TaskKind::Pause` is one --
/// as, since W8 Task 4, is `models::TaskKind::LoopUntil` for BOTH its
/// spellings). **104 since W8 Task 6**, whose two entries are `runs` entries
/// and whose flows are not refusals either.
/// **103 since W9a Task 11** (`valids/loop-values-from-uri`).
/// **100 since the AllowFailure flowable**: its three targets match.
const MISMATCHING_RUNS_ENTRIES: usize = 100;
/// **84 through W5; 63 after W6 Task 3, and 59 since W6 Task 4 corrected the
/// classifier.**
///
/// # The four W6 Task 4 removed were never stalled, and the count said they were
///
/// This constant's assertion says every member is *a task type this worker has
/// no handler for*. The four `retry-new-execution-*` entries declare only
/// `io.kestra.plugin.core.execution.Fail`, which **has** a handler and ran to a
/// `FAILED` outcome; what put them here was [`is_stalled`] omitting `RETRIED`
/// from its terminal list — see [`TERMINAL`]. So this is a **classifier
/// artefact leaving the population**, not four executions that started
/// finishing: the count would have fallen to 59 with `RETRIED` added and no
/// replay implemented at all, and the entries would still not have matched.
/// The bank is a separate measurement in the ratchet.
///
/// # 21 left at Task 3 and they were the whole of `Subflow`
///
/// Through W5 a `Subflow` was planned an opaque `TaskKind::Runnable`, which set
/// `task_runs.type_id`, which made it claimable -- and `claim::leaf` then
/// answered `None` for it, because `models::TaskKind` for the same task is not
/// a `Leaf`. `WorkerError::NotALeaf` aborted the tick and the execution stalled
/// there for ever. All 21 entries holding an executed top-level `Subflow` were
/// in this population for that reason and none is now: a `TaskKind::Executable`
/// gets no `type_id`, so nothing claims it, and `worker::subflow` terminates it.
///
/// The 21 are the 12 this commit banks plus nine that still mismatch on
/// something else -- five registered in `STILL_OVER_ACCEPTED_AT_RUNTIME`, two
/// held by `state::apply`'s attempt rule (`valids/retry-subflow`,
/// `valids/subflow-parent-retry`) and two by the missing `WARNING`
/// (`valids/task-allow-{{failure,warning}}-executable-flow`).
/// **59 through W8 Task 2, 50 now, and the fall is NINE where eight entries
/// banked.** The eight `Pause` entries W8 Task 3 banked were all stalling for
/// the same reason -- a `Pause` was planned `Runnable`, so it got a `type_id`,
/// so the claim took it, so `Registry::run` refused a type nothing handles and
/// the tick aborted with the run leased and unmoved. The ninth is
/// `valids/pause-timeout-allow-failure`, which is planned `TaskKind::Pause` too
/// and therefore FINISHES now: its pause times out, the run ends `FAILED`, and
/// the entry mismatches on `WARNING` rather than stalling. **A bank and an
/// unstall are different events and this figure counts the second**, which is
/// why the two numbers differ by one and why that is worth a sentence rather
/// than an arithmetic reconciliation.
///
/// **50 through W8 Task 3, 46 now, and the fall is FOUR where ONE entry
/// banked.** Same distinction, on a different construct: a `LoopUntil` was
/// planned `Runnable`, so it got a `type_id`, so the claim took it and
/// `Registry::run` refused a type nothing handles. All nine entries that reach
/// the construct stopped stalling for that reason; **five of them started
/// stalling again one run lower down**, on the
/// `io.kestra.plugin.core.output.OutputValues` child their bodies now reach,
/// which is what leaves the net at four. The four that finish are
/// `valids/waitfor-nested` (banked), `valids/loop-until-restart`,
/// `valids/loopuntil-failed-flowable` and `valids/waitfor-child-task-warning`
/// -- the last of those has an `OutputValues` child too and finishes anyway,
/// because its FIRST child fails and `childTaskRunExecuted` ends the loop
/// before the second is ever reached.
///
/// **46 through W8 Task 4, 47 now, and the entry that joined is the last one
/// named above** — W8 Task 5. `valids/waitfor-child-task-warning`'s first child
/// is an `allowFailure: true` `Fail`, so its run is `WARNING` rather than
/// `FAILED` now, `isTerminatedNoFail` counts it as executed, and the loop goes
/// on to the `OutputValues` child it used to stop short of. **A bank moving a
/// count the other way is not a regression**: the entry is reproduced strictly
/// further than it was, and what it stalls on is a missing handler that was
/// always going to be its blocker — `handler_surface.py` carries that blocker
/// row, and the paragraph above is the reasoning it falsifies.
///
/// **45 since W9a Task 11**: `valids/loop-values-from-uri` and
/// `valids/loop-outputs-auto` stalled at `write`, a
/// `io.kestra.plugin.core.storage.Write` the driver had no handler for; both
/// finish now.
///
/// **42 since W7b Task 4b**: the three `Pause` entries of [`RUNS_REFUSED`]
/// stalled at their opaque `Pause` run (the worker's `NotALeaf`, the very
/// defect the user's ruling of 2026-10-11 closes) and are refused at start now.
///
/// **0 since no-silent-stall Task 1, and that is the package's point measured
/// on the corpus.** Every one of the 42 stalled on a task no handler serves:
/// an `OutputValues`/`Labels`/`SetVariables`/... leaf (`unknown_task_type`), or
/// an `AllowFailure`/`WorkingDirectory` planned opaque. Each of those is a
/// static refusal now, so the flow is refused at start ([`RUNS_REFUSED`]) and
/// no execution is left mid-flight. The derivation above, "every one holds a
/// type this worker has no handler for", now runs over an empty population;
/// the count is what keeps it honest, because the first entry to stall again
/// moves it.
const ENTRIES_LEFT_UNFINISHED: usize = 0;

/// **The guard W3a deleted did NOT come back, and this is why — measured, not
/// assumed.**
///
/// Kestra's `resolveParallelNexts` returns nothing while any existing child run
/// of the same branch is still `CREATED` ("first created, leave"). W3a ported
/// it, could not make the bypassing mutation kill anything, and deleted it with
/// the reason recorded: *"Deleting it is right for today's only caller. It is
/// not a property of `advance`… the trigger to watch for is W4's worker, which
/// is **message-driven** by construction."* It instrumented the decision point
/// over all 294 entries and measured **0** under its own harness and
/// `valids/dag` 1 batch / `valids/if` 3 under a driver that applies one worker
/// result BETWEEN `advance()` calls. The W3b brief says to expect it back.
///
/// **It is still 0, and the reason is a real difference between the driver W3a
/// predicted and the one W3b built.** W3a's prediction is about a caller that
/// applies a command batch **partially**. `worker::claim::settle` never does:
/// it applies every command of one `advance()` round before asking again, and
/// the whole cascade — the result, the rounds, the successors and the spawns —
/// is one transaction. A worker result is therefore never delivered *between*
/// two rounds of one batch; it is delivered before the first. W3b's driver is
/// transaction-driven, not message-driven, and the state the guard defends
/// against needs the second.
///
/// **A zero is only a measurement if the instrument can move**, which is the
/// shape the deleted guard itself had. `worker::claim`'s
/// `the_create_beside_a_created_sibling_counter_can_fire` is the positive
/// control: it drives the decision point directly, asserts the counter
/// increments, and asserts on the same state that a child of a *different*
/// parent does not move it.
#[tokio::test(flavor = "multi_thread")]
async fn the_guard_w3a_deleted_did_not_come_back_and_the_reason_is_the_transaction() {
    let side = egret_side().await;
    println!("create-beside-a-CREATED-sibling: {:?}", side.guard);
    assert_eq!(
        side.guard,
        BTreeMap::new(),
        "W3a predicted `valids/dag` 1 and `valids/if` 3 for a MESSAGE-driven \
         driver. This one is transaction-driven: `settle` applies a whole \
         `advance()` round before asking again, so no batch is ever applied \
         partially. If this ever becomes non-empty, W3a's guard has to come \
         back -- with the test that makes it fire"
    );

    // And the entries W3a named do reach this driver, so the zero is not an
    // artefact of never running them. Both are still baselined, and what they
    // differ on is asserted by
    // `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`.
    for id in ["valids/dag", "valids/if", "valids/parallel-disabled-tasks"] {
        let entry = golden()
            .into_iter()
            .find(|e| e.id == id)
            .expect("the entry is in the golden");
        assert!(
            matches!(evaluate(side, &entry), Outcome::Trace(_)),
            "{id} must actually have been driven, or the zero above means nothing"
        );
    }
}

/// **The sub-executions are compared now, and this is what they cost.**
///
/// Until W3b Task 5 this test was named
/// `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` and
/// measured a hole: egret's `Trace::spawned` was `Vec::new()` unconditionally,
/// so 41 entries carried 142 sub-executions that nothing on egret's side
/// answered, and **39** of them differed on nothing else. It asserted that all
/// 41 were still baselined, because one of them matching would have meant the
/// comparison had stopped reading the field.
///
/// The driver runs the sub-executions, so the field is populated and the
/// question changes: of the 41, how many now agree, and what holds the rest?
/// Both halves are asserted — the golden-side population, which cannot move,
/// and the per-entry outcome, which is the measurement this task reports.
#[tokio::test(flavor = "multi_thread")]
async fn the_sub_executions_are_compared_and_this_is_what_they_cost() {
    let side = egret_side().await;
    let baseline = baseline();
    let mut with_spawned: Vec<String> = Vec::new();
    let mut sub_executions = 0usize;
    let mut agree: Vec<String> = Vec::new();
    let mut only_spawned: Vec<String> = Vec::new();

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

        let Outcome::Trace(actual) = evaluate(side, entry) else {
            continue;
        };
        let mut actual = actual.clone();
        let mut expected = Trace::from(entry);
        actual.restrict_order_to(&expected);
        if actual == expected {
            agree.push(entry.id.clone());
            continue;
        }
        // Blank BOTH sides' `spawned` and compare what is left, which is what
        // the old test did. It now answers a different question: an entry in
        // this list agrees on everything the parent execution holds and differs
        // SOMEWHERE inside a sub-execution.
        let mut a = actual.clone();
        a.spawned.clear();
        expected.spawned.clear();
        if a == expected {
            only_spawned.push(entry.id.clone());
        }
    }

    assert_eq!(
        (with_spawned.len(), sub_executions),
        (41, 142),
        "the golden's sub-execution population: 41 entries carrying 142 \
         sub-executions between them. Derived from `golden.json` and unmoved by \
         anything egret does."
    );
    println!(
        "sub-executions: {} of {} entries with a `spawned` now MATCH: {agree:?}",
        agree.len(),
        with_spawned.len()
    );
    println!(
        "still differing only inside `spawned`: {} {only_spawned:?}",
        only_spawned.len()
    );
    assert!(
        agree.iter().all(|id| !baseline.contains(id)),
        "an entry that matches must have been banked in this commit: {agree:?}"
    );
    // **The names, not only the count** — because `baseline.txt`'s header
    // attributes each of them and got two attributions wrong: it said `Fifteen
    // are the sub-execution hole closing` above SIXTEEN names, and W3b Task
    // 5's report said `fifteen … and one needed only the render surface`. The
    // fact that settles it is this list: every entry that banked at Task 5
    // carries a `spawned`, so none banked on the render surface alone. Pinning
    // the names is what stops that prose from drifting again.
    assert_eq!(
        agree, SPAWNED_ENTRIES_THAT_MATCH_BY_NAME,
        "the entries with a `spawned` that match, by name. `baseline.txt`'s \
         header attributes each of these; a name appearing or leaving here \
         without that header moving is the drift this list exists to catch"
    );
    assert_eq!(
        agree.len(),
        SPAWNED_ENTRIES_THAT_MATCH,
        "how many of the 41 the spawner earns. **This is the number the W3b \
         plan's Task 4 could not move**: it built the spawner and reported both \
         counts unchanged, because `tests/executor.rs` still drove the harness. \
         The count is asserted rather than printed so that a later change which \
         loses one has to say so."
    );
    // W3b Task 7. The second figure was PRINTED and not asserted, and the
    // coverage statement quotes it — a count written down beside a derivation,
    // which is the defect this programme has shipped eight times. The three
    // parts are asserted together with the total they must make.
    let rest = with_spawned.len() - agree.len() - only_spawned.len();
    assert_eq!(
        (
            agree.len(),
            only_spawned.len(),
            rest,
            agree.len() + only_spawned.len() + rest
        ),
        (
            SPAWNED_ENTRIES_THAT_MATCH,
            SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED,
            // 15 through W9a Task 10; `valids/loop-values-from-uri` left it; 14 ->
            // 13 at WorkingDirectory Task 8, when `working-directory-taskrun` matched.
            13,
            41
        ),
        "the 41 partitioned: how many MATCH, how many agree on everything the \
         PARENT execution holds and differ somewhere inside a sub-execution, and \
         how many differ on something else as well. The total is asserted beside \
         the parts because the coverage statement publishes all three"
    );
}

/// The count above, as a named constant so the report and the test cannot
/// disagree about it.
// 23 -> 24 at WorkingDirectory Task 8: `valids/working-directory-taskrun` banked.
const SPAWNED_ENTRIES_THAT_MATCH: usize = 24;

/// And the names, which is what `baseline.txt`'s header attributes one by one.
/// Sixteen banked at W3b Task 5, `valids/replay-loop` at the whole-branch
/// review's fix round, when a sub-execution began inheriting its parent's
/// `inputs`, and `valids/loop-with-subflow` at W6 Task 3 -- its loop body is a
/// `Subflow`, so the body ran and the `Subflow` inside it stalled.
const SPAWNED_ENTRIES_THAT_MATCH_BY_NAME: &[&str] = &[
    "valids/flowable-with-parent-fail",
    "valids/loop-expression-context",
    "valids/loop-failed",
    "valids/loop-failed-no-transmit",
    "valids/loop-flowable",
    "valids/loop-map",
    "valids/loop-multiple",
    "valids/loop-nested",
    "valids/loop-parallel-equal",
    "valids/loop-parallel-less",
    "valids/loop-parallel-more",
    "valids/loop-parallel-unlimited",
    "valids/loop-serial",
    "valids/loop-serial-multiple-tasks",
    "valids/loop-switch",
    // W9a Task 11: a `Loop` whose `values:` is the ION file a
    // `core.storage.Write` wrote -- read by the worker before `advance()`.
    "valids/loop-values-from-uri",
    // W6 Task 3's only bank that carries a sub-execution: its `Loop` body is a
    // `Subflow`, so the body ran but its `Subflow` run stalled -- the whole
    // entry hung on the one construct this task shipped.
    "valids/loop-with-subflow",
    "valids/replay-loop",
    // W6 Task 4, and they are the only four names here that are NOT a `Loop`.
    // Their sub-execution is the replacement a `CREATE_NEW_EXECUTION` retry
    // starts -- a third kind of child, linked by `parent_execution_id` with no
    // loop index at all (migrations 0138 and 0139) -- so each carries a chain
    // of two: `RETRIED` -> `RETRIED` -> `FAILED`.
    "valids/retry-new-execution-flow-attempts",
    "valids/retry-new-execution-flow-duration",
    "valids/retry-new-execution-task-attempts",
    "valids/retry-new-execution-task-duration",
    "valids/switch-in-concurrent-loop",
    // WorkingDirectory Task 8: banked (its one `Loop` body is a WorkingDirectory
    // whose children egret now runs and matches). `-nested` stays baselined.
    "valids/working-directory-taskrun",
];

/// W3a's `39 of the 41 differ on nothing but `spawned`` has this as its
/// successor, and it is a different sentence: egret fills the field now, so
/// these **four** differ **inside** it rather than by leaving it empty.
///
/// *(The word was "six" and the constant under it was 8, on adjacent lines,
/// since the whole-branch review's fix round moved `valids/finally-loop` and
/// `valids/finally-loop-parallel` into this bucket. W4 Task 6's fourth sweep
/// pass found it. The value was compared on every run and was right; the word
/// beside it was not compared by anything. W6 Task 4 wrote the word and the
/// value in one edit, from the same measurement.)*
///
/// **8 until W6 Task 4, and the four that left are the four it banked.** They
/// were in this bucket for the strongest possible version of "differs inside
/// `spawned`": everything the parent execution holds agreed and the child was
/// **absent**, because nothing created a replay. The remaining four are
/// `invalids/loop-switch-failed`, `valids/finally-loop`,
/// `valids/finally-loop-parallel` and `valids/retry-flowable-parallel` -- all
/// `Loop`s, none a replay -- and the test prints the list on every run.
const SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED: usize = 4;

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
#[tokio::test(flavor = "multi_thread")]
async fn no_run_is_ever_created_for_a_task_inside_a_loop() {
    let side = egret_side().await;
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

        // egret's side, through the real driver.
        if let Outcome::Trace(actual) = evaluate(side, entry) {
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
        (38, 82),
        "the population: 36 comparable entries hold a `Loop` that egret plans, \
         with 75 tasks inside a loop body between them. Both are derived; if \
         they move, the `Loop` slice of the corpus has changed. It was (39, 81) \
         until no-silent-stall Task 1, which refuses three `Loop`-holding \
         entries at start. It moved once before, from 82: the \
         final review's `disabled:` deletes `valids/disable-flowable`'s \
         `1-each-2` from the plan, so the body it is in has one task fewer. The \
         golden half of this test is unchanged, because Kestra never created a \
         run for it either"
    );
}

/// **Every sub-execution egret runs is one Kestra ran, matched by
/// `(parent run, loop index)`, over the whole corpus.**
///
/// The test this replaces was `every_first_batch_egret_spawns_is_a_prefix_of_
/// what_kestra_ran`, and a **prefix** was all it could ask for: `advance()`
/// emits one batch and nothing executed it, so egret's side was a list of
/// requests rather than a list of sub-executions. The driver runs the whole
/// loop life cycle, so the relation to assert is now equality of the ORDERED
/// KEY LIST — which is the field `normalise.py` sorts `spawned` by, and the
/// only thing that decides the order at all.
///
/// **A count of matches is not enough, and that is the lesson from the
/// prefix version**: a spawner that ran one iteration of everything would
/// satisfy "every one egret ran is one Kestra ran". So the assertion is on the
/// per-entry `(parent_run, value)` sequence, side by side, and the entries
/// where the two lists differ are named rather than summed.
#[tokio::test(flavor = "multi_thread")]
async fn every_sub_execution_egret_runs_is_one_kestra_ran() {
    let side = egret_side().await;
    fn keys(t: &Trace) -> Vec<(Option<usize>, Option<String>)> {
        t.spawned
            .iter()
            .map(|s| (s.parent_run, s.value.clone()))
            .collect()
    }

    let mut compared = 0usize;
    let mut identical: Vec<String> = Vec::new();
    let mut differ: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        if entry.spawned.is_empty() {
            continue;
        }
        let Outcome::Trace(actual) = evaluate(side, entry) else {
            continue;
        };
        compared += 1;
        let expected = Trace::from(entry);
        if keys(actual) == keys(&expected) {
            identical.push(entry.id.clone());
        } else {
            differ.insert(
                entry.id.clone(),
                (keys(actual).len(), keys(&expected).len()),
            );
        }
    }

    println!(
        "top-level spawned keys: {} of {compared} entries identical; \
         differing (egret, kestra): {differ:?}",
        identical.len()
    );
    assert_eq!(
        (identical.len(), differ.len(), compared),
        (
            TOP_LEVEL_SPAWN_KEYS_IDENTICAL,
            TOP_LEVEL_SPAWN_KEYS_DIFFER,
            40
        ),
        "the totals asserted beside the parts. Both figures are what W3b Task 4 \
         built the spawner for and could not measure, because the oracle still \
         drove a harness that spawned nothing. 41 compared until no-silent-stall \
         Task 1 refused three of them at start."
    );
}

/// **Over-acceptance at RUNTIME, which had no register.**
///
/// `STILL_OVER_ACCEPTED` is publish-only: flows Kestra refused and egret plans.
/// W3b creates the first *runtime* instances — egret running something Kestra
/// did not — and every one of them is named somewhere in this repository while
/// none was counted as over-acceptance. The plan's Global Constraint says
/// *"over-acceptance must not ship"*, so the class needs a register of its own
/// or the constraint is about half of it.
///
/// Three shapes, all derived from the two traces rather than read off a list:
///
/// - egret produced **more runs** than Kestra recorded;
/// - egret produced **more sub-executions** than Kestra recorded;
/// - a run Kestra ended `KILLED`/`FAILED` that egret ended `SUCCESS` or
///   `WARNING` — i.e. egret completed work Kestra stopped.
///
/// A **stalled** execution is deliberately not one of these: egret doing LESS
/// than Kestra is under-acceptance, which the ratchet already reports as a
/// mismatch and `ENTRIES_LEFT_UNFINISHED` already counts.
#[tokio::test(flavor = "multi_thread")]
async fn runtime_over_acceptance_is_registered_and_owned() {
    let side = egret_side().await;
    let mut found: BTreeMap<String, String> = BTreeMap::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let Outcome::Trace(actual) = evaluate(side, entry) else {
            continue;
        };
        let expected = Trace::from(entry);
        let mut why: Vec<String> = Vec::new();
        if actual.runs.len() > expected.runs.len() {
            why.push(format!(
                "runs ({} egret, {} kestra)",
                actual.runs.len(),
                expected.runs.len()
            ));
        }
        if actual.spawned.len() > expected.spawned.len() {
            why.push(format!(
                "sub-executions ({} egret, {} kestra)",
                actual.spawned.len(),
                expected.spawned.len()
            ));
        }
        let stopped = |s: &str| s == "KILLED" || s == "FAILED";
        let completed = |s: &str| s == "SUCCESS" || s == "WARNING";
        for (a, e) in actual.runs.iter().zip(expected.runs.iter()) {
            let (Some(av), Some(ev)) = (a.state.last(), e.state.last()) else {
                continue;
            };
            if completed(av) && stopped(ev) {
                why.push(format!("run {:?} {ev} -> {av}", a.task));
            }
        }
        if !why.is_empty() {
            found.insert(entry.id.clone(), why.join("; "));
        }
    }

    println!("runtime over-acceptance, derived: {found:#?}");
    let names: Vec<&str> = found.keys().map(String::as_str).collect();
    // Sorted for the comparison, grouped by OWNER in the table itself — the
    // same rule `UNATTRIBUTED` follows, and for the same reason: the derivation
    // arrives sorted and only one of the two can have both properties.
    let mut registered: Vec<&str> = STILL_OVER_ACCEPTED_AT_RUNTIME
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let before_dedup = registered.len();
    registered.sort_unstable();
    registered.dedup();
    assert_eq!(
        registered.len(),
        before_dedup,
        "an id named twice would be two owners for one entry, and the \
         comparison below would not see it"
    );
    assert_eq!(
        names, registered,
        "every entry where egret ran something Kestra did not must be in \
         STILL_OVER_ACCEPTED_AT_RUNTIME with an owner. This is the class the \
         plan's Global Constraint forbids shipping, and until this test existed \
         it had no register at all -- `STILL_OVER_ACCEPTED` is publish-only"
    );
}

/// The register the assertion above compares against: `(entry, owner)`.
///
/// **None of these is new information** — each is named in `baseline.txt`'s
/// header or in this file — and that is the point: they were named as
/// divergences and never counted as the class they belong to.
const STILL_OVER_ACCEPTED_AT_RUNTIME: &[(&str, &str)] = &[
    // **W2: egret RENDERS an expression Kestra's renderer throws on**, so the
    // run succeeds and everything after it runs too. `{{ taskrun.value }}` is
    // undefined inside a `Loop` body and a `Switch` case in this artifact —
    // Kestra FAILED both — and `{{ 1 / taskrun.attemptsCount }}` is Java's
    // `ArithmeticException: / by zero` against minijinja's float `Infinity`.
    // The last is the one `baseline.txt` names for `restart-child`; the other
    // three are the same class and were named nowhere.
    (
        "valids/all-flowable",
        "W2: `{{ taskrun.value }}` in a `Loop` body renders where Kestra throws, so `1-each` \
         succeeds and egret runs 3 iterations and 12 runs where Kestra ran 1 and 3",
    ),
    (
        "valids/disable-flowable",
        "W2: `{{ taskrun.value }}` in a `Switch` case renders where Kestra throws",
    ),
    (
        "valids/restart-child",
        "W2: `{{ 1 / taskrun.attemptsCount }}` renders `Infinity` where Java throws, so the run \
         SUCCEEDS and egret runs one more task",
    ),
    // WorkingDirectory Task 8, measured on the re-harvested golden: the child `s2`'s
    // `runIf: {{ outputs.failed }}` cannot render (the render context has no
    // `outputs`, as `valids/task-runif` shows), so egret creates a FAILED run for
    // it and fails the block after `s1`. Kestra creates no run for `s2` and
    // fails the block without an attempt. The entry is baselined.
    (
        "valids/working-directory-invalid-runif",
        "WorkingDirectory: `s2`'s `runIf:` render fails, so egret creates a FAILED run for it; \
         Kestra creates none and fails the block without an attempt (the context has no `outputs`)",
    ),
    (
        "valids/restart-loop",
        "W2: the same `Infinity`, inside a `Loop`, so egret runs 3 iterations where Kestra ran 1",
    ),
    // **W4 Task 3 wired `TaskHandler::cancel`, and deliberately not to this
    // seam.** The kill path fires on a task-level `timeout:` expiring, which is
    // the artifact's only `kill(null)` site
    // (`AbstractWorkerCallable.callWithTimeout`). A run that a FLOWABLE
    // abandoned is `advance()`'s decision, and `advance()` has no worker to
    // tell — so work Kestra stopped still runs to completion here.
    // `worker::cancel`'s module doc carries the same statement.
    (
        "valids/parallel-fail-with-flowable",
        "W4: a `Sleep` still in flight when its sibling failed the `Parallel`. Kestra FAILED it; \
         the kill path reaches only a task's own expired `timeout:`, so egret completes it",
    ),
    // **W8, through `sla:` alone.** Kestra CANCELLED or KILLED the execution;
    // egret ran the tasks. This comment said "`sla:` and flow `concurrency:`,
    // both rows of the residual baseline", and both halves went at W8 Task 1:
    // every one of the five was an `sla:`'s, and the `flow concurrency:` row no
    // longer exists -- it was deleted for attributing `CANCELLED` to a
    // construct that never produces it. See
    // `the_cancelled_state_never_comes_from_a_concurrency_limit`.
    //
    // **W8 Task 6 took the five to THREE, and the split is the whole point of
    // that task.** The two that left --
    // `valids/flow-concurrency-sla-assert-cancel` and
    // `valids/sla-execution-condition` -- are the `EXECUTION_ASSERTION`s, and
    // egret now cancels the execution before any run exists, so it no longer
    // runs anything Kestra did not. The three that remain are `MAX_DURATION`s
    // and **no implementation can take them off this list under this harness**:
    // `driver.rs` builds its registry with `Sleep::capped(Duration::ZERO)`, so
    // the `Sleep` these flows are built around finishes inside the tick that
    // started it and the window `MaxDurationSLA.evaluate` measures does not
    // exist. They are over-acceptances of the HARNESS, not of egret, and that
    // distinction is why they stay registered rather than being excused.
    //
    // **Exactly ONE of the original five declares a `concurrency:`** --
    // `valids/flow-concurrency-sla-assert-cancel`, which is one of the two that
    // left. A first draft of this comment said three, counting flow IDs rather
    // than declarations, which is the name-versus-declaration confusion in the
    // very sentence announcing that the confusion was fixed.
    // `valids/flow-concurrency-sla-fail-child` carries the word in its **id**
    // and writes no `concurrency:` at all; the other three do not carry it even
    // there.
    (
        "valids/flow-concurrency-sla-fail-child",
        "W8: `MAX_DURATION` killed the `Sleep`; the harness's capped `Sleep` leaves no window for \
         one, so egret completes it",
    ),
    (
        "valids/sla-max-duration-fail",
        "W8: `MAX_DURATION` killed the `Sleep`; egret completes it",
    ),
    ("valids/sla-subflow", "W8: the same, through a subflow"),
    // **W8 Task 4: the same class as the four `W2:` rows at the top of this
    // list, on a new operator.** Kestra renders through Pebble, whose
    // `OperatorUtils` widens both operands of `<` to `Number` and throws on a
    // `String`; minijinja orders values across types and answers a boolean. So
    // the `LoopUntil`'s condition evaluates to `true` here, the loop SUCCEEDS
    // on its first iteration, and Kestra FAILED the task instead. It is a
    // RENDERER divergence wearing a loop's name, and it is the corpus's only
    // number-against-string comparison --
    // `the_one_corpus_comparison_pebble_refuses_and_minijinja_answers` is what
    // says so and what would fail if egret ever gained Pebble-strict operators.
    (
        "valids/loopuntil-failed-flowable",
        "W2/W8: `{{ outputs.loop.iterationCount < \"5\" }}` -- Pebble throws on a relational \
         operator against a quoted operand and minijinja answers `True`, so the loop's condition \
         is truthy, the loop SUCCEEDS on iteration 1 and Kestra FAILED the task",
    ),
    // **Unclaimed: a `Loop`'s own `outputs:`.** `advance()` does not implement
    // them, so a loop whose `outputs:` fails to render — Kestra FAILED it at
    // the first iteration — succeeds here and runs the other two. Named as a
    // cause of the residual baseline's `Loop` row and owned by nobody.
    (
        "valids/loop-outputs-failed-render",
        "unclaimed: a `Loop`'s `outputs:` reading `{{ outputs.nonExistentTask.value }}`. Kestra \
         FAILED the loop at iteration 1; egret runs 3",
    ),
    // **W6 made a `Subflow` execute, and these five are what that revealed: an
    // over-acceptance that was already registered on the CHILD, now visible in
    // its PARENT.** None of them is a new divergence. Before W6 Task 3 the
    // parent's `Subflow` run stalled -- `build_plan` planned it an opaque
    // `Runnable`, the claim took it, and `WorkerError::NotALeaf` aborted the
    // tick -- so the parent could not get as far as running anything Kestra did
    // not. It can now, and what it then does is transmit its child's terminal:
    // the child SUCCEEDS here because of the row above or below it in this very
    // list, `transmitFailed` faithfully carries that up, and the parent runs
    // its successor.
    //
    // The owner of each is therefore the CHILD's row, named in the message, and
    // fixing that row fixes this one with it. Registered separately rather than
    // folded into the child's entry because this test compares entry ids and a
    // parent that is not named here is indistinguishable from one nobody
    // looked at.
    (
        "valids/flow-concurrency-sla-fail-parent",
        "W8, through a subflow: its child `valids/flow-concurrency-sla-fail-child` is over-accepted \
         above (no `sla:`), so the child SUCCEEDS, `transmitFailed` carries SUCCESS up, and the \
         parent's `subflow` run SUCCEEDS where Kestra FAILED it",
    ),
    (
        "valids/sla-parent-flow",
        "W8, through a subflow: its child `valids/sla-subflow` is over-accepted above, so the \
         parent's `subflow` run SUCCEEDS where Kestra FAILED it",
    ),
    (
        "valids/restart-parent",
        "W2, through a subflow: its child `valids/restart-child` is over-accepted above (the \
         `Infinity` render), so the child SUCCEEDS and egret runs one more task in the parent",
    ),
    (
        "valids/restart-parent-loop",
        "W2, through a subflow inside a `Loop`: the same `valids/restart-child`, reached from a \
         loop sub-execution, so egret runs 3 sub-executions and 3 runs where Kestra ran 1 and 2",
    ),
    // **The only one of the five that is a new INSTANCE and not only a newly
    // visible one**, which fix round 1's review separated: the other four are
    // owned by a row already in this list, so the child was over-accepted
    // before and the parent merely could not reach it. This one has no
    // pre-existing row -- its cause is a construct nothing in egret evaluates,
    // and the run that now SUCCEEDs where Kestra FAILED it did not exist while
    // the parent stalled. No capability was added that fires where Kestra would
    // not; a run was.
    (
        "valids/subflow-secret-output-parent",
        "unclaimed: the flow-level `outputs:` block is not evaluated anywhere in egret, so a child \
         declaring `type: SECRET` outputs simply SUCCEEDS. On the artifact the PARENT fails inside \
         `ExecutableUtils.subflowExecutionResultFromChildExecution`'s catch block while extracting \
         them -- which is what gives its golden run `[CREATED, RUNNING, FAILED]` beside an attempt \
         of `[CREATED, FAILED]` -- so egret's `subflow` run SUCCEEDS and its `hello` successor runs",
    ),
];

// W6 Task 4: +4 identical / -4 differing, and they are the four
// `retry-new-execution-*` entries. Their key is `(None, None)` -- a replay
// carries neither a parent run nor a loop index -- and egret produced an EMPTY
// key list for each until `worker::replay` created the execution.
// W9a Task 11: +2 identical / -2 differing, `valids/loop-values-from-uri` and
// `valids/loop-outputs-auto`: their `Loop` iterates its ION file now, where it
// failed at `CREATED` and spawned nothing.
// no-silent-stall Task 1: -3 identical, three entries whose keys matched and
// that are refused at start now (`RUNS_REFUSED`), so egret produces no key
// list for them at all.
const TOP_LEVEL_SPAWN_KEYS_IDENTICAL: usize = 33;
const TOP_LEVEL_SPAWN_KEYS_DIFFER: usize = 7;

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
/// [`every_sub_execution_egret_runs_is_one_kestra_ran`] would fail on each. So
/// the default is guarded by a corpus assertion and not only by a line of Java.
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
         and `every_sub_execution_egret_runs_is_one_kestra_ran` would fail five \
         times"
    );
    assert_eq!(
        defaulted, 23,
        "the population: top-level `Loop`s in the comparable corpus that write no \
         `concurrencyLimit` and resolve at least one value, in a flow egret plans \
         (24 until no-silent-stall Task 1 refused one such flow at start)"
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
    // Task 7 widened `build_plan` again, so today's plan is no longer Task 5's
    // in one further respect: a task written inside an `errors:` list has a
    // `PlanTask` now and had none then. That movement is DERIVED here rather
    // than absorbed into the constants below, because the constants are what
    // makes the 40 a measurement -- swapping them for whatever today prints
    // would leave this test agreeing with itself.
    let mut in_an_errors_list = 0usize;
    // **W6 widened it once more, and this is the movement that IS the package's
    // finding.** A `Subflow` was a `TaskKind::Runnable` through W5, so every
    // one of its runs landed in `fed` AND in `over_fed` -- fed because
    // `golden_outcome` supplied its terminal, over-fed because `advance()`
    // emitted a `SUBMITTED` no `Subflow` run in `golden.json` carries. It is a
    // `TaskKind::Executable` now, so those runs are `flowable` and the
    // `SUBMITTED` is gone. Derived here rather than folded into the constants
    // below, for `in_an_errors_list`'s reason: constants swapped for whatever
    // today prints leave this test agreeing with itself.
    let mut is_a_subflow = 0usize;
    // **W7b Task 4b (the user's ruling of 2026-10-11) narrowed it.** A `Pause`
    // declaring `behavior:`/`errors:`/`onPause:` was planned an opaque
    // `Runnable` -- `fed` and over-fed -- and is now refused at plan
    // (`unsupported_pause_property`), so its runs land in `unplanned`. Derived
    // here, for `in_an_errors_list`'s reason, and added back below.
    let mut refused_pause_runs = 0usize;

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        // **no-silent-stall Task 1's refusals are reconstructed away**, unlike
        // W7b's `Pause` one above: the 49 entries they refuse at start were
        // planned by every builder this test reconstructs, so they are read
        // from the flattening alone (`plan_stored`) and every figure below
        // stays the one its package committed.
        let planned = match build_plan(&corpus_source(&entry.id)) {
            Err(e)
                if [
                    "unsupported_allow_failure:",
                    "unsupported_allow_failure_retry:",
                    "unsupported_working_directory:",
                    "unknown_task_type:",
                ]
                .iter()
                .any(|c| e.message.starts_with(c)) =>
            {
                egret_workflow::executor::plan_stored(&corpus_source(&entry.id))
            }
            other => other,
        };
        if let Err(e) = &planned {
            if e.message.starts_with("unsupported_pause_property:") {
                // The pause's own runs: those were `fed` and over-fed. Every
                // other run of the entry was `unplanned` already.
                let flow = egret_workflow::spec::parse(&corpus_source(&entry.id))
                    .expect("a refused-at-plan entry still parses");
                let pauses: Vec<&str> = flow
                    .tasks
                    .iter()
                    .filter(|t| t.r#type.ends_with("flow.Pause"))
                    .map(|t| t.id.as_str())
                    .collect();
                refused_pause_runs += trace
                    .runs
                    .iter()
                    .filter(|r| pauses.contains(&r.task.as_str()))
                    .count();
            }
        }
        let Ok(plan) = planned else {
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
                // A TASK-level `errors:` only. The flow's own `errors:` was
                // already in the plan at Task 3 -- `Plan::errors` is populated
                // from `flow.errors` -- so its runs were never `unplanned` and
                // are not what moved.
                if plan
                    .tasks
                    .iter()
                    .position(|p| p.id == run.task)
                    .is_some_and(|i| plan.tasks.iter().any(|t| t.errors.contains(&i)))
                {
                    in_an_errors_list += 1;
                }
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
                    Some(TaskKind::Executable { .. }) => {
                        flowable += 1;
                        is_a_subflow += 1;
                        assert!(
                            !submitted,
                            "{}: `Subflow` run {} carries SUBMITTED in the GOLDEN, which would \
                             refute the whole reason W6 opened src/executor/",
                            entry.id, run.task
                        );
                    }
                    Some(_) => flowable += 1,
                }
            }
            stack.extend(t.spawned.iter());
        }
    }

    assert_eq!(
        in_an_errors_list, 14,
        "the runs Task 7 alone added to the plan: every one is a task written in \
         a TASK-level `errors:` list, which W1's model dropped until Task 7 \
         widened it. The flow's own `errors:` is not among them -- `Plan::errors` \
         has been populated since Task 3. The four more are the AllowFailure \
         `errors:` runs of `valids/allow-failure` (`local-error`), \
         `valids/allow-failure-with-retry` (`error_handler`) and \
         `valids/finally-allowfailure` (`e1`, `e2`): W1's model dropped those \
         lists too, and spec §5.2 gave the flowable its own."
    );
    assert_eq!(
        is_a_subflow, 22,
        "every `Subflow` run in the comparable golden: 21 that executed, across \
         20 entries, plus `valids/task-runif`'s `notexecutedflowable`, which a \
         falsy `runIf:` SKIPPED. All 22 were `fed` and over-submitted through \
         W5; none of them is either since W6 planned `Subflow` as \
         `TaskKind::Executable`"
    );
    assert_eq!(
        refused_pause_runs, 3,
        "`valids/pause-behavior`, `valids/pause-errors-finally-after-execution` \
         and `valids/pause_on_pause`: one `Pause` run each, fed and over-fed \
         while it was planned opaque"
    );
    assert_eq!(
        (
            fed - in_an_errors_list + is_a_subflow + refused_pause_runs,
            flowable - is_a_subflow,
            unplanned + in_an_errors_list - refused_pause_runs,
            over_fed.values().sum::<usize>() + is_a_subflow + refused_pause_runs
        ),
        (438, 109, 311, 104),
        "the reconstruction must reproduce Task 5's four committed figures -- up \
         to the 10 runs Task 7's `errors:` support moved out of `unplanned` and \
         into `fed`, which is derived above rather than folded into these \
         numbers, and up to the one the null-list fix moved out of `unplanned` \
         and into `flowable` (`valids/switch-impossible`'s `impossible` run: the \
         flow is planned at all now, so its `Switch` is a flowable rather than a \
         task with no plan), and up to the 22 W6 moved out of `fed` and out of \
         the over-feed at the same time, and up to what W8 Task 4 moved by \
         planning `LoopUntil`/`WaitFor`. **That last one is the finding this \
         test keeps re-finding, from the other side**: implementing a flowable \
         SHRINKS the over-feed by its own runs and ENLARGES `fed` by whatever \
         was nested inside it. Derived over the nine entries that reach the \
         construct, which hold 27 runs between them: the 9 top-level loop runs \
         left `fed` and the over-feed together (105 -> 96), all 11 loop runs \
         plus `loopuntil-failed-flowable`'s nested `Dag` are `flowable` now \
         (83 -> 95), and the 18 runs that were inside an opaque loop left \
         `unplanned` (342 -> 324), 15 of them into `fed` (415 -> 421). If any of \
         this does not hold, nothing below is a measurement of anything. **The AllowFailure \
         flowable moves it once more, to 425/101/314/92**: its three targets plan \
         as flowables, so each block leaves `fed` and its over-feed for `flowable` \
         and each nested run that was `unplanned` is `fed` (their `errors:` runs \
         enter `in_an_errors_list`, 10 -> 14), a net of (+4, +5, -9, -3) over \
         (fed - in_errors, flowable, unplanned + in_errors, over-feed). \
         `valids/retry-with-flowable-errors` is refused by its block-level retry \
         and stays on the `plan_stored` path, now with its block planned: \
         (0, +1, -1, -1)."
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
    // W6 took a whole TYPE out of this bucket as well as 22 runs, so the `+ 1`
    // is checked rather than assumed: `Subflow` must be absent from the map
    // entirely, which is what a type that is no longer fed looks like.
    assert!(
        !over_fed.contains_key("io.kestra.plugin.core.flow.Subflow"),
        "`Subflow` must contribute NO over-submitted run since W6 planned it \
         `TaskKind::Executable`; it contributed {:?}",
        over_fed.get("io.kestra.plugin.core.flow.Subflow")
    );
    assert_eq!(
        (
            flow_then + is_a_subflow + refused_pause_runs,
            over_fed.len() + 1 + usize::from(refused_pause_runs > 0)
        ),
        // AllowFailure left the over-feed: its four block runs are flowables now
        // (the three targets, and `valids/retry-with-flowable-errors`), and its
        // type left the map.
        // Re-measured at WorkingDirectory Task 8: `shell.Commands` joins the
        // over-fed types (its six real-flow children carry no SUBMITTED in the
        // golden), and the flow bucket moves with the banked `working-directory-taskrun`.
        (65, 10),
        "Task 5's flow bucket, reconstructed: 7 types and 89 runs, of which \
         `io.kestra.plugin.core.flow.Subflow` was one type and 22 runs until W6 \
         -- and of which `LoopUntil` (3 runs) and `WaitFor` (6) were two more \
         types and 9 more runs until W8 Task 4 planned them. 80 -> 71 and \
         12 -> 10, the same movement the four-bucket assertion above records \
         from the other side. `io.kestra.plugin.core.flow.Pause` left as a type \
         and 3 runs at W7b Task 4b (refused at plan) and is added back here"
    );
    assert!(
        !over_fed.contains_key("io.kestra.plugin.core.flow.Pause"),
        "no `Pause` is planned opaque since W7b Task 4b: {:?}",
        over_fed.get("io.kestra.plugin.core.flow.Pause")
    );
}

#[test]
fn the_state_vocabulary_this_enum_holds_is_derived_from_the_golden() {
    // **Derived from `RunState` itself, and it was NOT until W3c.** This was a
    // literal list of eight names, so the test compared the golden against a
    // list in this file and never once looked at the enum: adding `Skipped` to
    // `RunState` left it passing. `RunState::ALL` is now the single source, and
    // the four numbers below are what move when it does.
    let held: BTreeSet<&str> = RunState::ALL.iter().map(|s| s.wire()).collect();
    // Task 3 drafted six; `SUBMITTED` was its own addition, `RETRIED` is Task
    // 7's and `SKIPPED` is W3c's. All are kept here so the "how much did that
    // one state buy" question stays answerable.
    // **Every tier is `held` MINUS the states that arrived after it**, so each
    // one has to name every later addition. W8 Task 6 is the first addition
    // that had to be written into all six lines rather than one: adding
    // `CANCELLED` to the enum and to none of these filters silently promoted it
    // into *Task 3's draft of six*, and every tier read three entries too high
    // (35 -> 38, 229 -> 232, 233 -> 236, 237 -> 240, 253 -> 256). The counts
    // below are what caught that; a `missing`-only assertion would not have.
    let drafted: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| {
            !matches!(
                *s,
                "SUBMITTED"
                    | "RETRIED"
                    | "SKIPPED"
                    | "PAUSED"
                    | "RESTARTED"
                    | "WARNING"
                    | "CANCELLED"
            )
        })
        .collect();
    let through_task_6: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| {
            !matches!(
                *s,
                "RETRIED" | "SKIPPED" | "PAUSED" | "RESTARTED" | "WARNING" | "CANCELLED"
            )
        })
        .collect();
    let through_w3a: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| {
            !matches!(
                *s,
                "SKIPPED" | "PAUSED" | "RESTARTED" | "WARNING" | "CANCELLED"
            )
        })
        .collect();
    let through_w3c: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| !matches!(*s, "PAUSED" | "RESTARTED" | "WARNING" | "CANCELLED"))
        .collect();
    // W8 Task 2's tier, which was `held` itself until W8 Task 5 added
    // `WARNING`. It is spelled out rather than left as "everything but the
    // newest" so that the chain below keeps one column per addition.
    let through_w8_task_2: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| !matches!(*s, "WARNING" | "CANCELLED"))
        .collect();
    // W8 Task 5's tier. `held` itself until W8 Task 6 added `CANCELLED`, which
    // is the LAST of the golden's thirteen -- so `fits_now` below is now the
    // whole `runs` population and the two sets are equal.
    let through_w8_task_5: BTreeSet<&str> = held
        .iter()
        .copied()
        .filter(|s| !matches!(*s, "CANCELLED"))
        .collect();

    let mut all: BTreeSet<String> = BTreeSet::new();
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let (
        mut fits_drafted,
        mut fits_task_6,
        mut fits_w3a,
        mut fits_w3c,
        mut fits_w8_task_2,
        mut fits_w8_task_5,
        mut fits_now,
        mut entries,
    ) = (0, 0, 0, 0, 0, 0, 0, 0usize);

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        entries += 1;
        let mut used: BTreeSet<String> = entry.execution.iter().cloned().collect();
        let mut stack: Vec<&Trace> = Vec::new();
        let trace = Trace::from(entry);
        stack.push(&trace);
        while let Some(t) = stack.pop() {
            used.extend(t.execution.iter().cloned());
            for run in &t.runs {
                used.extend(run.state.iter().cloned());
                used.extend(run.attempts.iter().flatten().cloned());
            }
            stack.extend(t.spawned.iter());
        }
        all.extend(used.iter().cloned());
        for state in used.iter().filter(|s| !held.contains(s.as_str())) {
            *missing.entry(state.clone()).or_default() += 1;
        }
        if used.iter().all(|s| drafted.contains(s.as_str())) {
            fits_drafted += 1;
        }
        if used.iter().all(|s| through_task_6.contains(s.as_str())) {
            fits_task_6 += 1;
        }
        if used.iter().all(|s| through_w3a.contains(s.as_str())) {
            fits_w3a += 1;
        }
        if used.iter().all(|s| through_w3c.contains(s.as_str())) {
            fits_w3c += 1;
        }
        if used.iter().all(|s| through_w8_task_2.contains(s.as_str())) {
            fits_w8_task_2 += 1;
        }
        if used.iter().all(|s| through_w8_task_5.contains(s.as_str())) {
            fits_w8_task_5 += 1;
        }
        if used.iter().all(|s| held.contains(s.as_str())) {
            fits_now += 1;
        }
    }

    println!(
        "state vocabulary: the golden uses {} states over {entries} `runs` entries; \
         RunState holds {}. Entries wholly inside the vocabulary: {fits_drafted} \
         (Task 3's draft of 6), {fits_task_6} (through Task 6, 7), {fits_w3a} \
         (Task 7, 8), {fits_w3c} (W3c, 9), {fits_w8_task_2} (W8 Task 2, 11), \
         {fits_w8_task_5} (W8 Task 5, 12), {fits_now} (W8 Task 6, 13). Still \
         missing, by entries using each: {missing:?}",
        all.len(),
        held.len()
    );
    assert_eq!(
        (all.len(), entries),
        (13, 273),
        "the golden's own vocabulary and population; `oracle-exec/README.md`'s \
         state table is derived from these"
    );
    assert_eq!(
        (
            fits_drafted,
            fits_task_6,
            fits_w3a,
            fits_w3c,
            fits_w8_task_2,
            fits_w8_task_5,
            fits_now
        ),
        // Re-measured at WorkingDirectory Task 8 over the 301-entry golden: each
        // stage's count rises by the harvested entries that fit it (the last is
        // the 273 `runs` entries, which is the population's own total).
        (39, 234, 238, 243, 259, 270, 273),
        "what each addition to `RunState` bought. `SUBMITTED` (Task 3) is the \
         big one: 35 -> 229. `RETRIED` (Task 7) buys 4, which is the whole \
         `RETRIED` population -- and the four entries are exactly the \
         `retry-new-execution-*` flows, which cannot bank anyway because the \
         replacement execution is not representable. `SKIPPED` (W3c) buys 4, \
         which is the whole `SKIPPED` population -- and, like `RETRIED`'s four, \
         **none of them banks**: every one of the four is held by a construct \
         W3a does not implement as well (`Subflow`, an `ExecutionUpdatableTask`, \
         `finally:`, an opaque `WorkingDirectory`). Being inside the vocabulary \
         is necessary and not sufficient -- and **W8 Task 2's 16 (237 -> 253) \
         bank nothing at all**, which is the third measured example: when Task \
         2 added `PAUSED` and `RESTARTED` both constructs that emit them were \
         still planned opaque, so nothing in this crate emitted either variant. \
         **Both producers exist now** -- `Pause` at W8 Task 3 (8 banked) and \
         `LoopUntil` at W8 Task 4 (1) -- and the sentence is kept in the past \
         tense it was always about, because the claim it supports is what Task \
         2's SIXTEEN bought on the day, which is still nothing. **W8 Task 5's \
         `WARNING` buys 10 (253 -> 263), which is the whole `WARNING` \
         population, and SEVEN of the twelve entries this package's \
         `allowFailure:` row held bank in that commit** -- the first addition \
         to this enum that banks most of what it buys. The FIVE it does not \
         reach are the ones held by something else: four by the `AllowFailure` \
         FLOWABLE, which is a different construct, and one \
         (`valids/waitfor-child-task-warning`) by an `OutputValues` child no \
         handler exists for -- measured, by unbaselining it and watching it \
         stall at `CREATED RUNNING` with its `WARNING` child already correct. \
         **W8 Task 6's `CANCELLED` buys the last 3 (263 -> 266), which is the \
         whole `CANCELLED` population, and TWO of the three bank in that \
         commit** -- the two `sla:` entries. The third, \
         `valids/exit-cancelled`, is held by an `Exit`, an \
         `ExecutionUpdatableTask` this crate does not implement"
    );
    assert_eq!(
        missing,
        BTreeMap::new(),
        "**W8 Task 6 emptied this map**, and an empty map is exactly the shape \
         a broken walk also produces -- so it is asserted BESIDE two numbers \
         that can disagree: `all.len() == 13` above says the walk found \
         thirteen states, and `fits_now == 266` says every `runs` entry is \
         wholly inside the enum. `RETRIED` left this table at Task 7, \
         `SKIPPED` at W3c, `PAUSED` (18) and `RESTARTED` (12) at W8 Task 2, \
         `WARNING` (10) at W8 Task 5, and `CANCELLED` (3) here"
    );
    // The same fact from the other side, and it is what makes the empty map
    // above falsifiable by something other than itself: the two vocabularies
    // are now the SAME SET, so a variant removed from `RunState` or a state
    // added to the golden fails here by name.
    let held_owned: BTreeSet<String> = held.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        held_owned, all,
        "`RunState` now holds exactly the golden's vocabulary -- neither more \
         nor less. `models::State` still holds four Kestra states neither of \
         these sets has (KILLING, QUEUED, BREAKPOINT, RESUBMITTED); that \
         direction is `the_two_state_vocabularies_agree`'s"
    );
}

/// **The golden's three populations, recursed and un-recursed, both pinned** —
/// W8 Task 2 fix round 1.
///
/// Task 2 published *"0 of the golden's 604 attempt histories"* while claiming
/// the census was *"walked recursively through `spawned`"*. **604 is the
/// un-recursed count; recursed it is 878.** The claim's truth value did not
/// move — `PAUSED` is in 0 attempts either way — but the number was wrong, and
/// it was wrong in the arm the control did not cover: the control reproduced
/// `oracle-exec/README.md`'s **run** table (840/828/639/…), which is the
/// recursed run figure and was right, and said nothing about attempts.
///
/// So this pins **both** numbers for **all three** populations. The failure it
/// exists to catch is not "someone miscounted" but *"someone counted one arm
/// recursively and another arm not, and reported one sentence about both"* —
/// which is only visible when the pair is asserted side by side.
///
/// `RunState`'s `Paused` and `Restarted` docs quote 408/840/878; if any of
/// those moves, this fails and names which.
#[test]
fn the_golden_populations_are_derived_recursed_and_unrecursed() {
    fn count(entries: &[GoldenEntry], recurse: bool) -> (usize, usize, usize, usize, usize) {
        let (mut execs, mut runs, mut atts) = (0usize, 0usize, 0usize);
        let (mut paused_atts, mut restarted_runs) = (0usize, 0usize);
        #[allow(clippy::too_many_arguments)]
        fn sub(
            execution: &[String],
            runs_in: &[GoldenRun],
            spawned: &[GoldenSpawned],
            recurse: bool,
            execs: &mut usize,
            runs: &mut usize,
            atts: &mut usize,
            paused_atts: &mut usize,
            restarted_runs: &mut usize,
        ) {
            if !execution.is_empty() {
                *execs += 1;
            }
            for r in runs_in {
                *runs += 1;
                if r.state.iter().any(|s| s == "RESTARTED") {
                    *restarted_runs += 1;
                }
                for a in &r.attempts {
                    *atts += 1;
                    if a.iter().any(|s| s == "PAUSED") {
                        *paused_atts += 1;
                    }
                }
            }
            if recurse {
                for sp in spawned {
                    sub(
                        &sp.execution,
                        &sp.runs,
                        &sp.spawned,
                        recurse,
                        execs,
                        runs,
                        atts,
                        paused_atts,
                        restarted_runs,
                    );
                }
            }
        }
        for e in entries.iter().filter(|e| e.verdict == Verdict::Runs) {
            sub(
                &e.execution,
                &e.runs,
                &e.spawned,
                recurse,
                &mut execs,
                &mut runs,
                &mut atts,
                &mut paused_atts,
                &mut restarted_runs,
            );
        }
        (execs, runs, atts, paused_atts, restarted_runs)
    }

    let g = golden();
    let flat = count(&g, false);
    let deep = count(&g, true);
    println!(
        "golden populations: un-recursed (execs, runs, attempts) = ({}, {}, {}); \
         recursed = ({}, {}, {})",
        flat.0, flat.1, flat.2, deep.0, deep.1, deep.2
    );
    assert_eq!(
        (flat.0, flat.1, flat.2),
        (273, 588, 624),
        "the UN-recursed populations. 604 is the attempt figure W8 Task 2 \
         published while claiming a recursive walk; it is kept here so the two \
         can never be confused again"
    );
    assert_eq!(
        (deep.0, deep.1, deep.2),
        (415, 858, 898),
        "the RECURSED populations, which are the ones `RunState::Paused` and \
         `RunState::Restarted` quote. 840 is also the figure \
         `oracle-exec/README.md`'s run-history table is over, which is the \
         control that the recursion is the same one that table used"
    );
    // The two claims the numbers above are in service of, so the count and the
    // claim cannot drift apart -- and stated as counts, not booleans, so a
    // broken walk that visited nothing would fail rather than pass.
    assert_eq!(
        (deep.3, deep.4),
        (0, 0),
        "`PAUSED` must appear in no attempt history and `RESTARTED` in no run \
         history, over the RECURSED population. `apply`'s rule 1 would put \
         `Paused` on the open attempt, which is why the first producer needs \
         something other than a plain `SetRunState`"
    );
    assert!(
        deep.2 > flat.2 && deep.1 > flat.1 && deep.0 > flat.0,
        "sanity: recursing must find MORE of all three, or the walk is not \
         recursing and both assertions above would be measuring the same thing"
    );
}

/// **`SKIPPED` comes from `runIf:`, and never from `disabled:`** — the
/// attribution derived rather than written down.
///
/// `oracle-exec/README.md` said `disabled: true` produced the golden's
/// `SKIPPED` runs in three separate places, and the plan's *States and
/// constructs no W3a task claims* table said it too. It is false, and it was
/// false in a way that mattered: it gave a state to a construct that does not
/// produce it, and it left the construct that DOES produce it — `runIf:` —
/// with no row and no owner anywhere in the package.
///
/// Both halves are asserted, because only the second one is a control:
///
/// - every run whose state history holds `SKIPPED` is named here, and its
///   flow's YAML must declare `runIf:` and must NOT declare `disabled:`;
/// - and no flow that DOES declare `disabled:` may produce a `SKIPPED`
///   anywhere in its trace. Without that half the test would pass on a golden
///   where `disabled:` skipped things too.
///
/// This is what `executor/plan.rs`'s `flatten_list` stands on: a disabled task
/// gets **no run at all**, not a run in a state `RunState` cannot hold. Had the
/// README's sentence been true, deleting the task from the plan would have been
/// the wrong shape of fix.
#[test]
fn the_skipped_state_comes_from_runif_and_never_from_disabled() {
    let mut skipped_runs: Vec<(String, String)> = Vec::new();
    let mut disabled_entries: Vec<String> = Vec::new();

    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        let mut stack: Vec<&Trace> = vec![&trace];
        let mut saw_skipped = false;
        while let Some(t) = stack.pop() {
            for run in &t.runs {
                if run.state.iter().any(|s| s == "SKIPPED")
                    || run.attempts.iter().flatten().any(|s| s == "SKIPPED")
                {
                    saw_skipped = true;
                    skipped_runs.push((entry.id.clone(), run.task.clone()));
                }
            }
            stack.extend(t.spawned.iter());
        }
        if corpus_source(&entry.id).contains("disabled:") {
            disabled_entries.push(entry.id.clone());
            assert!(
                !saw_skipped,
                "{}: a flow declaring `disabled:` produced a SKIPPED run. That \
                 is the reading `oracle-exec/README.md` used to carry, and if it \
                 ever becomes true then `flatten_list` deleting the task from \
                 the plan is the wrong shape of fix",
                entry.id
            );
        }
    }

    skipped_runs.sort();
    assert_eq!(
        skipped_runs,
        [
            ("valids/task-runif", "notexecuted"),
            ("valids/task-runif", "notexecutedflowable"),
            ("valids/task-runif-executionupdating", "skipSetVariables"),
            ("valids/task-runif-executionupdating", "skipUnsetVariables"),
            (
                "valids/task-runif-taskswithstate",
                "skipped_when_failures_exist"
            ),
            ("valids/task-runif-workingdirectory", "log_test"),
            // WorkingDirectory Task 8: the `runIf: {{ false }}` child of the
            // re-harvested `working-directory-real-runif-false`.
            ("valids/working-directory-real-runif-false", "skipped"),
        ]
        .map(|(e, t)| (e.to_string(), t.to_string())),
        "every SKIPPED run in the golden, by name: 7 runs across 5 entries (6 across 4 \
         before WorkingDirectory Task 8 added `working-directory-real-runif-false`). \
         `the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` \
         counts the ENTRIES (4); this names the runs, so a state moving between \
         them is visible too"
    );

    let owners: BTreeSet<String> = skipped_runs.iter().map(|(e, _)| e.clone()).collect();
    for id in &owners {
        let source = corpus_source(id);
        assert!(
            source.contains("runIf:"),
            "{id}: produces a SKIPPED run and does not declare `runIf:` -- the \
             attribution has moved and the README's row must move with it"
        );
        assert!(
            !source.contains("disabled:"),
            "{id}: produces a SKIPPED run AND declares `disabled:`, so this \
             corpus can no longer tell the two constructs apart and the \
             attribution must be re-derived by hand"
        );
    }

    assert_eq!(
        disabled_entries,
        [
            "valids/disable-error",
            "valids/disable-flowable",
            "valids/disable-simple",
            "valids/if-with-only-disabled-tasks",
            "valids/parallel-disabled-tasks",
            "valids/sequential-with-disabled",
        ],
        "the control's own population, named so it cannot silently become \
         empty: six comparable golden entries declare `disabled:`, and not one \
         of them produces a SKIPPED anywhere. `valids/working-directory` is a \
         seventh corpus flow with the flag and has no golden entry -- it is one \
         of the 22 `task-type-not-in-image` rejections"
    );
}

/// **`CANCELLED` never comes from a flow `concurrency:` limit** — the fourth
/// run of the rule `the_skipped_state_comes_from_runif_and_never_from_disabled`
/// established, and the third time the trigger was a state credited to the
/// wrong construct.
///
/// `oracle-exec/README.md` labelled the residual baseline's `flow
/// concurrency:` row `→ CANCELLED` in two places, and the programme spec built
/// a W8 package on it. It is false in exactly the shape `disabled:`/`SKIPPED`
/// was: the state really is in the golden, three times, and **not one of the
/// three is a concurrency limit's**. A row is then reachable-looking for a
/// reason that has nothing to do with the construct it names.
///
/// Three halves, because only the third is a control:
///
/// - every entry whose trace holds `CANCELLED` is named here, with what
///   produces it — an `sla:` whose `behavior: CANCEL`, or an
///   `io.kestra.plugin.core.execution.Exit` declaring `state: CANCELLED`;
/// - **no** such entry is produced by a `concurrency:` limit, asserted as the
///   positive form: each of the three declares a CANCEL-behaviour producer that
///   is not one;
/// - and the control that makes the zero a measurement rather than an absence:
///   **`behavior: CANCEL` is the only `concurrency:` configuration that could
///   ever cancel, two corpus flows write it, and neither produces a `CANCELLED`
///   anywhere in its trace.** Without this half the test would pass on a corpus
///   that simply contained no cancelling limit, which is a different fact and a
///   much weaker one.
///
/// # `QUEUED` never occurs — and what that does and does not cover
///
/// **`QUEUED` occurs zero times in `golden.json`.** It is in `harvest.py`'s
/// `STATE_TYPES` — the vocabulary the harvester validates every captured state
/// against — and `normalise.py`'s rule 4 keeps history state sequences
/// **verbatim**, so a queued run would have survived into the golden had one
/// ever happened. Zero is therefore not "we could not see it"; it is *it did
/// not happen*.
///
/// **But it can only see `QUEUE` behaviour, and fix round 1 over-claimed from
/// it.** That version said the zero proved "no concurrency limit ever fired at
/// all". The conclusion is true; the stated evidence did not reach it. The 13
/// declarers are **10 `QUEUE`, 2 `CANCEL`, 1 `FAIL`**, and it takes one
/// instrument per behaviour:
///
/// - the **10 `QUEUE`** are the `QUEUED` zero above;
/// - the **2 `CANCEL`** are the `CANCELLED` control below — `flow-concurrency-cancel`
///   and `…-cancel-pause` are the only flows writing `behavior: CANCEL` and
///   neither golden holds a `CANCELLED`;
/// - the **1 `FAIL`** (`valids/flow-concurrency-fail`) is covered by neither,
///   and **a count of `FAILED` could not cover it** — the golden holds 609 of
///   them for other reasons. What covers it is that this flow is one of the
///   **7 declarers that already MATCH**, asserted below: egret implements no
///   concurrency at all, so an entry it reproduces exactly is an entry whose
///   limit did not fire.
///
/// The three legs are kept as the **mechanism** — they say *why* no limit ever
/// fired, which no count can: the harness drives flows serially, the single
/// self-contending flow is banked, and 7 of 13 declarers already match.
///
/// **What this evidence does not contain**, named rather than generalised over:
/// it says nothing about whether Kestra's `concurrency: {behavior: CANCEL}`
/// cancels *in general* — it cannot, because `harvest.py` drives flows strictly
/// serially (`for i, rec in enumerate(wanted, 1): execute_once(...)`, each
/// awaited to termination) and so never contends a `limit: 1`. The claim is the
/// narrow one the ratchet needs: **under this oracle** the state is never a
/// limit's, so a row attributing it to one strands its entries.
///
/// The one contended population the corpus does hold —
/// `valids/flow-concurrency-parallel-subflow-kill`, whose `Parallel` starts two
/// `Subflow`s of the same limited flow at once — is asserted below to be
/// **banked**, and its golden shows why a `QUEUE` that really did fire would
/// still be invisible: all three of its `order` pairs are `overlapping` and its
/// `spawned` is empty, so *when* the second child execution starts is in no
/// compared field.
#[test]
fn the_cancelled_state_never_comes_from_a_concurrency_limit() {
    // **The primary evidence, first: `QUEUED` never occurs.** Every state string
    // anywhere in the golden -- execution histories, run histories, attempt
    // histories, sub-executions -- counted, so this cannot pass by looking in
    // the wrong place. `QUEUE` is a limit's DEFAULT behaviour and `QUEUED` is
    // the only state it produces; `harvest.py`'s `STATE_TYPES` carries the name
    // and `normalise.py` keeps histories verbatim, so one would have survived.
    let mut states: BTreeMap<String, usize> = BTreeMap::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        let mut stack: Vec<&Trace> = vec![&trace];
        while let Some(t) = stack.pop() {
            for st in t
                .execution
                .iter()
                .chain(t.runs.iter().flat_map(|r| r.state.iter()))
                .chain(t.runs.iter().flat_map(|r| r.attempts.iter().flatten()))
            {
                *states.entry(st.clone()).or_default() += 1;
            }
            stack.extend(t.spawned.iter());
        }
    }
    assert_eq!(
        states.get("QUEUED"),
        None,
        "a `QUEUED` appeared in the golden, so a concurrency limit DID fire \
         under this oracle and the whole zero has to be re-derived. States \
         seen: {states:?}"
    );
    // The two-sided half: the walk above must really be finding states, or the
    // assertion is over an empty map and says nothing.
    assert!(
        states.get("RUNNING").copied().unwrap_or(0) > 1000 && states.contains_key("CANCELLED"),
        "the state walk found nothing to speak of, so the `QUEUED` assertion \
         above was vacuous: {states:?}"
    );

    let mut cancelled: Vec<String> = Vec::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let trace = Trace::from(entry);
        let mut stack: Vec<&Trace> = vec![&trace];
        let mut saw = false;
        while let Some(t) = stack.pop() {
            if t.execution.iter().any(|s| s == "CANCELLED") {
                saw = true;
            }
            for run in &t.runs {
                if run.state.iter().any(|s| s == "CANCELLED")
                    || run.attempts.iter().flatten().any(|s| s == "CANCELLED")
                {
                    saw = true;
                }
            }
            stack.extend(t.spawned.iter());
        }
        if saw {
            cancelled.push(entry.id.clone());
        }
    }

    cancelled.sort();
    assert_eq!(
        cancelled,
        [
            "valids/exit-cancelled",
            "valids/flow-concurrency-sla-assert-cancel",
            "valids/sla-execution-condition",
        ],
        "every golden entry whose trace holds CANCELLED, by name. \
         `the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` \
         counts them (3); this says WHICH, which is the whole question"
    );

    // The positive half: each of the three declares a CANCEL-behaviour producer
    // that is not a concurrency limit.
    for id in &cancelled {
        let source = corpus_source(id);
        let sla_cancel = source.contains("\nsla:") && source.contains("behavior: CANCEL");
        let exit_cancel = source.contains("io.kestra.plugin.core.execution.Exit")
            && source.contains("state: CANCELLED");
        assert!(
            sla_cancel || exit_cancel,
            "{id}: holds a CANCELLED and declares neither an `sla:` with \
             `behavior: CANCEL` nor an `Exit` with `state: CANCELLED`. The \
             attribution has moved and every table naming an owner for this \
             state must move with it"
        );
    }
    assert_eq!(
        cancelled
            .iter()
            .filter(|id| corpus_source(id).contains("\nsla:"))
            .count(),
        2,
        "two of the three are an `sla:`'s; the third is `valids/exit-cancelled`, \
         an `ExecutionUpdatableTask`. Counted so that a corpus in which all \
         three became one construct's could not pass the loop above unnoticed"
    );

    // The control, and the reason the zero is a measurement: `behavior: CANCEL`
    // is the only `concurrency:` setting that can cancel anything. Both flows
    // that write it are named, so this can never silently become an empty loop.
    //
    // **The population here is DECLARERS, and the class the control argues
    // about is "executions a CANCEL limit could have cancelled".** Those are
    // not the same set, and two corpus entries sit outside the scan below --
    // **for two DIFFERENT reasons, which the wording this comment carried
    // until W8 Task 1 fix round 4 merged into one by saying both "sit in the
    // gap":**
    //
    // * `valids/flow-concurrency-subflow` runs a `Subflow` of
    //   `valids/flow-concurrency-cancel`, so a SECOND execution runs under
    //   that CANCEL limit while declaring none itself (golden
    //   `CREATED RUNNING SUCCESS`, not baselined). It is in the gap of the
    //   class named above, exactly.
    // * `valids/trigger-flow-listener-with-concurrency-limit` listens on
    //   `trigger-flow-with-concurrency-limit`'s `QUEUED`/`RUNNING`/`SUCCESS`.
    //   That flow declares **`behavior: QUEUE`**, not `CANCEL`, so the
    //   listener is **not** in the gap of "executions a CANCEL limit could
    //   have cancelled" -- no CANCEL limit is anywhere near it. It is in the
    //   gap of a wider class, "executions related to a limit", and it earns
    //   its place in this comment as the `QUEUED` witness, which is what the
    //   sentence below already says it is.
    //
    // The wording came verbatim from the review that raised this, so it is a
    // conflation adopted rather than one invented -- and adopting a sentence
    // without re-deriving it is the error this task has recorded six times.
    //
    // Neither entry would be seen by the scan below. **The conclusion survives
    // because it does not rest on this scan**: the first assertion above pins
    // the whole `CANCELLED` set by name, over every entry, so a cancel landing
    // on a non-declarer would have been caught there. The two are named here so
    // the gap is a known one rather than an unexamined one -- and the listener
    // is a second witness for the `QUEUED` zero, since the corpus contains a
    // trigger that watches for `QUEUED` and never saw one.
    let mut declarers: Vec<String> = Vec::new();
    let mut cancel_limits: Vec<String> = Vec::new();
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let source = corpus_source(&entry.id);
        if !source.contains("\nconcurrency:") {
            continue;
        }
        declarers.push(entry.id.clone());
        if source.contains("behavior: CANCEL") && !source.contains("\nsla:") {
            cancel_limits.push(entry.id.clone());
            assert!(
                !cancelled.contains(&entry.id),
                "{}: declares `concurrency: {{behavior: CANCEL}}` AND holds a \
                 CANCELLED. This corpus can no longer tell a limit's cancel \
                 from an `sla:`'s, and the attribution must be re-derived by \
                 hand",
                entry.id
            );
        }
    }
    declarers.sort();
    cancel_limits.sort();
    assert_eq!(
        cancel_limits,
        [
            "valids/flow-concurrency-cancel",
            "valids/flow-concurrency-cancel-pause"
        ],
        "the control's own population, named so it cannot silently become \
         empty: two comparable golden entries declare a limit whose behaviour \
         is CANCEL, and neither cancels. `valids/flow-concurrency-cancel`'s \
         whole execution history is `CREATED RUNNING SUCCESS`"
    );

    // The third leg: **10** of the 13 declarers are not baselined at all, which
    // by the ratchet's other direction means they MATCH with nothing
    // implemented. It was 7 through W8 Task 2; two moved at W8 Task 3
    // (`valids/flow-concurrency-cancel-pause` and `-queue-pause`, flows whose
    // entire task list is the `Pause`, so their `concurrency:` has nothing to
    // contend with) and the tenth is W8 Task 6's.
    //
    // **The tenth is the strongest row this leg has ever had**, and it is worth
    // naming rather than counting: `valids/flow-concurrency-sla-assert-cancel`
    // declares `concurrency: {behavior: QUEUE, limit: 1}` AND ends `CANCELLED`,
    // which is the exact shape a reader would attribute to the limit. It banks
    // in the commit that implements `sla:` and NOTHING about `concurrency:` --
    // so the entry's `CANCELLED` is now reproduced, by name, from the other
    // construct. That is a positive measurement where the rest of this test is
    // an absence.
    let baseline = baseline();
    let matching: Vec<&String> = declarers
        .iter()
        .filter(|id| !baseline.contains(*id))
        .collect();
    assert_eq!(
        (declarers.len(), matching.len()),
        (13, 10),
        "13 comparable entries declare a flow `concurrency:` and 10 of them \
         already match, which is what makes `reaches 0` a control rather than \
         an assertion about an unimplemented construct: {matching:?}"
    );
    assert!(
        matching
            .iter()
            .any(|id| { id.as_str() == "valids/flow-concurrency-sla-assert-cancel" }),
        "the tenth is the one that carries a CANCELLED, and it is the reason \
         this leg is a measurement rather than an absence -- see the comment \
         above. If it leaves this set, the argument that the golden's CANCELLED \
         is the `sla:`'s has lost its positive half"
    );
    // **The contending flow is not itself a declarer, and asserting that it was
    // is how this guard first failed.** `valids/flow-concurrency-parallel-
    // subflow-kill` writes no `concurrency:` at all: its `Parallel` starts two
    // `Subflow`s of `-child`, and it is `-child` that carries the limit. The
    // pair is asserted as a pair for that reason -- a scan of declarers alone
    // cannot see the only contention this corpus creates.
    assert!(
        matching.contains(&&"valids/flow-concurrency-parallel-subflow-kill-child".to_string()),
        "the corpus's ONLY limited-and-contended flow, banked by W6 Task 3"
    );
    let contender = "valids/flow-concurrency-parallel-subflow-kill";
    assert!(
        !corpus_source(contender).contains("\nconcurrency:")
            && corpus_source(contender).contains("io.kestra.plugin.core.flow.Parallel")
            && !baseline.contains(contender),
        "the flow that CREATES the contention -- two `Subflow`s of `-child` \
         from one `Parallel`, so `limit: 1` really does fire -- declares no \
         limit of its own and is banked. It matches anyway, because what QUEUE \
         changes (when the second child execution starts) is in no compared \
         field: its three `order` pairs are all `overlapping` and its \
         `spawned` is empty"
    );

    println!(
        "CANCELLED: {cancelled:?}; concurrency declarers: {}, already matching: {}",
        declarers.len(),
        matching.len()
    );
}

// --------------------------------------------------------------------------
// Task 8: the refusals
// --------------------------------------------------------------------------

/// A group of `refused` golden entries that share one cause, and the fragment
/// of egret's own message that says egret refused them for *that* cause.
struct RefusalGroup {
    /// What makes every flow in this group invalid — the implementation unit.
    cause: &'static str,
    /// A substring of `PlanError::message`. Asserting on it is what makes a
    /// refusal that becomes a *different* refusal visible: `expect_err` alone
    /// degrades into "something went wrong", and every one of these flows is
    /// wrong in more than one way, so a rule that fires for the wrong reason
    /// still returns `Err`.
    message: &'static str,
    ids: &'static [&'static str],
}

/// The 23 refusals, by cause. Not by directory: `invalids/loop-switch-failed`
/// is not in the golden's `refused` set at all (Kestra's own `RunContextTest`
/// executes it and asserts `FAILED`), and the 23 rejections classed
/// `task-type-not-in-image` or `checkout-drift` have no golden entry — those
/// are the pinned image or the sibling checkout differing from each other, and
/// refusing them would be a defect manufactured out of a naming convention.
const REFUSAL_GROUPS: &[RefusalGroup] = &[
    RefusalGroup {
        cause: "the YAML is not a flow — a required field is absent or has a value \
                no variant of its type accepts",
        message: "cannot parse flow at line",
        ids: &[
            // Kestra: `Invalid type: null` — the input declares no `type`.
            "invalids/inputs",
            // Kestra: `Invalid type: FOO`.
            "invalids/inputs-bad-type",
            // Kestra: `tasks[null].id: must not be blank`.
            "invalids/invalid",
            // Kestra: `tasks[null].id: must not be null`, and separately the
            // system labels. egret refuses the missing id, which is the same
            // half of the same flow.
            "invalids/system-labels",
        ],
    },
    RefusalGroup {
        cause: "the same key twice in one mapping — YAML would silently keep the last",
        message: "duplicate field 'message'",
        ids: &["invalids/duplicate-key"],
    },
    // These two were ONE group until the null-list fix, and the group was not a
    // cause at all: both flows carry an explicit `null` where W1's model had a
    // bare `Vec<Task>`, so both were refused at PARSE — right by accident, while
    // the same rule over-refused `valids/switch-impossible`, a flow this Kestra
    // published and ran. `wire::null_is_absent` removed that rule, and each flow
    // is now refused for the cause Kestra's own 422 names. Their `reason` fields
    // in `corpus.json` are what the two `message`s below were chosen against.
    //
    // Kestra's 422 for `invalid-parallel` is also the direct evidence for what
    // the fix does: it lists `tasks[para4].tasks: The 'tasks' property cannot be
    // empty` beside para2's and para3's. Kestra READ `tasks: null` — Jackson did
    // not error on it — and then bean validation refused it for being EMPTY,
    // which is exactly the two-stage behaviour egret now reproduces.
    RefusalGroup {
        cause: "a `Switch` with neither `cases:` nor `defaults:` — it runs nothing \
                under any match. Kestra: `tasks[impossible]: No task defined, \
                neither cases or default have any tasks`",
        message: "switch_no_branches:",
        ids: &["invalids/switch-invalid"],
    },
    RefusalGroup {
        cause: "a flow whose id and namespace are both the empty string, carrying \
                three `Parallel`s with no children. Kestra names all of it; egret \
                reports the namespace first and counts the rest",
        message: "invalid_namespace:",
        ids: &["invalids/invalid-parallel"],
    },
    RefusalGroup {
        cause: "a flowable with nothing to decide on — it cannot be planned at all",
        message: "has no condition to branch on",
        ids: &["invalids/if-without-condition"],
    },
    RefusalGroup {
        cause: "a `Dag` whose `dependsOn` graph cannot be ordered",
        message: "has a cyclic dependency through task id(s)",
        ids: &["invalids/dag-cyclicdependency"],
    },
    RefusalGroup {
        cause: "a `Dag` node depending on a task that is not a node of it",
        message: "depends on task id(s) that are not nodes of it",
        ids: &["invalids/dag-notexist-task"],
    },
    RefusalGroup {
        cause: "an id declared twice, so a reference to it names two things",
        message: "duplicate_task_id:",
        ids: &["invalids/duplicate", "invalids/duplicate-parallel"],
    },
    RefusalGroup {
        cause: "an input id declared twice",
        message: "duplicate_input_id:",
        ids: &["invalids/duplicate-inputs"],
    },
    RefusalGroup {
        cause: "a flow with no tasks — there is nothing to execute",
        message: "empty_tasks:",
        ids: &["invalids/empty", "valids/invalid-draft-flow"],
    },
    RefusalGroup {
        cause: "a flow that calls itself as a subflow",
        message: "self_subflow:",
        ids: &["invalids/recursive-flow"],
    },
    // no-silent-stall Task 1. This was `STILL_OVER_ACCEPTED`'s first row
    // ("`unknown_task_type` is a WARNING, and promoting it to an error would
    // refuse every plugin egret has not registered"). The product decision
    // that row waited for was taken (spec §3): a type no handler serves
    // stalled every run that reached it, so it is refused, and Kestra's reason
    // and egret's are now the same one.
    RefusalGroup {
        cause: "a task type that does not exist (`Invalid type: ...MissingTwo`)",
        message: "unknown_task_type:",
        ids: &["invalids/invalid-task"],
    },
    RefusalGroup {
        cause: "a `WorkingDirectory` with no children",
        message: "working_directory_tasks_required:",
        ids: &["invalids/workingdirectory-no-tasks"],
    },
    RefusalGroup {
        cause: "a `WorkingDirectory` whose child is not a runnable task",
        message: "working_directory_child_not_runnable:",
        ids: &["invalids/workingdirectory-invalid"],
    },
    RefusalGroup {
        cause: "a `Schedule` trigger that cannot supply an input with no default",
        message: "schedule_missing_inputs:",
        ids: &[
            "invalids/scheduled-missing-inputs-defaults",
            "invalids/scheduled-missing-inputs-defaults-2",
        ],
    },
    RefusalGroup {
        cause: "an `itemType` an ARRAY or MULTISELECT input cannot hold",
        message: "input_item_type_invalid:",
        ids: &["invalids/inputs-validation"],
    },
    RefusalGroup {
        cause: "a dotted reference to an id containing a dash — Pebble reads the \
                dash as subtraction",
        message: "dashed_input_reference:",
        ids: &["invalids/inputs-key-with-subtraction-symbol-validation"],
    },
    RefusalGroup {
        cause: "the same, on an `outputs.` reference",
        message: "dashed_output_reference:",
        ids: &["invalids/outputs-key-with-subtraction-symbol-validation"],
    },
];

/// The refusals egret does NOT make, each with the reason and the owner.
///
/// They are asserted to be **accepted**, not merely absent from the table
/// above: an over-acceptance that quietly becomes a refusal must fail this
/// test and force the record to be corrected, exactly as the ratchet's
/// `banked` arm does for the other direction.
const STILL_OVER_ACCEPTED: &[(&str, &str)] = &[
    (
        "invalids/invalid-property",
        "`Unrecognized field \"invalid\" (class ...debug.Return)` — needs the plugin's own \
         property schema. W1 models a task's plugin properties as an opaque `Leaf { spec }` \
         on purpose. Owner: the same plugin-schema gap.",
    ),
    (
        "invalids/dag-invalid-subtask",
        "`tasks[task1].tasks[task1].task.message: must not be null` — a `@NotNull` on \
         `Log.message`, i.e. bean validation of a plugin's own property. Same gap again.",
    ),
    (
        "invalids/inputs-bad-validator-syntax",
        "`inputs[badValidatorSyntax].validator: invalid pattern [\\]` — Kestra compiles it \
         with `java.util.regex.Pattern`. Compiling it with the `regex` crate instead would \
         refuse valid Java patterns (lookaround, backreferences), which is an OVER-refusal \
         traded for an over-acceptance. Owner: W1, and it needs a Java-Pattern-compatible \
         syntax check, not `Regex::new`.",
    ),
    (
        "invalids/inputs-with-multiple-constraint-violations",
        "publish returned **200**; Kestra refuses it when the execution's input VALUES are \
         read (`value `H` doesn't match the values `[A, B, C]`). `build_plan(&str)` takes no \
         input values, so no change to it can reach this seam. Owner: W3b/W1 input coercion.",
    ),
];

/// egret refuses every flow this Kestra refuses, or the exception is named.
///
/// Over-acceptance — egret answering where Kestra refuses — is the divergence
/// class the programme forbids shipping, so this is the test the whole
/// `invalids` corpus exists for. It asserts three things the ratchet alone
/// does not:
///
/// 1. the refusal is a **refusal**, not a panic and not a trace;
/// 2. it is egret's own message **for that cause**, so a refusal that becomes
///    a different refusal is visible rather than silently still passing;
/// 3. the partition is **exhaustive** — every golden `refused` id is in
///    exactly one of the two tables, and the totals are asserted beside the
///    parts.
#[test]
fn egret_refuses_every_flow_kestra_refuses() {
    let refused: Vec<GoldenEntry> = golden()
        .into_iter()
        .filter(|e| e.verdict == Verdict::Refused)
        .collect();

    // Every `refused` entry is one of the two classes that are evidence that
    // egret must refuse. The other two classes have no golden entry at all.
    for e in &refused {
        let class = e.rejected_class.as_deref().unwrap_or("<none>");
        assert!(
            matches!(class, "invalid-flow" | "refused-by-both"),
            "{}: a `refused` golden entry must be classed by CAUSE, not by directory; \
             got {class}",
            e.id
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut refused_in_groups = 0usize;

    for group in REFUSAL_GROUPS {
        for id in group.ids {
            let source = corpus_source(id);
            refused_in_groups += 1;
            match build_plan(&source) {
                Ok(plan) => failures.push(format!(
                    "  {id}: OVER-ACCEPTED. Kestra refused it; egret planned {} task(s).\n\
                           cause: {}",
                    plan.tasks.len(),
                    group.cause
                )),
                Err(e) if !e.message.contains(group.message) => failures.push(format!(
                    "  {id}: refused for a DIFFERENT reason than the group it is in.\n\
                           cause:    {}\n      expected: {:?}\n      actual:   {:?}",
                    group.cause, group.message, e.message
                )),
                Err(_) => {}
            }
        }
    }

    for (id, reason) in STILL_OVER_ACCEPTED {
        let source = corpus_source(id);
        if let Err(e) = build_plan(&source) {
            failures.push(format!(
                "  {id}: now REFUSED ({:?}), but it is recorded as still over-accepted.\n\
                       Delete its row from STILL_OVER_ACCEPTED, give it a group, and \
                 delete its `baseline.txt` line.\n      recorded reason: {reason}",
                e.message
            ));
        }
    }

    // The partition, asserted rather than assumed.
    let grouped: BTreeSet<&str> = REFUSAL_GROUPS
        .iter()
        .flat_map(|g| g.ids.iter().copied())
        .collect();
    let excepted: BTreeSet<&str> = STILL_OVER_ACCEPTED.iter().map(|(id, _)| *id).collect();
    let golden_refused: BTreeSet<&str> = refused.iter().map(|e| e.id.as_str()).collect();
    let mut both = grouped.intersection(&excepted).peekable();
    assert!(
        both.peek().is_none(),
        "an id may be in exactly one of the two tables: {:?}",
        both.collect::<Vec<_>>()
    );
    let covered: BTreeSet<&str> = grouped.union(&excepted).copied().collect();
    assert_eq!(
        covered, golden_refused,
        "every golden `refused` id must be in exactly one table, and nothing else may be"
    );

    // The total, beside the parts.
    assert_eq!(
        refused_in_groups + STILL_OVER_ACCEPTED.len(),
        refused.len(),
        "{} refused in {} groups + {} named exceptions must equal the {} `refused` \
         golden entries",
        refused_in_groups,
        REFUSAL_GROUPS.len(),
        STILL_OVER_ACCEPTED.len(),
        refused.len()
    );

    println!(
        "refusals: {} golden `refused` entries = {} refused across {} causes + {} named \
         over-acceptances",
        refused.len(),
        refused_in_groups,
        REFUSAL_GROUPS.len(),
        STILL_OVER_ACCEPTED.len()
    );

    assert!(
        failures.is_empty(),
        "OVER-ACCEPTANCE is the divergence class this programme forbids shipping:\n{}",
        failures.join("\n")
    );
}

/// The residual baseline, attributed to a construct — derived, not written
/// down beside a derivation.
///
/// This is the coverage statement's numerator table. 149 entries is a number;
/// "22 of them use a `Subflow`, which no W3a task touches" is an account. The
/// attribution is textual and deliberately so: it asks what each baselined
/// flow's own YAML *declares*, not what egret's model made of it, because a
/// construct egret cannot parse would score zero by construction — that is the
/// seventh guard-that-could-not-fail this package shipped, and it was hiding a
/// `Loop`'s `errors:`.
///
/// **The rows overlap and this is not a partition.** A flow may declare a
/// `Subflow` inside a `finally:` and is counted by both. What IS asserted to
/// be exhaustive is the remainder: the entries declaring none of these
/// constructs are listed by name, so a new baselined entry that belongs to no
/// owner cannot arrive silently.
///
/// Two of the rows are constructs the *States and constructs no W3a task
/// claims* table did not carry, and checking the remainder rather than
/// trusting the table is what found them — the same way Task 3 found `Subflow`
/// and Task 6 found `LoopUntil`:
///
/// - **`afterExecution:` (10 entries).** W1's `Flow::after_execution` parses
///   and `spec::validate` walks it; `build_plan` flattens `tasks` and `errors`
///   and nothing else, so those tasks are in no plan. It is `finally:`'s
///   sibling and is unclaimed for the same reason.
/// - **`sla:` (5 entries).** W1 ports it as a data shape and says so; nothing
///   evaluates one. The programme spec gives it to W8.
///
/// **A third arrived at the final review, and it arrived because an
/// attribution written in prose was wrong.** `oracle-exec/README.md` credited
/// the golden's `SKIPPED` state to `disabled: true` in three places. Walking
/// every run of `golden.json` finds `SKIPPED` on **6 runs across 4 entries**
/// — `valids/task-runif`, `…-executionupdating`, `…-taskswithstate`,
/// `…-workingdirectory` — and **not one of those four YAML files contains
/// `disabled:` at all**. Every one is `runIf:`.
/// `the_skipped_state_comes_from_runif_and_never_from_disabled` derives that
/// on every run so the sentence cannot drift back.
///
/// - **`runIf:` (7 entries).** Unclaimed, and unclaimed by *two* packages at
///   once: W1's `Task` does not carry the field (it is neither in the struct
///   nor in `children()`, so it survives only inside a leaf's opaque `spec`),
///   and `RunState` has no `SKIPPED` for the executor to put a skipped run
///   into. Either half alone would block it. Its entries were absorbed by
///   other rows until now — `valids/task-runif` was counted under `Subflow`,
///   which it happens to declare, so fixing `Subflow` would not have banked
///   it.
///
/// **`disabled: true` left this table at the final review**, which is the
/// other direction the same rule runs in: it is implemented, so it holds
/// nothing. Of its six entries four banked; `valids/disable-flowable` is now
/// attributed to the `Loop` that really holds it, and
/// `valids/parallel-disabled-tasks` is in the named remainder below, held by
/// one `order` pair — the same thing that holds `valids/dag` and `valids/if`.
#[test]
fn the_residual_baseline_is_attributed_to_a_construct_or_named() {
    // (label, the strings a flow's own YAML writes for it)
    const CONSTRUCTS: &[(&str, usize, &[&str])] = &[
        // **37 through W3b Task 4, 21 at W3b Task 5, 20 now.** The label moved
        // with the number: W3b Task 5 re-points the oracle at the real driver,
        // `worker::spawn` executes `Command::SpawnExecution`, and 16 of those
        // 37 bank. What is left is a `Loop` whose SUB-EXECUTION still diverges,
        // which is a different sentence from the one this row used to make.
        // The whole-branch review's fix round banks a seventeenth,
        // `valids/replay-loop`, by giving a sub-execution its parent's
        // `inputs`.
        (
            "Loop — a sub-execution egret runs and gets wrong",
            // 19 through W9a Task 10; `valids/loop-values-from-uri` banked at
            // W9a Task 11 (a `Loop` over an ION file); 17 since WorkingDirectory
            // Task 8 banked `valids/working-directory-taskrun` (a `Loop` whose
            // sub-execution egret now gets right).
            17,
            &["io.kestra.plugin.core.flow.Loop\n", "core.loop\n"],
        ),
        // **`Subflow` was a row here with 22 through W5, and W6 Task 3 removed
        // it rather than lowering it to 10.** Same rule as `disabled: true` and
        // `runIf:` below, run for the third time: a row attributes an entry to
        // the construct that HOLDS it, and `Subflow` holds nothing now — it
        // executes, and 12 of the 22 banked in the commit that removed this row.
        //
        // The remaining 10 are all held by something else, and lowering the
        // number instead would have kept six of them attributed to a construct
        // that works. **Three** are absorbed by rows that stayed (`Loop` once,
        // `AllowFailure` twice) and the other **seven** are named one by one in
        // `UNATTRIBUTED` below, which is where the honest attribution for each
        // of them lives. This sentence said "four" and "six" and named `flow
        // concurrency:` as the fourth absorber; W8 Task 1 deleted that row for
        // being an attribution to a construct that holds nothing, and its one
        // entry -- `valids/flow-concurrency-sla-fail-parent` -- became the
        // seventh name below. The fourth absorber was itself the misattribution.
        // 16 until the AllowFailure flowable: `valids/allow-failure-with-retry` and
        // `valids/finally-allowfailure` carry a `finally:` that egret runs now.
        ("finally:", 14, &["finally:"]),
        (
            "ExecutionUpdatableTask",
            16,
            &[
                "io.kestra.plugin.core.execution.Labels",
                "io.kestra.plugin.core.execution.SetVariables",
                "io.kestra.plugin.core.execution.UnsetVariables",
                "io.kestra.plugin.core.execution.Exit",
            ],
        ),
        (
            "AllowFailure / allowFailure:",
            // Two now: the three targets are banked (no `AllowFailure` construct left
            // on them), and `valids/retry-with-flowable-errors`, `pause-timeout-allow-failure`
            // remain.
            2,
            &[
                "io.kestra.plugin.core.flow.AllowFailure",
                "core.allowfailure",
                "allowFailure: true",
            ],
        ),
        // **12 through W8 Task 2, 4 through W8 Task 4, 3 now.** W8 Task 3
        // banked eight and the three that remain are held by three DIFFERENT
        // properties of the construct rather than by the wait:
        // `pause-behavior` by `behavior:`, `pause_on_pause` by `onPause:`, and
        // `pause-errors-finally-after-execution` by `errors:`/`finally:`/
        // `afterExecution:`. The fourth was `pause-timeout-allow-failure`, held
        // by `allowFailure:` turning FAILED into a `WARNING` `RunState` cannot
        // hold -- which was the `AllowFailure` row's cause and not this one's,
        // and was counted here only because this table attributes by
        // declaration. **W8 Task 5 supplied the variant and banked it**, so the
        // row this one over-claimed is gone rather than argued about.
        // `oracle-exec/baseline.txt` names all three with what holds each.
        (
            "Pause",
            3,
            &["io.kestra.plugin.core.flow.Pause", "core.pause"],
        ),
        ("afterExecution:", 10, &["afterExecution:"]),
        // **9 through W8 Task 3; 8 since W8 Task 4 banked
        // `valids/waitfor-nested`.** The eight that remain are not held by the
        // construct, and `oracle-exec/baseline.txt` names what holds each: six
        // by an `io.kestra.plugin.core.output.OutputValues` child this crate
        // ships no handler for, one by a renderer divergence
        // (`{{ outputs.loop.iterationCount < "5" }}` -- Pebble throws on a
        // relational operator against a quoted operand and minijinja answers),
        // and one by `checkFrequency.maxDuration`, whose golden iteration count
        // carries the artifact's delay-poller latency. This table attributes by
        // DECLARATION, so all eight still count here.
        (
            "LoopUntil / WaitFor",
            8,
            &[
                "io.kestra.plugin.core.flow.LoopUntil",
                "io.kestra.plugin.core.flow.WaitFor",
                "core.loopuntil",
                "core.waitfor",
            ],
        ),
        // **`flow concurrency:` was a row here with 7 through W5 and 6 through
        // W6, and W8 Task 1 REMOVES it rather than lowering it to 0.** Fourth
        // run of the same rule, and the third time the trigger was a state
        // credited to the wrong construct: the row's label in
        // `oracle-exec/README.md` was `concurrency:` -> `CANCELLED`, and
        // `the_cancelled_state_never_comes_from_a_concurrency_limit` derives
        // that `CANCELLED` occurs in exactly three golden entries of which
        // **two are an `sla:`'s and one is an `Exit`'s**. Not one is a limit's.
        // That is `disabled:`/`SKIPPED` again: a row looks reachable because
        // the state really is there, for another reason.
        //
        // The row is deleted rather than lowered because a row attributes an
        // entry to the construct that HOLDS it, and this one holds nothing --
        // five of its six are counted, unmoved, by rows above (`Pause` twice,
        // `afterExecution:` twice, `sla:` once) and the sixth is
        // `valids/flow-concurrency-sla-fail-parent`, which moves to
        // `UNATTRIBUTED` below with the cause that really holds it: an `sla:`
        // in its CHILD. That is exactly where `valids/sla-parent-flow` already
        // sits, for exactly that reason.
        //
        // **And the zero has a mechanism rather than being a zero.**
        // `oracle-exec/harvest.py`'s trace loop is strictly serial -- `for i,
        // rec in enumerate(wanted, 1): execute_once(...)`, each awaited to
        // termination -- so a `limit: 1` is never contended by the harness.
        // That alone would not settle it, because a single flow can contend
        // with itself: `valids/flow-concurrency-parallel-subflow-kill` starts
        // two `Subflow`s of the same limited flow from one `Parallel`, and it
        // is the corpus's ONLY contended population. **W6 Task 3 banked it**,
        // along with `-child` and `valids/flow-concurrency-subflow`, and its
        // golden says why the bank was possible: its three `order` pairs are
        // all `overlapping` and its `spawned` is empty, so what `QUEUE` changes
        // -- WHEN the second child execution starts -- lands in no compared
        // field. The third leg is a control: **10 of the 13 corpus declarers
        // already MATCH** with nothing implemented -- 7 through W8 Task 2, the
        // two `-pause` ones W8 Task 3 banked, and
        // `valids/flow-concurrency-sla-assert-cancel`, which W8 Task 6 banked
        // by implementing `sla:` and nothing else. That tenth is the strongest
        // row the leg has: it is a `QUEUE`-limited flow whose golden ends
        // CANCELLED, and the CANCELLED is now reproduced from the other
        // construct by name.
        // `disabled: true` was a row here through Task 8 with 6 entries, and
        // `runIf:` was a row with 7 through W3a's final review. Both are
        // implemented now, so neither holds anything and a row for either would
        // be an attribution to a construct that is no longer the cause -- the
        // same rule that ADDS a row when a cause is found, run backwards.
        //
        // **`runIf:` banked none of its 7 and the row still goes**, which is the
        // distinction that matters here: an attribution is to the thing that
        // holds an entry, not to a construct the flow happens to declare. Every
        // one of the seven declares at least one construct that still has a row
        // -- measured, and the rows above absorb all seven without moving,
        // which is what the counts here assert. `afterExecution:`,
        // `ExecutionUpdatableTask`, `finally:`, `Subflow` and `WorkingDirectory`
        // are the five, and every one of them is unclaimed or opaque today.
        // **5 until W8 Task 6, 3 now**, and the two that left are the two
        // `EXECUTION_ASSERTION`s (`valids/flow-concurrency-sla-assert-cancel`,
        // `valids/sla-execution-condition`). The three left are the
        // `MAX_DURATION`s, which no implementation can bank under this harness
        // -- `Sleep::capped(Duration::ZERO)` leaves no interval for
        // `MaxDurationSLA.evaluate` to measure. See `oracle-exec/README.md`'s
        // fifth blindness mechanism.
        //
        // **This row's count is a DECLARER count and the CAUSE count is
        // higher**, which is the asymmetry W8 Task 6 records: two more
        // baselined entries are held by an `sla:` in a DIFFERENT flow
        // (`valids/sla-parent-flow` and
        // `valids/flow-concurrency-sla-fail-parent`, each calling a `Subflow`
        // whose target declares the `MAX_DURATION`), and this needle greps one
        // flow's own source, so it structurally cannot see them. Both are in
        // `UNATTRIBUTED` below, by name, with that cause.
        ("sla:", 3, &["\nsla:"]),
        // **The `CREATE_NEW_EXECUTION retry` row was here with 4 and is
        // removed by W6 Task 4**, under the same rule that removed
        // `disabled: true` and `runIf:`: the construct is implemented
        // (`worker::replay`), all four entries are banked, and the needle
        // `"CREATE_NEW_EXECUTION"` now matches **no baselined entry at all**.
        //
        // That last clause is measured rather than assumed, and the assumption
        // it replaces was written down first: a draft of this comment kept the
        // row at 0 "because `valids/retry-flowable-parallel` is the fifth flow
        // declaring `CREATE_NEW_EXECUTION` and deleting the row would orphan
        // it". **It declares no such thing** -- its `retry:` is a bare
        // `type: constant` on a `Loop`, so its behaviour is the default
        // `RETRY_FAILED_TASK`, and it is held by the `Loop` row. Only four
        // corpus flows in 351 write the word, and they are the four this task
        // banked.
        (
            "WorkingDirectory",
            // 4 -> 3 at WorkingDirectory Task 8: `valids/working-directory-taskrun`
            // is banked; the other three (`task-runif-workingdirectory`,
            // `working-directory-invalid-runif`, `-taskrun-nested`) stay baselined.
            3,
            &[
                "io.kestra.plugin.core.flow.WorkingDirectory",
                "core.workingdirectory",
            ],
        ),
    ];

    // Every baselined entry that declares none of them, with what holds it.
    // Sorted, because the derivation walks `baseline()` -- a `BTreeSet`.
    //
    // **There are 36, this comment said 29, and the paragraph below it says a
    // number here would be a third place to be wrong. Both of those were true
    // at once until W8 Task 1's sweep.** The 29 went stale at W6 and nothing
    // caught it, because the derivation compares the LIST and never the count
    // -- which is precisely the argument the paragraph below makes for not
    // writing one. It is kept, corrected, only because the sentence it opens is
    // a history that has earned its place; the derivation remains the authority
    // and this number is not read by anything.
    //
    // This comment said "eleven of the thirteen" until W3b
    // Task 7's sweep. W3b Task 5 added sixteen rows here in one commit and
    // left the sentence above them describing the list before it did -- the
    // same stale-twin shape the README carried, one file over. There were
    // fourteen before that: `valids/switch-impossible` is banked by the
    // null-list fix, and what held it was never a construct the executor lacked
    // but an OVER-REFUSAL in W1's model. This list is where that showed -- an
    // entry no construct explains, because no construct was missing.
    //
    // The count is deliberately NOT restated here as a number: the derivation
    // below compares this table against `baseline()` element by element, so a
    // count in a comment would be a third place for it to be wrong.
    const UNATTRIBUTED: &[(&str, &str)] = &[
        (
            "invalids/dag-invalid-subtask",
            "over-accepted: a @NotNull on a plugin property",
        ),
        (
            "invalids/inputs-bad-validator-syntax",
            "over-accepted: a java.util.regex dialect",
        ),
        (
            "invalids/inputs-with-multiple-constraint-violations",
            "over-accepted: refused at a different seam",
        ),
        (
            "invalids/invalid-property",
            "over-accepted: an unrecognised plugin property",
        ),
        (
            "valids/dag",
            "one `order` pair a clockless machine cannot reach",
        ),
        // **The six W6 Task 3 moved here from the `Subflow` row**, each with
        // the cause that actually holds it now that `Subflow` runs. Five of the
        // six are pinned from a second direction as well:
        // `STILL_OVER_ACCEPTED_AT_RUNTIME` names three of them and
        // `no_corpus_subflow_turns_transmit_failed_off`'s two companions in
        // `executor::state::apply` name the other two.
        (
            "valids/restart-parent",
            "its child `valids/restart-child`'s `1 / taskrun.attemptsCount`              renders Infinity here and throws in Java, so the child SUCCEEDS              and the parent transmits a SUCCESS Kestra never had",
        ),
        (
            "valids/retry-subflow",
            "`state::apply` rule 1: Kestra accumulates a retried `Subflow`'s              sub-execution retries INSIDE the parent's attempt, so its attempt              reads `... FAILED RETRYING CREATED RUNNING FAILED` where a              terminated attempt here takes no further state",
        ),
        (
            "valids/subflow-parent-retry",
            "the same rule 1, twice: `subflow1` and `subflow2` each carry two              attempts of that shape",
        ),
        (
            "valids/sla-parent-flow",
            "`sla:` in its child `valids/sla-subflow`, which egret does not              implement, so the child SUCCEEDS and the parent transmits it",
        ),
        // **Moved here by W8 Task 1, from the `flow concurrency:` row that was
        // deleted above.** It declares a `concurrency:` and that is not what
        // holds it: its golden is `CREATED RUNNING FAILED`, never `CANCELLED`,
        // and the FAILED comes from `MAX_DURATION` in its child. Identical in
        // shape and cause to `valids/sla-parent-flow` two entries up, which is
        // why it sits beside it -- a per-flow needle over `sla:` structurally
        // cannot see either of them.
        (
            "valids/flow-concurrency-sla-fail-parent",
            "`sla:` in its child `valids/flow-concurrency-sla-fail-child`,              which egret does not implement, so the child SUCCEEDS and              `transmitFailed` carries a SUCCESS Kestra never had up to it",
        ),
        (
            "valids/subflow-secret-output-parent",
            "the flow-level `outputs:` block, which nothing in egret evaluates;              on the artifact the PARENT fails while extracting the child's              SECRET output",
        ),
        (
            "valids/task-runif",
            "`io.kestra.plugin.core.output.OutputValues`, which has no handler.              Its `Subflow` is the corpus's only unresolvable target and a falsy              `runIf:` SKIPS it, so `Subflow` was never what held this entry",
        ),
        (
            "valids/flow-with-outputs-failed",
            "flow-level `outputs:`, which nothing in W3a evaluates",
        ),
        // **W3b Task 1's Concern 3 was right: this attribution was wrong.**
        // `valids/full`'s top-level keys are `id`, `labels`, `namespace`,
        // `tasks` -- it has neither `outputs:` nor `variables:`. Re-pointing
        // the oracle at the real driver settles what actually holds it, and it
        // is the same cause as `dag`, `if` and `parallel-disabled-tasks`:
        // `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`
        // measures all four as differing on `order` ALONE.
        ("valids/full", "`order` pairs, same cause as `dag` and `if`"),
        ("valids/if", "three `order` pairs, same cause"),
        (
            "valids/parallel-disabled-tasks",
            "one `order` pair, same cause: with `disabled:` implemented, every \
             run, history and attempt matches and `(task3, task2)` is `before` \
             in Kestra and `overlapping` here",
        ),
        // W3b Task 1 predicted durability would REMOVE this blocker. It does
        // not, and the reason is the driver rather than the executor: the
        // oracle's clock moves only to the next `not_before` and never further
        // (`driver::pass_time`), so `maxDuration` is never reached and egret
        // retries a fourth time where Kestra stopped at three.
        (
            "valids/retry-failed-flow-duration",
            "maxDuration: the driver moves time only to the next deadline",
        ),
        // WorkingDirectory Task 9: Kestra's scheduler adds about 0.6 s per retry,
        // so its fourth attempt falls outside maxDuration and it stops at three.
        // Egret retries with no scheduler latency, so it makes four. Measured
        // 5/5 rounds on the pinned image.
        (
            "valids/retry-failed-task-duration",
            "maxDuration: Kestra's ~0.6 s per-retry scheduler latency stops it at three attempts, egret makes four",
        ),
        // ------------------------------------------------------------------
        // W3b Task 5: the entries the FEED used to carry, each with its cause.
        // ------------------------------------------------------------------
        //
        // A task type this worker writes no handler for. `Registry::run`
        // refuses it, the tick aborts for that run and the execution stalls --
        // which is what the shipping worker does. W3a's harness fed these
        // runs a terminal state out of the golden, which is the whole of why
        // they used to match; W3b Task 1 measured all eleven in advance.
        ("valids/assert", "no handler: OutputValues, Assert"),
        ("valids/flow-with-array-outputs", "no handler: OutputValues"),
        ("valids/output-values", "no handler: OutputValues"),
        ("valids/replay", "no handler: OutputValues"),
        ("valids/webhook-outputs", "no handler: OutputValues"),
        ("valids/webhook-wait", "no handler: OutputValues"),
        ("valids/purge_logs_execution_only", "no handler: PurgeLogs"),
        ("valids/purge_logs_full_arguments", "no handler: PurgeLogs"),
        ("valids/purge_logs_no_arguments", "no handler: PurgeLogs"),
        ("valids/purge_logs_trigger_only", "no handler: PurgeLogs"),
        ("valids/purge_logs_with_batch_size", "no handler: PurgeLogs"),
        // The five the feed hid that Task 1's derivation could not see: a fed
        // terminal hides everything the task would have DONE.
        (
            "valids/errors",
            "`errorLogs()`, which W2 refuses and assigns to W3; there is no \
             task-log store to read",
        ),
        (
            "valids/variables",
            "`render()`, refused by W2 and assigned to W3 in the same table",
        ),
        (
            "valids/webhook-plaintext",
            "`??` over an undefined ROOT (`trigger.…`); egret resolves a missing \
             member and not a missing root",
        ),
        (
            "valids/restart-child",
            "`1 / taskrun.attemptsCount` with a 0 count: Java throws, minijinja \
             renders Infinity",
        ),
        (
            "valids/parallel-fail-with-flowable",
            "a sibling still in flight when its `Parallel` failed; the kill path \
             W4 Task 3 wired reaches only a task's own expired `timeout:`",
        ),
        (
            "valids/retry-success",
            "WARNING, which RunState cannot hold",
        ),
        (
            "valids/subflow-secret-output-child",
            "a SECRET output; the harness has no encryption key",
        ),
    ];

    let baseline = baseline();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unattributed: Vec<String> = Vec::new();

    for id in &baseline {
        let source = corpus_source(id);
        let mut any = false;
        for (label, _, needles) in CONSTRUCTS {
            if needles.iter().any(|n| source.contains(n)) {
                *counts.entry(label).or_default() += 1;
                any = true;
            }
        }
        if !any {
            unattributed.push(id.clone());
        }
    }

    let mut report = String::new();
    for (label, expected, _) in CONSTRUCTS {
        let got = counts.get(label).copied().unwrap_or(0);
        writeln!(report, "  {got:4}  {label}").unwrap();
        assert_eq!(
            got, *expected,
            "{label}: the residual baseline moved. Re-derive this row and the \
             coverage statement in oracle-exec/README.md in the same commit"
        );
    }
    // Sorted for the comparison, grouped by CAUSE in the table above. W3b Task
    // 5 added sixteen rows at once and grouping them by what holds them is the
    // whole point of the table; `baseline()` is a `BTreeSet`, so the derivation
    // arrives sorted and only one of the two can have both properties.
    let mut named: Vec<String> = UNATTRIBUTED
        .iter()
        .map(|(id, _)| (*id).to_string())
        .collect();
    let before_dedup = named.len();
    named.sort();
    named.dedup();
    assert_eq!(
        named.len(),
        before_dedup,
        "an id named twice in UNATTRIBUTED would be two attributions for one \
         entry, and the comparison below would not see it"
    );
    assert_eq!(
        unattributed, named,
        "a baselined entry that declares none of the constructs above belongs to \
         nobody until someone names it. Add it to UNATTRIBUTED with what holds it, \
         or bank it"
    );

    println!(
        "residual baseline: {} entries, attributed by construct (rows OVERLAP; \
         {} declare none and are named individually)\n{report}",
        baseline.len(),
        unattributed.len()
    );
}

/// **The step bound dominates the widest golden entry, derived.**
///
/// `MEASURED_MAX_HISTORY` and the factor of 4 are a bound over an environment
/// that has grown a tier and a render slot since it was written, and its own
/// doc quotes two numbers about the widest entry. Deriving them here is what
/// stops those being counts written beside a derivation — the defect W3a
/// produced seven times.
#[test]
fn the_step_bound_dominates_the_widest_golden_entry() {
    fn runs_of(t: &Trace) -> usize {
        t.runs.len() + t.spawned.iter().map(runs_of).sum::<usize>()
    }
    fn history_of(t: &Trace) -> usize {
        t.execution.len()
            + t.runs
                .iter()
                .map(|r| r.state.len() + r.attempts.iter().map(Vec::len).sum::<usize>())
                .sum::<usize>()
            + t.spawned.iter().map(history_of).sum::<usize>()
    }

    let traces: Vec<Trace> = golden()
        .iter()
        .filter(|e| e.verdict == Verdict::Runs)
        .map(Trace::from)
        .collect();
    let widest_history = traces.iter().map(history_of).max().expect("non-empty");
    let widest_runs = traces.iter().map(runs_of).max().expect("non-empty");
    let widest_top_level = traces
        .iter()
        .map(|t| t.runs.len())
        .max()
        .expect("non-empty");

    println!(
        "widest golden entry: {widest_history} history entries, {widest_runs} runs \
         (with sub-executions), {widest_top_level} at the top level; step bound {}",
        step_bound()
    );
    assert_eq!(
        (widest_history, widest_runs, widest_top_level),
        (765, 94, 13),
        "the three numbers `step_bound`'s derivation quotes. `MEASURED_MAX_HISTORY` \
         is the first; the other two are what W3c's doubling of the tier-2 term is \
         measured against"
    );
    assert!(
        step_bound() >= 4 * widest_history,
        "the factor covers the four tiers"
    );
    // **The bound the DRIVER actually uses**, tied to this derivation rather
    // than written twice. `step_bound()` was W3a's harness's; `driver::STEP_BOUND`
    // is the one that decides `Broken` today, and the two agreeing is what stops
    // this whole derivation from being about a number nothing reads.
    assert_eq!(
        driver::STEP_BOUND,
        step_bound(),
        "the driver's bound and this derivation must be the same number"
    );
    // Tier 2 is the term W3c doubled: two render slots per run instead of one.
    assert!(
        2 * widest_runs < step_bound(),
        "tier 2 can consume at most one step per unrendered slot, and there are \
         two slots per run; {} < {}",
        2 * widest_runs,
        step_bound()
    );
}

// --------------------------------------------------------------------------
// W4 Task 3 — the population the kill path could ever move
// --------------------------------------------------------------------------

/// **Derived, not written down: the kill path's ratchet-visible population is
/// EMPTY, and it is empty three separate ways.**
///
/// W4's spec §1.1 arrived at 0 for this third by looking at the five baselined
/// ids whose NAME contains "kill". That is a grep over identifiers and it is
/// not a derivation — an entry can be reached by the kill path without saying
/// "kill" anywhere, and an entry can say "kill" while declaring nothing this
/// worker runs. So the answer is re-derived here from what the path actually
/// needs, and each leg alone is sufficient:
///
/// 1. **Nothing expires.** `worker::cancel::guarded` calls
///    `TaskHandler::cancel` from exactly one branch —
///    `worker::timeout::Guarded::Expired` — so a kill needs a task-level
///    `timeout:` that runs out.
///    [`the_baselined_timeout_declarations_are_derived_and_none_can_move_the_ratchet`]
///    pins the whole of that population: four declarations, three on a `Pause`
///    (a flowable, whose `timeout:` is `advance()`'s), and the single runnable
///    one an HOUR on a `core.log.Log`. Instrumented over all 294 entries the
///    budget was applied 4 times and expired **0**.
/// 2. **A kill could not be refused even if one fired.** Every type the oracle's
///    driver registers answers `Ok(())`, asserted below against the driver's own
///    registry rather than a copied list.
/// 3. **A kill changes nothing the golden records.** `guarded` has one return
///    path and it returns the budget's outcome; the refusal goes to `tracing`,
///    and `golden.json` holds no failure message in any case.
///
/// The "kill"-named ids are still enumerated, because the spec's number came
/// from them and a reader will look: none of them declares a `timeout:` on
/// anything, so none is reachable by leg 1 either. **The list is a moving one
/// and that is the point** -- five at W5, three at W8 Task 2, two now -- and
/// every departure has been an entry banked for a reason that had nothing to do
/// with killing anything. See the assertion below for each.
#[tokio::test]
async fn the_kill_path_cannot_move_the_ratchet_and_the_named_kill_entries_are_not_its_population() {
    let baseline = baseline();

    // ---- the ids the spec's number came from --------------------------------
    let named: Vec<String> = baseline
        .iter()
        .filter(|id| id.to_lowercase().contains("kill"))
        .cloned()
        .collect();
    assert_eq!(
        named,
        vec![
            "valids/exit-killed".to_string(),
            "valids/flow-concurrency-queue-killed-after-execution".to_string(),
        ],
        "the baselined ids whose name contains `kill`. It was FIVE through W5, \
         THREE through W8 Task 2 and is TWO now, and not one departure was a \
         kill. W6 Task 3 banked `valids/flow-concurrency-parallel-subflow-kill` \
         and its `-child`: the name is the Java test's, the flows declare \
         `concurrency:` with `behavior: QUEUE`, and what QUEUE changes is WHEN \
         a child starts, which a parent's trace does not record. W8 Task 3 \
         banked `valids/pause-kill-delay`, which is a bare `Pause` with \
         `pauseDuration: PT1M` and no `kill` anywhere in it -- it is named for \
         a Java test that kills the execution from OUTSIDE, which a standalone \
         harvest never does, so its golden is the pause simply elapsing"
    );

    // Not one of them declares a `timeout:` anywhere, so leg 1 excludes every
    // one of them regardless of what they contain.
    fn any_timeout(task: &egret_workflow::models::Task) -> bool {
        task.timeout.is_some()
            || task
                .children()
                .into_iter()
                .chain(task.error_children())
                .any(any_timeout)
    }
    // **All FOUR of `Flow`'s task lists**, not the two the first draft walked.
    // `models::Flow` has `tasks`, `errors`, `finally_` (`@JsonProperty("finally")`)
    // and `after_execution` — the fourth being the one `Flow.java:88` has and
    // the porting brief missed. Walking two of four happened to be sound today,
    // because `executor::build_plan` names neither `finally_` nor
    // `after_execution` (grep: no occurrence in `src/executor/plan.rs`), so a
    // `timeout:` there could not reach a claimable run at all. That is a
    // property of an unrelated file, though, and this test must not depend on
    // it silently — so it reads all four and the argument disappears.
    let with_a_budget: Vec<&String> = named
        .iter()
        .filter(|id| {
            let flow = egret_workflow::spec::parse(&corpus_source(id))
                .unwrap_or_else(|e| panic!("{id} does not parse: {}", e.message));
            flow.tasks
                .iter()
                .chain(flow.errors.iter())
                .chain(flow.finally_.iter())
                .chain(flow.after_execution.iter())
                .any(any_timeout)
        })
        .collect();
    assert!(
        with_a_budget.is_empty(),
        "an id named `kill` that DOES carry a `timeout:` would be reachable by \
         the kill path and would have to be attributed: {with_a_budget:?}"
    );

    // ---- leg 2: every handler the ratchet can reach answers `Ok(())` --------
    //
    // Read from the driver's own registry, which is what the oracle runs, so a
    // fifth handler added there is covered without this list being touched.
    let registry = driver::registry(&driver::unconnected_pool());
    let types: Vec<String> = registry
        .type_ids()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        !types.is_empty(),
        "an empty registry would pass leg 2 vacuously"
    );
    for ty in &types {
        let task = egret_workflow::worker::TaskInput {
            workspace_id: uuid::Uuid::nil(),
            execution_id: uuid::Uuid::nil(),
            run_id: uuid::Uuid::nil(),
            task_id: "t".to_string(),
            type_id: ty.clone(),
            value: None,
            attempt: 1,
            spec: serde_json::json!({}),
            render: egret_workflow::runners::RenderContext::new(),
            timeout: None,
            working_dir: None,
            principal: None,
            claimed_at: chrono::DateTime::UNIX_EPOCH,
            log: egret_workflow::logs::RunLog::discard(),
        };
        egret_workflow::worker::Registry::cancel(&registry, &task)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{ty} refuses a kill, so a corpus entry whose budget expired \
                     could produce a refusal the ratchet has never seen: {e}"
                )
            });
    }
}

// --------------------------------------------------------------------------
// W4 Task 2 — the population a worker-enforced `timeout:` could ever move
// --------------------------------------------------------------------------

/// **Derived, not written down: the ratchet-visible population of the task-level
/// `timeout:` hole is EMPTY, and the "four baselined entries" figure counts
/// declarations rather than reachable entries.**
///
/// W4's spec and plan both say *"four baselined entries declare a `timeout:`"*,
/// which is true and is re-derived here (`143 = 4 + 139`). What that sentence
/// does not say is that **three of the four are on a `Pause`** — a flowable,
/// whose `timeout:` is `advance()`'s and which Kestra's worker never sees
/// (measured on the pinned artifact: a `Sequential` carrying `timeout: PT1S`
/// around a four-second `Sleep` ran to SUCCESS in 4.2 s) — and that **the one
/// runnable among them asks for an hour** on a `core.log.Log`, which cannot
/// outrun it.
///
/// So this test is what stops a green ratchet being read as coverage of the
/// worker's budget: nothing in `baseline.txt` can move because of it, in either
/// direction, and that is a property of the corpus rather than of the fix.
/// Instrumenting `worker::timeout::guard` and running the whole 294-entry
/// oracle agreed from the other side — the budget was applied exactly **4**
/// times (`1s`, `1s`, `250ms`, `3600s`, i.e. those four declarations) and
/// **expired 0** times, with `tests/worker.rs`'s own property as the positive
/// control for the instrument on the same build.
///
/// **That measurement was taken at W4 and W8 Task 3 invalidates three quarters
/// of it, which is worth stating rather than leaving to a reader to notice.**
/// Three of the four applications were `Pause`es, and a `Pause` is a
/// `TaskKind::Pause` now: `claim::persist` writes it no `type_id`, the claim's
/// `WHERE type_id IS NOT NULL` never takes it, and `cancel::guarded` is
/// therefore never reached for one. The budget is applied **once** on this
/// build, to `valids/full`'s hour-long `core.log.Log`. The figure has not been
/// re-instrumented here — this test measures DECLARATIONS, not applications —
/// so it is corrected as a claim rather than restated as a new measurement, and
/// whoever needs the application count must instrument it again.
#[test]
fn the_baselined_timeout_declarations_are_derived_and_none_can_move_the_ratchet() {
    fn walk(task: &egret_workflow::models::Task, out: &mut Vec<(String, String, String)>) {
        if let Some(t) = &task.timeout {
            out.push((task.id.clone(), task.r#type.clone(), t.source().to_string()));
        }
        for child in task.children().into_iter().chain(task.error_children()) {
            walk(child, out);
        }
    }

    let baseline = baseline();
    let mut declaring: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    let mut without = 0usize;
    for id in &baseline {
        let flow = egret_workflow::spec::parse(&corpus_source(id))
            .unwrap_or_else(|e| panic!("{id} does not parse: {}", e.message));
        let mut found = Vec::new();
        // **All four of `Flow`'s task lists.** This walked `tasks:` and
        // `errors:` only, which was sound today and derived nothing: W4 Task 3
        // wrote out the argument for reading all four in
        // `the_kill_path_cannot_move_the_ratchet_…` — `finally:` and
        // `afterExecution:` are lists `build_plan` does not name, so a
        // `timeout:` there could not reach a claimable run — and then this test,
        // written one task earlier, was left depending on that unrelated file.
        // The count is unchanged at 4, which is the point: a walk that agrees
        // with the wider one is worth having, and one that only happens to
        // agree is not.
        for task in flow
            .tasks
            .iter()
            .chain(flow.errors.iter())
            .chain(flow.finally_.iter())
            .chain(flow.after_execution.iter())
        {
            walk(task, &mut found);
        }
        if found.is_empty() {
            without += 1;
        } else {
            declaring.insert(id.clone(), found);
        }
    }

    // The total beside the parts, so neither can drift alone.
    assert_eq!(
        declaring.len() + without,
        baseline.len(),
        "every baselined entry is either declaring a `timeout:` or not"
    );
    // (2, 101, 103) -> (2, 102, 104): `valids/retry-failed-task-duration` is baselined
    // and declares no `timeout:` anywhere, so only the denominator and `without` move.
    assert_eq!(
        (declaring.len(), without, baseline.len()),
        (2, 102, 104),
        "the baselined population that declares a `timeout:`, and the rest \
         ((2, 106, 108) until no-silent-stall Task 1 banked \
         `invalids/invalid-task`, which declares none). It \
         was (4, 139, 143) through W5 and (4, 123, 127) through W8 Task 2: W6 \
         Task 3 banked 12 and W6 Task 4 banked 4, and not one of the 16 \
         declares a `timeout:` anywhere, so the numerator did not move through \
         either. **W8 Task 3 moved the numerator for the first time**, by \
         banking `valids/pause-timeout` -- a `timeout:` on a `Pause` is \
         `advance()`'s, not the worker's, and this task is where it started \
         being enforced. W8 Task 4's bank moved the DENOMINATOR only: \
         `valids/waitfor-nested` declares no `timeout:` anywhere, and so did \
         W8 Task 5's seven and W8 Task 6's two: {:#?}",
        declaring
    );
    assert_eq!(
        declaring.keys().cloned().collect::<Vec<_>>(),
        vec![
            "valids/full".to_string(),
            "valids/pause-errors-finally-after-execution".to_string(),
        ],
        "and they are these TWO, by name. `valids/pause-timeout` was the fourth \
         and W8 Task 3 banked it; `valids/pause-timeout-allow-failure` was the \
         third and W8 Task 5 banked it, which is the second time a `timeout:` \
         has left this list and the first time `allowFailure:` decided what the \
         bound's terminal was"
    );

    // **Which of them a WORKER budget could ever apply to**, split by the
    // declaring task's own `type:` rather than by anything about the entry.
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for tasks in declaring.values() {
        for (_, ty, _) in tasks {
            *by_type.entry(ty.clone()).or_default() += 1;
        }
    }
    assert_eq!(
        by_type,
        BTreeMap::from([
            ("io.kestra.plugin.core.flow.Pause".to_string(), 1),
            ("io.kestra.plugin.core.log.Log".to_string(), 1),
        ]),
        "ONE `Pause` — a flowable's timeout, which is `advance()`'s and which \
         Kestra's worker ignores — and one runnable. It was three until W8 Task \
         3 banked `valids/pause-timeout` by ENFORCING one of them, and two \
         until W8 Task 5 banked `valids/pause-timeout-allow-failure` by \
         enforcing the same bound with `allowFailure:` deciding its terminal"
    );

    // The runnable one, and the budget it asks for. `Handlers` is the registry
    // the shipping worker dispatches on, so "a type this worker can run" is
    // read from it rather than asserted as a list.
    let registry =
        egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current());
    let runnable: Vec<(String, String, std::time::Duration)> = declaring
        .iter()
        .flat_map(|(id, tasks)| tasks.iter().map(move |t| (id.clone(), t)))
        .filter(|(_, (_, ty, _))| registry.get(ty).is_some())
        .map(|(id, (task_id, _, source))| {
            (
                id,
                task_id.clone(),
                egret_workflow::executor::parse_duration(source)
                    .expect("every corpus `timeout:` is an ISO-8601 literal"),
            )
        })
        .collect();
    assert_eq!(
        runnable,
        vec![(
            "valids/full".to_string(),
            "t3".to_string(),
            std::time::Duration::from_secs(3_600)
        )],
        "exactly one baselined `timeout:` is on a task this worker has a \
         handler for, and it is an HOUR on a `core.log.Log` — a budget that \
         handler cannot outrun, so the entry cannot move either"
    );
}

// --------------------------------------------------------------------------
// W4 Task 6 — the population `claim::wake` could ever have moved
// --------------------------------------------------------------------------

/// **Derived, not written down: the retry declarations this oracle carries, and
/// which of them the waker could ever reach.**
///
/// W4's spec and brief said *"twelve baselined entries declare a `retry:`"*;
/// W4 Task 1 re-derived **14** and could not reconstruct 12 from any grouping.
/// The coverage statement then quoted the 14 with the words *"which is derived
/// rather than assumed"* — and **nothing derived it**: no test in this crate
/// counted a `retry:` declaration until this one. That is the shape this
/// programme keeps shipping, a number beside a derivation that does not exist,
/// so the count lives here now and the statement cites it.
///
/// Three splits, each with the total asserted beside its parts:
///
/// 1. **Over the whole golden**, because the coverage question is not a
///    question about `baseline.txt`. A change on a *matching* entry is
///    reportable; a change on a baselined one is not, whatever it says — the
///    argument *The ordered pairs: kept is not asserted* makes about pairs,
///    which holds of every field. So the retry population splits into a half
///    the ratchet speaks for and a half it cannot, and only the second half is
///    the waker's.
/// 2. **By `behavior:`**, because the two behaviours reach the waker
///    differently. **This item read *"`CREATE_NEW_EXECUTION` never writes a
///    `not_before` at all … no waker could have touched one"*, and W6 Task 4
///    made both halves false in the commit that could have left them
///    standing.** That arm now emits the `ScheduleRetry` the artifact's own
///    `ExecutionDelay` is (`ExecutorService.java:600-609` builds one for both
///    behaviours and differs only in its `delayType`), the run it names goes
///    `RETRIED` rather than `RETRYING`, and `claim::due_retries` has a second
///    arm for exactly that state — which is how the replacement execution
///    knows when to start. The default is `RETRY_FAILED_TASK`
///    (`RetryBehavior`'s `#[default]`, from `AbstractRetry`'s own), so this is
///    read from the parsed model rather than from the presence of the key.
/// 3. **Flow-level against task-level**, because `Flow::retry` is the same
///    `AbstractRetry` type and a walk over `tasks:` alone would miss the five
///    entries that declare it there.
///
/// What the ratchet says about the waker, after all that, is **nothing** — see
/// the coverage statement's item 11 for the mechanism and its measurement. That
/// is unchanged by W6 Task 4 and is worth saying twice, because the four
/// entries it banked DO now depend on a due deadline being noticed: the oracle
/// notices it with `driver::pass_time`, which rewinds `executions.created_at`
/// and calls `worker::resume` itself, and never through `worker::wake`. The
/// waker's own arm is driven by `tests/worker.rs`'s
/// `a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists`, in
/// real time, which is the only instrument that covers it.
///
/// **The name still holds and is worth re-reading rather than assuming**: it
/// is about the BASELINED population, and the waker reaches none of that —
/// after Task 4 those ten are all `RETRY_FAILED_TASK`, and the ratchet does not
/// exercise the waker for either behaviour.
#[test]
fn the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them() {
    use egret_workflow::models::{Retry, RetryBehavior};

    fn behavior(r: &Retry) -> RetryBehavior {
        match r {
            Retry::Constant { common, .. }
            | Retry::Exponential { common, .. }
            | Retry::Random { common, .. } => common.behavior.clone(),
        }
    }

    fn walk(task: &egret_workflow::models::Task, out: &mut Vec<RetryBehavior>) {
        if let Some(r) = &task.retry {
            out.push(behavior(r));
        }
        for child in task.children().into_iter().chain(task.error_children()) {
            walk(child, out);
        }
    }

    let baseline = baseline();
    // `declaring[id] = (behaviours, flow_level)`.
    let mut declaring: BTreeMap<String, (Vec<RetryBehavior>, bool)> = BTreeMap::new();
    let (mut without, mut unparseable) = (0usize, 0usize);
    let mut total = 0usize;

    for entry in golden() {
        total += 1;
        // A `refused` entry can fail to parse — that is what egret refusing it
        // sometimes means — and an entry this walk cannot read is counted
        // rather than skipped, so the instrument's own blind spot is a number
        // on this page instead of a silent subtraction.
        let Ok(flow) = egret_workflow::spec::parse(&corpus_source(&entry.id)) else {
            unparseable += 1;
            continue;
        };
        let mut found: Vec<RetryBehavior> = Vec::new();
        // **All four of `Flow`'s task lists**, for the reason
        // `the_kill_path_cannot_move_the_ratchet_…` gives: `finally:` and
        // `afterExecution:` are lists a two-list walk silently drops.
        for task in flow
            .tasks
            .iter()
            .chain(flow.errors.iter())
            .chain(flow.finally_.iter())
            .chain(flow.after_execution.iter())
        {
            walk(task, &mut found);
        }
        let flow_level = flow.retry.is_some();
        if let Some(r) = &flow.retry {
            found.push(behavior(r));
        }
        if found.is_empty() {
            without += 1;
        } else {
            declaring.insert(entry.id.clone(), (found, flow_level));
        }
    }

    assert_eq!(
        (declaring.len(), without, unparseable, total),
        (26, 270, 5, 301),
        "every golden entry either declares a `retry:`, does not, or cannot be \
         parsed at all; the parts and the total: {:#?}",
        declaring.keys().collect::<Vec<_>>()
    );

    let (baselined, matching): (Vec<&String>, Vec<&String>) = declaring
        .keys()
        .partition(|id| baseline.contains(id.as_str()));
    assert_eq!(
        (baselined.len(), matching.len(), declaring.len()),
        // 25 -> 26 declaring a `retry:` at WorkingDirectory Task 8: the harvested
        // `retry-failed-task-duration`. Baselined at WorkingDirectory Task 9, so
        // it moves from the matching side to the baselined side: 7 -> 8, 19 -> 18.
        (8, 18, 26),
        "the split the coverage question turns on: a `retry:` on a MATCHING \
         entry is one the ratchet would report a change to, and a `retry:` on \
         a baselined entry is one it would not. matching: {matching:?}"
    );
    assert_eq!(
        baselined.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec![
            "valids/full",
            "valids/retry-failed-flow-duration",
            "valids/retry-failed-task-duration",
            "valids/retry-flowable-parallel",
            "valids/retry-subflow",
            "valids/retry-success",
            "valids/retry-with-flowable-errors",
            "valids/subflow-parent-retry",
        ],
        "the SEVEN, by name (EIGHT until the AllowFailure flowable banked\n         `valids/allow-failure-with-retry`). W4's spec said twelve, W4 Task 1 derived fourteen, \
         W6 Task 4 banked the four `retry-new-execution-*` entries out of the \
         list -- which is the whole of the difference between fourteen and ten \
         -- and W8 Task 5 took BOTH `valids/task-allow-{{warning,failure}}-runnable` \
         out of it, the first `retry:` entries a bank has moved to the matching \
         side since. Their three attempts all still fail; what moved is the \
         run's own terminal after the last of them, which is exactly why the \
         softening is gated on `shouldBeRetried`"
    );

    // The behaviour split over the baselined fourteen: only `RETRY_FAILED_TASK`
    // ever writes a `not_before`, so only those entries could ever have stalled
    // on the missing waker.
    let mut new_execution: Vec<&str> = Vec::new();
    let mut failed_task: Vec<&str> = Vec::new();
    for id in &baselined {
        let (behaviours, _) = &declaring[*id];
        match behaviours.contains(&RetryBehavior::CreateNewExecution) {
            true => new_execution.push(id),
            false => failed_task.push(id),
        }
    }
    assert_eq!(
        (new_execution.len(), failed_task.len(), baselined.len()),
        // (0, 7, 7) -> (0, 8, 8) at WorkingDirectory Task 9: the baselined
        // `retry-failed-task-duration` is a `RETRY_FAILED_TASK` one.
        (0, 8, 8),
        "**every remaining baselined `retry:` is a `RETRY_FAILED_TASK` one**, \
         because W6 Task 4 banked all four `CREATE_NEW_EXECUTION` entries. The \
         assertion under this used to name them; the successor is the matching \
         side below, which is where they went: {new_execution:?}"
    );

    // **The half the ratchet DOES speak for.** A `retry:` on a matching entry
    // is one a change to would be reported, so retrying itself — `ScheduleRetry`,
    // the `RETRYING` state, `reopen_due` putting the run back — is covered here.
    // The waker is the part that is not, and these two counts are what keep the
    // two claims apart.
    let mut matching_failed_task: Vec<&str> = Vec::new();
    let mut matching_new_execution: Vec<&str> = Vec::new();
    for id in &matching {
        let (behaviours, _) = &declaring[*id];
        match behaviours.contains(&RetryBehavior::CreateNewExecution) {
            true => matching_new_execution.push(id),
            false => matching_failed_task.push(id),
        }
    }
    assert_eq!(
        (
            matching_failed_task.len(),
            matching_new_execution.len(),
            matching.len()
        ),
        // (14, 4, 18) -> (15, 4, 19) at WorkingDirectory Task 8: the harvested
        // `retry-failed-task-duration` is a non-baselined `RETRY_FAILED_TASK` entry.
        // (15, 4, 19) -> (14, 4, 18) at WorkingDirectory Task 9: baselining it moves
        // it back to the baselined side.
        (14, 4, 18),
        "the matching half, split the same way. It was (11, 0, 11): every \
         matching entry declared the behaviour that writes a `not_before`, and \
         W6 Task 4 moved the four `CREATE_NEW_EXECUTION` entries across, which \
         is the FIRST time the ratchet speaks for that behaviour at all. W8 \
         Task 5 moved a twelfth and a thirteenth `RETRY_FAILED_TASK` one \
         across, `valids/task-allow-{{warning,failure}}-runnable`, whose `retry:` \
         is exactly what gates their `allowFailure:` -- the first two failures \
         are retried and only the third is softened. \
         failed_task: {matching_failed_task:?}; new_execution: \
         {matching_new_execution:?}"
    );
    assert_eq!(
        matching_new_execution,
        vec![
            "valids/retry-new-execution-flow-attempts",
            "valids/retry-new-execution-flow-duration",
            "valids/retry-new-execution-task-attempts",
            "valids/retry-new-execution-task-duration",
        ],
        "and they are these four, by name -- the entries whose whole replay \
         chain the ratchet would now report a change to"
    );

    // Flow-level against task-level, over the whole 25: a walk over `tasks:`
    // alone would have reported a smaller population and looked right.
    let flow_level: Vec<&String> = declaring
        .iter()
        .filter(|(_, (_, at_flow_level))| *at_flow_level)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        flow_level.len(),
        5,
        "golden entries whose `retry:` is the FLOW's: {flow_level:?}"
    );
}

// ---------------------------------------------------------------------------
// W6 Task 3: the three derivations `src/` cites for `Subflow`
// ---------------------------------------------------------------------------

/// Every `Subflow` task id a corpus flow declares, from all four root lists and
/// through `children()` **and** `error_children()`.
///
/// Wider than `publish::resolve_task`'s walk on purpose and for
/// `driver::subflow_targets`' reason — the question here is *what would a
/// resolver have to reach*, not *what does publish reach today*.
fn subflow_tasks(source: &str) -> BTreeMap<String, egret_workflow::models::Subflow> {
    let mut out = BTreeMap::new();
    let Ok(flow) = egret_workflow::spec::parse(source) else {
        return out;
    };
    fn walk(
        t: &egret_workflow::models::Task,
        out: &mut BTreeMap<String, egret_workflow::models::Subflow>,
    ) {
        if let egret_workflow::models::TaskKind::Subflow(s) = &t.kind {
            out.insert(t.id.clone(), s.clone());
        }
        for c in t.children().into_iter().chain(t.error_children()) {
            walk(c, out);
        }
    }
    for list in [
        &flow.tasks,
        &flow.errors,
        &flow.finally_,
        &flow.after_execution,
    ] {
        for t in list {
            walk(t, &mut out);
        }
    }
    out
}

/// **The measurement W6 opened `src/executor/` for: no `Subflow` run in the
/// golden carries `SUBMITTED`.**
///
/// `SUBMITTED` is the one state in Kestra's record that separates a task the
/// worker ran from one the executor ran (`RunState::Submitted`'s own doc: 580
/// of the golden's 840 run histories), and `flowable::start` emits it for every
/// [`TaskKind::Runnable`]. So a `Subflow` left planned `Runnable` mismatches
/// **every** entry W6 exists to bank, whatever else it gets right — which is
/// why "keep it opaque and let a handler spawn and wait", the cheaper answer,
/// banks nothing.
///
/// Derived over the golden rather than remembered, per the plan's rule that a
/// figure is safe only if a running program compares it. The counts are
/// asserted too: a walk that found no `Subflow` run at all would pass the
/// `SUBMITTED` half vacuously, which is the shape this programme has paid for
/// ten times.
#[test]
fn no_subflow_run_in_the_golden_passes_through_submitted() {
    let (mut executed, mut skipped, mut entries) = (0usize, 0usize, 0usize);
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let subflows = subflow_tasks(&corpus_source(&entry.id));
        if subflows.is_empty() {
            continue;
        }
        let trace = Trace::from(entry);
        let mut here = 0usize;
        let mut stack: Vec<&Trace> = vec![&trace];
        while let Some(t) = stack.pop() {
            for run in &t.runs {
                if !subflows.contains_key(&run.task) {
                    continue;
                }
                here += 1;
                assert!(
                    !run.state.iter().any(|s| s == "SUBMITTED"),
                    "{}: `Subflow` run {:?} reads {:?} in the golden. If this ever \
                     carries SUBMITTED, `TaskKind::Executable` is the wrong shape \
                     and plan.rs's whole argument for opening src/executor/ falls",
                    entry.id,
                    run.task,
                    run.state
                );
                assert!(
                    !run.attempts.iter().flatten().any(|s| s == "SUBMITTED"),
                    "{}: `Subflow` attempt of {:?} carries SUBMITTED",
                    entry.id,
                    run.task
                );
                match run.state.as_slice() {
                    [c, s] if c == "CREATED" && s == "SKIPPED" => skipped += 1,
                    _ => executed += 1,
                }
            }
            stack.extend(t.spawned.iter());
        }
        if here > 0 {
            entries += 1;
        }
    }
    assert_eq!(
        (executed, skipped, entries),
        (24, 1, 22),
        "the population this claim rests on: 24 executed `Subflow` runs and one \
         SKIPPED (`valids/task-runif`'s `notexecutedflowable`), over 22 entries. \
         Three of the 24 are inside a loop sub-execution rather than at the top \
         level (`valids/loop-with-subflow` twice, `valids/restart-parent-loop` \
         once), which is why `planning_the_loop_removed_forty_spurious_submitted_emissions` \
         -- whose reconstruction excludes a loop body, because Task 5's builder \
         had no plan for one -- counts 22 and not 25"
    );
}

/// **The same measurement for `Pause`, and it is W8 Task 3's reason for
/// giving it a `TaskKind` of its own: no `Pause` run in the golden carries
/// `SUBMITTED` either.**
///
/// A `Pause` planned [`TaskKind::Runnable`] gets a `SUBMITTED` from
/// `flowable::start` and a `type_id` from `claim::persist`, and each alone
/// mismatches every entry this task banks — the first on the run history, the
/// second by making the run claimable so `Registry::run` refuses it and the
/// execution stalls with no trace at all.
///
/// The population is asserted beside the property for the reason
/// `no_subflow_run_in_the_golden_passes_through_submitted` asserts its own: a
/// walk that reached no `Pause` run would satisfy the `SUBMITTED` half while
/// measuring nothing, and this crate has paid for that shape more than once.
#[test]
fn no_pause_run_in_the_golden_passes_through_submitted() {
    fn pause_ids(source: &str) -> BTreeSet<String> {
        fn walk(t: &egret_workflow::models::Task, out: &mut BTreeSet<String>) {
            if matches!(t.kind, egret_workflow::models::TaskKind::Pause(_)) {
                out.insert(t.id.clone());
            }
            for c in t.children().into_iter().chain(t.error_children()) {
                walk(c, out);
            }
        }
        let mut out = BTreeSet::new();
        let Ok(flow) = egret_workflow::spec::parse(source) else {
            return out;
        };
        for list in [
            &flow.tasks,
            &flow.errors,
            &flow.finally_,
            &flow.after_execution,
        ] {
            for t in list {
                walk(t, &mut out);
            }
        }
        out
    }

    let (mut runs, mut entries, mut paused) = (0usize, 0usize, 0usize);
    for entry in golden().iter().filter(|e| e.verdict == Verdict::Runs) {
        let ids = pause_ids(&corpus_source(&entry.id));
        if ids.is_empty() {
            continue;
        }
        let trace = Trace::from(entry);
        let mut here = 0usize;
        let mut stack: Vec<&Trace> = vec![&trace];
        while let Some(t) = stack.pop() {
            for run in &t.runs {
                if !ids.contains(&run.task) {
                    continue;
                }
                here += 1;
                assert!(
                    !run.state.iter().any(|s| s == "SUBMITTED"),
                    "{}: `Pause` run {:?} reads {:?} in the golden. If this ever \
                     carries SUBMITTED, `TaskKind::Pause` is the wrong shape and \
                     `Pause` belongs back among the opaque kinds",
                    entry.id,
                    run.task,
                    run.state
                );
                assert!(
                    !run.attempts.iter().flatten().any(|s| s == "PAUSED"),
                    "{}: `Pause` attempt of {:?} carries PAUSED, which is the one \
                     thing `Command::PauseRun` exists to prevent",
                    entry.id,
                    run.task
                );
                if run.state.iter().any(|s| s == "PAUSED") {
                    paused += 1;
                }
            }
            stack.extend(t.spawned.iter());
        }
        if here > 0 {
            entries += 1;
        }
        runs += here;
    }
    assert_eq!(
        (runs, paused, entries),
        (12, 12, 12),
        "the population this claim rests on: twelve golden entries declare a \
         `Pause`, each contributes exactly one `Pause` run, and every one of the \
         twelve reaches PAUSED. The three numbers are asserted together because \
         they can only agree by being right — a walk that found no runs gives \
         (0, 0, 0) and a `PAUSED` count below the run count would mean a golden \
         `Pause` that never paused"
    );
}

/// **The `Pause`es the oracle is structurally blind to, named and counted.**
///
/// Seven corpus flows declare a `Pause` bounded by neither `pauseDuration:` nor
/// `timeout:`. On the pinned artifact each one "reached PAUSED and stayed there
/// for the full 90s" — `corpus.json` says so in as many words, once per flow —
/// so none of them is in `golden.json` and **no line of `baseline.txt` can move
/// because of what egret does with an unbounded pause, in either direction**.
///
/// That is the shape of the over-acceptance W8 Task 3 most risks: a `Pause`
/// that resumed itself after some default would leave the ratchet exactly where
/// it is. `executor::tests::an_unbounded_pause_never_resumes_itself` is the
/// only instrument that can see it; this test is what says so with a running
/// program instead of a sentence, and it fails the moment one of the seven
/// acquires a bound or a golden entry.
#[test]
fn the_unbounded_pauses_are_absent_from_the_golden_and_the_ratchet_cannot_see_them() {
    fn unbounded(source: &str) -> bool {
        fn walk(t: &egret_workflow::models::Task) -> bool {
            let here = match &t.kind {
                egret_workflow::models::TaskKind::Pause(p) => {
                    p.pause_duration.is_none() && t.timeout.is_none()
                }
                _ => false,
            };
            here || t.children().into_iter().chain(t.error_children()).any(walk)
        }
        let Ok(flow) = egret_workflow::spec::parse(source) else {
            return false;
        };
        flow.tasks
            .iter()
            .chain(&flow.errors)
            .chain(&flow.finally_)
            .chain(&flow.after_execution)
            .any(walk)
    }

    let found: Vec<String> = corpus()
        .iter()
        .filter(|e| unbounded(&corpus_source(&e.id)))
        .map(|e| e.id.clone())
        .collect();
    assert_eq!(
        found,
        [
            "valids/follow-dependencies-partial-parent",
            "valids/loop-pause",
            "valids/pause-test",
            "valids/pause_no_tasks",
            "valids/pause_on_resume",
            "valids/pause_on_resume_optional",
            "valids/resume-validate",
        ],
        "the seven corpus flows whose `Pause` has no bound at all"
    );

    let in_golden: BTreeSet<String> = golden().iter().map(|e| e.id.clone()).collect();
    let comparable: Vec<&String> = found.iter().filter(|id| in_golden.contains(*id)).collect();
    assert!(
        comparable.is_empty(),
        "an unbounded `Pause` flow has a golden entry ({comparable:?}), so the \
         oracle CAN now see what egret does with one — which is good news and \
         makes every claim in `flowable::pause`'s header about this blindness \
         stale"
    );
    // The control, so the emptiness above is not the emptiness of a broken
    // walk: the BOUNDED pauses are in the golden, all twelve of them.
    let bounded_in_golden = golden()
        .iter()
        .filter(|e| {
            e.verdict == Verdict::Runs
                && corpus_source(&e.id).contains("core.flow.Pause")
                && !found.contains(&e.id)
        })
        .count();
    assert_eq!(
        bounded_in_golden, 12,
        "twelve `Pause` entries ARE comparable; a zero here would mean the \
         emptiness above proves nothing. `Verdict::Runs` is part of the filter \
         and the first draft left it out, which counted THIRTEEN: \
         `invalids/workingdirectory-invalid` declares a `Pause` too and is a \
         golden entry, but it is a REFUSAL with no trace at all, so counting it \
         among `Pause` traces would be counting a flow nobody ran"
    );
}

/// **The corpus names exactly one `Subflow` target no corpus flow holds, and
/// the run that names it is SKIPPED before anything resolves it.**
///
/// That is what makes `worker::subflow`'s unresolvable-target decision safe to
/// take: egret fails the RUN rather than refusing the FLOW, matching the
/// artifact (`egret.probe/probe-unresolvable`), and the one shape where the two
/// still differ — Kestra never emits `RUNNING`, egret does — is unreachable
/// here. A `build_plan` that refused the flow instead would turn
/// `valids/task-runif` from `runs` into `refused`, which is a regression rather
/// than a bank.
#[test]
fn the_only_unresolvable_subflow_target_in_the_corpus_is_skipped() {
    let held: BTreeSet<String> = corpus()
        .iter()
        .filter_map(|e| egret_workflow::spec::parse(&corpus_source(&e.id)).ok())
        .map(|f| format!("{}/{}", f.namespace, f.id))
        .collect();
    let mut unresolvable: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for entry in corpus() {
        for (task, sub) in subflow_tasks(&corpus_source(&entry.id)) {
            let (Some(ns), Some(id)) = (&sub.namespace, &sub.flow_id) else {
                continue;
            };
            // The one target that is not a name at all --
            // `valids/task-flow-dynamic`'s `{{ inputs.namespace }}` -- is left
            // out here rather than counted as missing: `driver::target_census`
            // owns it as `SUBFLOW_TARGETS_UNRENDERED`, and a flow that has no
            // golden entry cannot be regressed by this decision anyway.
            let name = format!("{ns}/{id}");
            if name.contains("{{") || held.contains(&name) {
                continue;
            }
            unresolvable
                .entry(name)
                .or_default()
                .push((entry.id.clone(), task));
        }
    }
    assert_eq!(
        unresolvable
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect::<Vec<_>>(),
        vec![(
            "dummy_namespace/dummy_flow",
            &[(
                "valids/task-runif".to_string(),
                "notexecutedflowable".to_string()
            )][..]
        )],
        "the whole population of unresolvable literal `Subflow` targets in the \
         corpus. `driver::SUBFLOW_TARGETS_ABSENT` counts the same one from the \
         other side"
    );

    let entry = golden()
        .into_iter()
        .find(|e| e.id == "valids/task-runif")
        .expect("valids/task-runif has a golden entry");
    let run = entry
        .runs
        .iter()
        .find(|r| r.task == "notexecutedflowable")
        .expect("its `notexecutedflowable` run is in the golden");
    assert_eq!(
        run.state,
        ["CREATED", "SKIPPED"],
        "the only unresolvable target in the corpus belongs to a run a falsy \
         `runIf:` skips, so nothing egret does when a target does not resolve is \
         visible to the ratchet -- which is why `worker::subflow` measured the \
         artifact for it instead"
    );
}

/// **No corpus `Subflow` writes `transmitFailed: false`.**
///
/// Cited twice in `src/`: by `executor::state::apply`, for the attempt-history
/// rule its two-step terminal would break, and by `worker::subflow::guess_state`,
/// which implements the branch anyway. Both statements are only safe while this
/// holds, so it is derived rather than remembered.
///
/// The default is `true` (`Subflow.java:132`) and `models::Subflow` carries the
/// same default, so this reads the effective value and not the presence of a
/// key.
#[test]
fn no_corpus_subflow_turns_transmit_failed_off() {
    let mut declarers = 0usize;
    let mut subflows = 0usize;
    for entry in corpus() {
        let tasks = subflow_tasks(&corpus_source(&entry.id));
        if tasks.is_empty() {
            continue;
        }
        declarers += 1;
        for (task, sub) in tasks {
            subflows += 1;
            assert!(
                sub.transmit_failed,
                "{}: `Subflow` {task:?} turns `transmitFailed` off, which makes \
                 `guess_state`'s second branch reachable -- and `executor::state::apply` \
                 rule 1 would then drop the second state off the attempt. Both \
                 doc comments say this cannot happen",
                entry.id
            );
        }
    }
    assert_eq!(
        (declarers, subflows),
        (25, 27),
        "the population: corpus flows declaring at least one `Subflow`, and the \
         `Subflow` tasks between them"
    );
}

/// **The deepest chain of `Subflow` targets the corpus holds is 2 links — three
/// flows — and its only cycle is a self-cycle both sides refuse.**
///
/// Both halves matter to `worker::subflow`, which is where the one answer this
/// package gives that is not the artifact's lives. The **cycle** half is the
/// load-bearing one: nothing on either side refuses A -> B -> A, and if the
/// corpus ever grew one the driver would run it — into a recursion that
/// overflowed a stack when it was measured. The **depth** half says that
/// `MAX_SPAWN_CHAIN` cannot be reached by anything here.
///
/// **This measures the SUBFLOW GRAPH, and that is narrower than what the guard
/// walks** — the name says so since fix round 1's Minor 3, which pointed out
/// that the old one (`…_is_far_inside_the_bound`) promised a bound that at the
/// time did not exist. `worker::subflow::chain_holds` walks the EXECUTION
/// chain, which counts
/// a `Loop` sub-execution as a link too; this walk cannot see loop nesting at
/// all. So the 2 below is a lower bound on the real chain, and what says
/// `MAX_SPAWN_CHAIN` refuses nothing real is the ratchet: **127** with the same
/// twelve — `131` when the bound went in and `127` since W6 Task 4's four
/// replay banks, re-run at both.
///
/// Walked over `(namespace, flowId)` pairs with a visited set, because the
/// graph **can** cycle — `spec::validate`'s `self_dependency` refuses only a
/// flow that names itself, so A → B → A is publishable on both sides. A walk
/// without one would not terminate, which is the same fact the constant exists
/// for.
#[test]
fn the_corpus_subflow_graph_has_no_cycle_and_no_deep_chain() {
    let by_name: BTreeMap<String, String> = corpus()
        .iter()
        .filter_map(|e| {
            let src = corpus_source(&e.id);
            egret_workflow::spec::parse(&src).ok().map(|f| {
                (
                    format!("{}/{}", f.namespace, f.id),
                    // The first file to claim a name wins, which is
                    // `driver::publish_corpus`'s rule and `UNIQUE (workspace_id,
                    // namespace, flow_id)`'s.
                    src,
                )
            })
        })
        .fold(BTreeMap::new(), |mut acc, (k, v)| {
            acc.entry(k).or_insert(v);
            acc
        });

    fn depth(name: &str, by_name: &BTreeMap<String, String>, seen: &mut BTreeSet<String>) -> usize {
        let Some(src) = by_name.get(name) else {
            return 0;
        };
        if !seen.insert(name.to_string()) {
            // A cycle. Reported as the bound rather than as infinity: this test
            // is about how deep a chain the corpus really has, and a cycle in it
            // would be caught by the assertion below.
            return usize::MAX;
        }
        let mut best = 0;
        for (_task, sub) in subflow_tasks(src) {
            let (Some(ns), Some(id)) = (&sub.namespace, &sub.flow_id) else {
                continue;
            };
            let target = format!("{ns}/{id}");
            if target.contains("{{") {
                continue;
            }
            let d = depth(&target, by_name, seen);
            best = best.max(if d == usize::MAX { d } else { d + 1 });
        }
        seen.remove(name);
        best
    }

    let mut deepest = (0usize, String::new());
    let mut cyclic: BTreeSet<String> = BTreeSet::new();
    for name in by_name.keys() {
        let d = depth(name, &by_name, &mut BTreeSet::new());
        if d == usize::MAX {
            cyclic.insert(name.clone());
            continue;
        }
        if d > deepest.0 {
            deepest = (d, name.clone());
        }
    }

    // **The corpus holds exactly one cycle and it is a SELF-cycle**, which both
    // sides already refuse: `spec::validate`'s `self_dependency` is egret's
    // half, and Kestra's own verdict for the entry is `refused`. A cycle of
    // length two or more is what nothing refuses, and there is none — if one
    // ever appears here, the driver will run it, into the recursion that
    // overflowed a stack when `worker::subflow`'s cycle guard was measured.
    assert_eq!(
        cyclic.iter().cloned().collect::<Vec<_>>(),
        vec!["io.kestra.tests/recursive-flow".to_string()],
        "every cycle of `Subflow` targets in the corpus"
    );
    assert!(
        egret_workflow::executor::build_plan(&corpus_source("invalids/recursive-flow")).is_err(),
        "and egret refuses to plan it, so nothing reaches the cycle guard for it"
    );
    assert_eq!(
        subflow_tasks(&corpus_source("invalids/recursive-flow"))
            .values()
            .map(|s| format!(
                "{}/{}",
                s.namespace.as_deref().unwrap_or(""),
                s.flow_id.as_deref().unwrap_or("")
            ))
            .collect::<Vec<_>>(),
        vec!["io.kestra.tests/recursive-flow".to_string()],
        "it is a SELF-cycle -- length one -- which is the case `self_dependency` \
         catches. The case nothing catches is A -> B -> A, and the corpus has none"
    );

    assert_eq!(
        deepest,
        (
            2,
            "io.kestra.tests/flow-concurrency-parallel-subflow-kill".to_string()
        ),
        "the deepest chain, counted in LINKS and named so a change says which \
         flow moved: `flow-concurrency-parallel-subflow-kill -> -child -> \
         -grandchild` is two links and three flows, and \
         `io.kestra.tests/subflow-parent -> subflow-child -> \
         subflow-grand-child` is the other. Two nested settles is nowhere near \
         what overflowed a stack when the cycle was measured (it did so before \
         16 links), so nothing in the corpus reaches the hazard \
         `worker::subflow`'s cycle guard leaves open"
    );
}

// --------------------------------------------------------------------------
// W8 Task 6: the `MAX_DURATION` refusal, as a running program
// --------------------------------------------------------------------------

/// The three corpus flows whose own `sla:` is a `MAX_DURATION`, with the bound
/// each declares in milliseconds.
///
/// Derived-with-a-literal on purpose: the ids and the bounds are read back out
/// of `corpus/` by [`the_max_duration_declarations_are_exactly_these`], so this
/// table cannot silently disagree with the YAML, and it is spelled out here so
/// a reader of the refusal has the numbers in front of them.
const MAX_DURATION_FLOWS: &[(&str, i64)] = &[
    ("valids/sla-max-duration-fail", 500),
    ("valids/sla-subflow", 1_000),
    ("valids/flow-concurrency-sla-fail-child", 3_000),
    // `valids/sla-max-duration-ok`'s bound is PT10S over a `PT0.5S` sleep. It
    // is the CONTROL and it is deliberately in this list: it is the one
    // `MAX_DURATION` flow that MATCHES, and it matches for a reason that has
    // nothing to do with the SLA -- it would match under a correct
    // implementation and under none at all, so it discriminates nothing. Having
    // it here is what stops "the three baselined ones" being read as "the
    // `MAX_DURATION` population".
    ("valids/sla-max-duration-ok", 10_000),
];

/// **The `MAX_DURATION` population is read off the corpus, not remembered.**
///
/// Four flows in 351 declare one (none of the six `working-directory-real-*`
/// flows declares a `sla:`), and the two `*-parent` entries that are ALSO
/// held by a `MAX_DURATION` declare no `sla:` at all — which is the asymmetry
/// [`the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded`] measures
/// and this test is the other half of: a per-flow needle sees exactly these
/// four and cannot see those two.
#[test]
fn the_max_duration_declarations_are_exactly_these() {
    let mut found: Vec<(String, i64)> = Vec::new();
    for entry in corpus() {
        let source = corpus_source(&entry.id);
        if !source.contains("type: MAX_DURATION") {
            continue;
        }
        // `duration: PT…` on the line after, in any order relative to
        // `behavior:`; the corpus writes both orders and a parser that assumed
        // one would silently find three flows.
        let iso = source
            .lines()
            .find_map(|l| l.trim().strip_prefix("duration: ").map(str::to_string))
            .unwrap_or_else(|| panic!("{} declares a MAX_DURATION with no duration:", entry.id));
        let ms = iso_millis(&iso)
            .unwrap_or_else(|| panic!("{}: {iso:?} is not an ISO-8601 duration", entry.id));
        found.push((entry.id.clone(), ms));
    }
    found.sort();
    let mut expected: Vec<(String, i64)> = MAX_DURATION_FLOWS
        .iter()
        .map(|(id, ms)| ((*id).to_string(), *ms))
        .collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "the corpus's whole `MAX_DURATION` population, with each bound read \
         back out of the YAML. `MAX_DURATION_FLOWS` is what the refusal is \
         stated against, so it must be derivable"
    );
    // The needle's blind spot, asserted rather than described: neither entry
    // held by a MAX_DURATION *in another flow* is in the list above.
    for parent in [
        "valids/sla-parent-flow",
        "valids/flow-concurrency-sla-fail-parent",
    ] {
        assert!(
            !corpus_source(parent).contains("sla:"),
            "{parent} is held by a `MAX_DURATION` and declares no `sla:` of its \
             own -- that is why the DECLARER count (4) and the CAUSE count (5) \
             differ, and why a per-flow grep is the wrong instrument for the \
             second"
        );
        assert!(
            baseline().contains(parent),
            "{parent} is still baselined; if it banks, the cause count moves \
             and this whole section has to be re-derived"
        );
    }
}

/// `PT…S` to milliseconds, for the three shapes the corpus writes
/// (`PT0.5S`, `PT1S`, `PT3S`, `PT10S`). Deliberately tiny: a real ISO parser
/// lives in `executor::parse_duration` and is not reachable from an integration
/// test, and a wrong answer here fails
/// [`the_max_duration_declarations_are_exactly_these`] loudly rather than
/// quietly.
fn iso_millis(iso: &str) -> Option<i64> {
    let secs: f64 = iso.strip_prefix("PT")?.strip_suffix('S')?.parse().ok()?;
    Some((secs * 1000.0).round() as i64)
}

/// **The `MAX_DURATION` window does not exist in this harness** — the
/// measurement behind W8 Task 6's refusal, and the reason five golden entries
/// are unbankable by any implementation **given this harness** (fix round 2's
/// qualifier: uncapping `driver.rs`'s `Sleep` moves all five, so "whatsoever"
/// promised more than the sentence below delivers).
///
/// # The claim, stated precisely enough to survive a reader who knows the clock
/// moves
///
/// `MaxDurationSLA.evaluate` compares `Duration.between(execution.startDate,
/// Instant.now())` against the bound, so it needs an interval in which the
/// execution is **non-terminal** and **older than the bound**. Two facts about
/// `oracle-exec/driver.rs` remove it:
///
/// 1. the registry is `Sleep::capped(Duration::ZERO)`, and `Sleep::run` is
///    synchronous inside one `worker::tick` — so the only task these flows have
///    reaches its terminal inside the tick that started it, and writes **no**
///    `not_before_ms`;
/// 2. `driver::pass_time` is the only thing in this harness that advances an
///    execution's clock, and its query selects on `r.not_before_ms IS NOT NULL`
///    — so with no deadline registered it returns `Ok(false)` the first time it
///    is asked and the driver's loop breaks.
///
/// **The clock really does move in this harness**, which is why the first fact
/// alone is not the answer: `pass_time` rewinds `executions.created_at` by
/// `shortfall + 1` and calls `worker::resume`, landing 1 ms past every deadline
/// it is given. What these flows lack is a deadline for it to move *to*. The
/// window is not entered late and not missed — there is no instant at which it
/// could be evaluated.
///
/// # This is true of the FOUR DECLARERS, and the two parents have a different
/// and stronger reason — W8 Task 6 fix round 1
///
/// Everything above and everything asserted below is measured over the four
/// flows that **declare** a `MAX_DURATION`. `valids/sla-parent-flow` and
/// `valids/flow-concurrency-sla-fail-parent` are not among them: they declare
/// none, they are absent from the registrar control printed below, and the
/// sentence *"no run of these flows is ever in flight across a step"* is
/// **false** for them — a `Subflow` run necessarily spans steps, which is the
/// whole of how it waits for its child. Generalising the four's measurement to
/// all five was W8 Task 6's error and fix round 1's narrowing.
///
/// Their mechanism is asserted separately, in
/// [`the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded`], and it is
/// the better fact: `pass_time`'s `WITH RECURSIVE` walks
/// `parent_execution_id`, a subflow child is linked by `trigger_execution_id`,
/// and so the clock-advance walk **cannot reach the child at all** — which
/// would hold even if the child registered a deadline.
///
/// # What is asserted, and why the zero is not asserted alone
///
/// The load-bearing assertion is a **zero** — no deadline is ever registered —
/// and a zero is what a broken query returns too. So it is asserted beside a
/// **control derived from the same query on the same rows**: the driven corpus
/// contains flows that DO register a deadline, they are named, and the count is
/// positive. A query that found nothing would fail on that half.
#[tokio::test(flavor = "multi_thread")]
async fn the_max_duration_window_does_not_exist_in_this_harness() {
    // The corpus has already been driven, and every row it wrote is still
    // there: `egret_side` TRUNCATEs once, inside its `OnceCell`, before the
    // first entry runs. Awaiting it here is what orders this test after that.
    let _ = egret_side().await;
    let db = common::db().await.expect("the oracle needs DATABASE_URL");

    let rows: Vec<(String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT e.flow_key, \
                count(*)::bigint AS runs, \
                count(r.not_before_ms)::bigint AS deadlines, \
                max(EXTRACT(EPOCH FROM (r.updated_at - e.created_at)) * 1000)::float8 \
                  AS elapsed_ms \
           FROM workflow.executions e \
           JOIN workflow.task_runs r ON r.execution_id = e.id \
          GROUP BY e.flow_key",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read the deadline census");

    let by_flow: BTreeMap<&str, (i64, i64, Option<f64>)> = rows
        .iter()
        .map(|(k, runs, deadlines, elapsed)| (k.as_str(), (*runs, *deadlines, *elapsed)))
        .collect();

    // ---- the control, first: the zero below has to be falsifiable ----------
    let registrars: Vec<&str> = rows
        .iter()
        .filter(|(_, _, deadlines, _)| *deadlines > 0)
        .map(|(k, ..)| k.as_str())
        .collect();
    println!(
        "flows that registered at least one `not_before_ms` under the driver: \
         {} of {} -- {registrars:?}",
        registrars.len(),
        rows.len()
    );
    assert!(
        registrars.len() >= 5,
        "the same query, over the same rows, must find flows that DO register a \
         deadline -- otherwise the zeroes below are a broken query rather than a \
         measurement. Found {registrars:?}"
    );

    // ---- the measurement ---------------------------------------------------
    for (id, bound_ms) in MAX_DURATION_FLOWS {
        let key = id.strip_prefix("valids/").expect("corpus ids are prefixed");
        let (runs, deadlines, elapsed) = by_flow
            .get(key)
            .copied()
            .unwrap_or_else(|| panic!("{id} produced no task_runs at all"));
        println!(
            "{id}: bound {bound_ms} ms, {runs} run(s), {deadlines} deadline(s), \
             elapsed at the last run write {elapsed:?} ms"
        );
        assert_eq!(
            deadlines, 0,
            "{id}: a capped `Sleep` registers no `not_before_ms`, so \
             `driver::pass_time` has nothing to move the clock TO and returns \
             `Ok(false)` on its first call. This is the whole of why the \
             `MAX_DURATION` window does not exist -- not that it is missed"
        );
        let elapsed = elapsed.expect("a row that has runs has an elapsed");
        assert!(
            elapsed < *bound_ms as f64,
            "{id}: the execution's last run row was written {elapsed} ms after \
             the execution was created, and the bound is {bound_ms} ms. This is \
             wall-clock rather than a logical clock, and the margin is two \
             orders of magnitude on an idle machine -- if it ever fails, the \
             answer is NOT to loosen it: it would mean the driver really did \
             leave a window open and the refusal has to be re-derived"
        );
    }

    // ---- and the run shape, which is the structural half -------------------
    //
    // "No run is ever in flight across a step" is what makes the elapsed figure
    // above irrelevant to the answer rather than merely comfortable. A `Sleep`
    // that reached PAUSED, or RETRYING, or was left RUNNING, would be a run the
    // executor had to come back to -- and coming back is the only thing that
    // puts a second evaluation point after the first.
    let sleeps: Vec<(String, String)> = sqlx::query_as(
        "SELECT e.flow_key, r.history::text \
           FROM workflow.executions e JOIN workflow.task_runs r ON r.execution_id = e.id \
          WHERE r.task_id = 'sleep' AND e.flow_key = ANY($1) \
          ORDER BY e.flow_key",
    )
    .bind(
        MAX_DURATION_FLOWS
            .iter()
            .map(|(id, _)| id.trim_start_matches("valids/").to_string())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&db.pool)
    .await
    .expect("read the sleep runs");
    // **SIX, not four**, and the two extra are the evidence rather than noise:
    // `sla-subflow` and `flow-concurrency-sla-fail-child` each ran TWICE in
    // this corpus -- once as their own golden entry and once as the child
    // `valids/sla-parent-flow` / `valids/flow-concurrency-sla-fail-parent`
    // spawned. Both copies go straight through, which is the same fact one
    // level down and is what
    // `the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded`
    // measures from the other side.
    assert_eq!(
        sleeps.len(),
        6,
        "one `sleep` run per `MAX_DURATION` flow, plus the second run of each \
         of the two that are also a `Subflow` TARGET: {sleeps:?}"
    );
    for (flow, history) in &sleeps {
        assert_eq!(
            history, "[\"CREATED\", \"SUBMITTED\", \"RUNNING\", \"SUCCESS\"]",
            "{flow}: the capped `Sleep` goes straight through. A `PAUSED` or a \
             `RETRYING` here would be a run the executor comes back to, which \
             is the only shape that gives this harness a second instant to \
             evaluate an SLA at"
        );
    }
}

/// **The two `*-parent` entries are held one level down, and by a child that
/// RAN** — W8 Task 6, and it is the measurement that keeps the refusal pinned
/// to the right mechanism.
///
/// `valids/sla-parent-flow` and `valids/flow-concurrency-sla-fail-parent`
/// declare no `sla:`. Each calls a `Subflow` whose target declares a
/// `MAX_DURATION`, and on the artifact the child FAILED and the parent's run
/// FAILED with it. egret's parent run SUCCEEDS.
///
/// **There are two ways to get a mismatch there and only one of them is the
/// `sla:`.** The other is that the target does not resolve at all: `publish`
/// refuses every corpus flow containing an `io.kestra.plugin.core.flow.Sleep`
/// (the spec registry has no row for it), so both children are **draft-only**
/// in `workflow.flows`, and a reader who stopped at the publish report would
/// conclude the parent never spawned anything. That reading is wrong and the
/// direction matters: `worker::subflow::spawn_one` FAILS the parent's run for
/// an unresolvable target, which is the same direction as the golden and would
/// have looked like the `sla:` working.
///
/// So this asserts what actually happened: the target resolves (`subflow::
/// resolve` selects any non-deleted, non-disabled row, draft or live), a child
/// execution exists, it ran its `Sleep`, and it ended **SUCCESS** — which is
/// the capped `Sleep` again, one level down.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded() {
    let _ = egret_side().await;
    let db = common::db().await.expect("the oracle needs DATABASE_URL");

    for (parent, child) in [
        ("sla-parent-flow", "sla-subflow"),
        (
            "flow-concurrency-sla-fail-parent",
            "flow-concurrency-sla-fail-child",
        ),
    ] {
        // A subflow child is linked by `trigger_execution_id`, never by
        // `parent_execution_id` -- migration 0138's CHECK makes carrying both
        // unrepresentable -- so this is the only walk that finds one.
        let children: Vec<(String, String)> = sqlx::query_as(
            "SELECT c.flow_key, c.state \
               FROM workflow.executions p \
               JOIN workflow.executions c ON c.trigger_execution_id = p.id \
              WHERE p.flow_key = $1 \
              ORDER BY c.created_at",
        )
        .bind(parent)
        .fetch_all(&db.pool)
        .await
        .expect("walk the trigger link");

        assert_eq!(
            children,
            vec![(child.to_string(), "SUCCESS".to_string())],
            "{parent} must have spawned exactly one child, {child}, and that \
             child must have SUCCEEDED. A child that is absent would mean the \
             target did not resolve -- a different cause with the same \
             direction as the golden -- and a child that FAILED would mean \
             something other than the capped `Sleep` is holding this entry"
        );

        // …and the child really ran its task, rather than resolving to an empty
        // execution: the `Sleep` is the whole of what a `MAX_DURATION` would
        // have had time to kill.
        let sleeps: Vec<(String,)> = sqlx::query_as(
            "SELECT r.state FROM workflow.executions e \
               JOIN workflow.task_runs r ON r.execution_id = e.id \
              WHERE e.trigger_execution_id IS NOT NULL AND e.flow_key = $1",
        )
        .bind(child)
        .fetch_all(&db.pool)
        .await
        .expect("read the child's runs");
        assert_eq!(
            sleeps,
            vec![("SUCCESS".to_string(),)],
            "{child}, as spawned by {parent}, ran exactly one task and it \
             succeeded"
        );

        // **The PREMISE the SUCCESS above rests on, named and asserted** --
        // W8 Task 6 fix round 1. "The child ended SUCCESS because its capped
        // `Sleep` succeeded" is the whole story only if the `Sleep` is the
        // child's ONLY failure road; that came from reading the YAML, not from
        // this test, and an unasserted premise is how a merely COMPATIBLE
        // explanation becomes a believed one.
        let tasks: Vec<(String,)> = sqlx::query_as(
            "SELECT r.task_id FROM workflow.executions e \
               JOIN workflow.task_runs r ON r.execution_id = e.id \
              WHERE e.trigger_execution_id IS NOT NULL AND e.flow_key = $1",
        )
        .bind(child)
        .fetch_all(&db.pool)
        .await
        .expect("read the child's task ids");
        assert_eq!(
            tasks,
            vec![("sleep".to_string(),)],
            "{child} has exactly ONE task and it is the `Sleep`, so its SUCCESS \
             cannot have come from anywhere else"
        );

        // **And the fifth entry's mechanism, which is STRONGER than the other
        // four's and is a DIFFERENT fact** -- W8 Task 6 fix round 1.
        //
        // For the three declarers, the reason `pass_time` cannot move the clock
        // is that no deadline is registered. For the two PARENTS there is a
        // reason that would hold even if one were: `pass_time`'s `WITH
        // RECURSIVE` walks `e.parent_execution_id`, and a subflow child is
        // linked by `trigger_execution_id` -- migration 0138's
        // `workflow_executions_one_link_kind` makes carrying both
        // unrepresentable. **The clock-advance walk cannot reach the child at
        // all.** This runs that exact CTE and asserts it returns only the
        // parent.
        let reachable: Vec<(String,)> = sqlx::query_as(
            "WITH RECURSIVE tree AS ( \
                 SELECT id, flow_key FROM workflow.executions WHERE flow_key = $1 \
                 UNION ALL \
                 SELECT e.id, e.flow_key FROM workflow.executions e \
                   JOIN tree t ON e.parent_execution_id = t.id \
             ) SELECT DISTINCT flow_key FROM tree ORDER BY flow_key",
        )
        .bind(parent)
        .fetch_all(&db.pool)
        .await
        .expect("walk pass_time's own tree");
        assert_eq!(
            reachable,
            vec![(parent.to_string(),)],
            "`driver::pass_time`'s recursive CTE joins `parent_execution_id`, \
             so the subflow child {child} is not in the tree it moves the clock \
             for -- even a child that DID register a `not_before_ms` could not \
             be reached"
        );

        assert!(
            baseline().contains(&format!("valids/{parent}")),
            "valids/{parent} is still baselined -- this test describes why, and \
             if it banks the description is stale"
        );
    }
}

/// **W8 Task 7 final round: the two `EXECUTION_ASSERTION` banks are
/// DISCRIMINATING, driven rather than argued.**
///
/// Both banked entries end `["CREATED","CANCELLED"]` with **0 runs** and they
/// are the corpus's only two `EXECUTION_ASSERTION` declarers, so *"violate
/// every `EXECUTION_ASSERTION` unconditionally"* banks both of them **byte for
/// byte**. The ratchet cannot tell that implementation from the right one, and
/// the attribution mutation for W8 Task 6 cannot either: reverting the feature
/// makes both entries mismatch under either reading.
///
/// `corpus.json` records what closes it. `valids/sla-execution-condition`
/// declares `inputs.string` as a `SELECT` over `PASS`/`CANCEL`/`LABEL` and the
/// harvest drove exactly one of the three — `CANCEL` — leaving **two
/// `inputs_alternatives` unused**, each with Kestra's own test method beside it:
///
/// | input | `inputs_from` | what must happen |
/// |---|---|---|
/// | `CANCEL` | `SLATestCase.java:62 executionConditionSLAShouldCancel()` | the golden: `CANCELLED`, 0 runs |
/// | `LABEL` | `SLATestCase.java:86 executionConditionSLAShouldLabel()` | the **`NONE`**-behaviour SLA is violated, which changes no state |
/// | *(none — `defaults: PASS`)* | `SLATestCase.java:56 executionConditionSLAShouldPass()` | **neither** assertion is violated |
///
/// **`PASS` is the row that does the work.** An implementation that violates
/// unconditionally cancels it; a correct one runs the flow. `LABEL` is the
/// second half and is a different question — there the assertion **is**
/// violated and the behaviour is `NONE`, so it separates *"violated"* from
/// *"cancelled"*, which `PASS` alone cannot.
///
/// # Why this is a test and not a bank
///
/// There is no golden entry to bank: `harvest.py` posts one input set per flow,
/// so the artifact's answers for `PASS` and `LABEL` were never captured and
/// **the ratchet cannot move by one entry here, in either direction**. The
/// authority for the two expectations is Kestra's own test *names*, recorded in
/// `corpus.json`'s `inputs_from` by the harvest — which is weaker than a
/// captured trace and is said so rather than dressed up. What this test
/// establishes is the discrimination: the same `src/sla.rs` that banks the two
/// entries also declines to cancel the two inputs that must not be cancelled,
/// and that discrimination used to live only in `sla.rs`'s own truthy/falsy
/// unit tables, where nothing connected it to the banked entries.
///
/// `egret_side()` is awaited first and its result discarded, purely to
/// serialise behind its one-time `TRUNCATE workflow.executions CASCADE`; this
/// test then seeds its own workspace and drives its own executions.
#[tokio::test(flavor = "multi_thread")]
async fn the_execution_assertion_banks_discriminate_on_the_inputs_the_harvest_did_not_drive() {
    let _ = egret_side().await;

    let db = common::db()
        .await
        .expect("this drives a REAL executor and needs DATABASE_URL");
    let (ws, _user) = common::seed_workspace(&db).await;
    let source = corpus_source("valids/sla-execution-condition");
    let reg = driver::registry(&db.pool);

    // (label, inputs, must the execution end CANCELLED?, how many runs)
    let cases: [(&str, serde_json::Value, bool, usize); 3] = [
        (
            "CANCEL (the golden's own input)",
            serde_json::json!({"string": "CANCEL"}),
            true,
            0,
        ),
        (
            "PASS (the SELECT's default, undriven)",
            serde_json::json!({}),
            false,
            1,
        ),
        (
            "LABEL (violated, behaviour NONE, undriven)",
            serde_json::json!({"string": "LABEL"}),
            false,
            1,
        ),
    ];

    let mut wrong: Vec<String> = Vec::new();
    for (label, inputs, expect_cancelled, expect_runs) in cases {
        let t = match driver::run(&db.pool, ws, &reg, &source, inputs.clone()).await {
            driver::Ran::Trace(t) => *t,
            driver::Ran::Refused(msg) => {
                wrong.push(format!("{label}: egret REFUSED the flow: {msg}"));
                continue;
            }
            driver::Ran::Broken(msg) => {
                wrong.push(format!("{label}: the driver broke: {msg}"));
                continue;
            }
        };
        let last = t.execution.last().cloned().unwrap_or_default();
        let cancelled = last == "CANCELLED";
        if cancelled != expect_cancelled || t.runs.len() != expect_runs {
            wrong.push(format!(
                "{label}: expected cancelled={expect_cancelled} with {expect_runs} run(s), \
                 got cancelled={cancelled} with {} run(s); execution {:?}, runs {:?}",
                t.runs.len(),
                t.execution,
                t.runs
                    .iter()
                    .map(|r| (&r.task, &r.state))
                    .collect::<Vec<_>>()
            ));
        }
    }

    assert!(
        wrong.is_empty(),
        "the `EXECUTION_ASSERTION` banks are not discriminating -- an \
         implementation that violated every assertion unconditionally would \
         produce the golden's CANCELLED for all three of these:\n  {}",
        wrong.join("\n  ")
    );
}

/// W9b Task 6 fix round 1 (M1): `driver::registry` installs the
/// namespace-file connect options for every pool but `unconnected_pool()`'s,
/// and recognises that one through the same `UNCONNECTED_URL` the pool is
/// built from. Any other pool is not mistaken for it.
#[tokio::test]
async fn the_driver_recognises_only_its_unconnected_pool() {
    assert!(driver::is_unconnected(&driver::unconnected_pool()));
    for other in [
        "postgres://egret:egret@localhost:5439/egret_test",
        "postgres://nobody@127.0.0.1:1/other",
        "postgres://nobody@127.0.0.1:2/none",
    ] {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(other)
            .unwrap();
        assert!(!driver::is_unconnected(&pool), "{other}");
    }
}
