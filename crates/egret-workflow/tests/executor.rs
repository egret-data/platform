//! Differential oracle for `advance()`: drives egret's state machine over the
//! flows in `oracle-exec/corpus/` and asserts that the **normalised semantic
//! trace** it produces is the one a pinned Kestra produced
//! (`oracle-exec/golden.json`, written by `oracle-exec/normalise.py` from
//! `traces.json`; see `oracle-exec/README.md`).
//!
//! # This is a RATCHET, not a pass/fail
//!
//! `advance()` is a stub at W3a Task 2, so **every** comparable entry
//! mismatches and every one is listed in `oracle-exec/baseline.txt`. A test
//! that is simply red for six tasks measures nothing — a NEW mismatch would be
//! invisible in the noise — so this fails in **both** directions:
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
//! # What the oracle FEEDS IN, and why that is not circular
//!
//! `advance()` does not decide whether a `Log` task succeeds — Kestra's
//! executor does not either; its worker does. So [`simulate`] supplies exactly
//! one thing from the golden: **the terminal state of a run whose `PlanTask`
//! is `Runnable`**. Everything else — which runs are created, in what order,
//! under which parent, with what history, and what the execution's own state
//! does — is egret's, and is what is compared.
//!
//! Feeding a *flowable's* state would be circular, and is not done: a
//! `Sequential`, `Parallel`, `Dag`, `If`, `Switch` or `Loop` run's state is the
//! executor's decision and is exactly what this oracle exists to judge.
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
    advance, apply, build_plan, ExecutionState, Plan, RunState, TaskKind, Tick,
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

/// How many `advance()` calls one execution may take before the simulation is
/// declared broken.
///
/// Derived, not picked. Every `Command` writes at most one history entry
/// (`CreateRun` adds a run whose history is `[Created]`; `SetRunState` and
/// `SetExecutionState` push one; `StartRun` pushes one plus one attempt entry),
/// and a call that emits no command ends the loop — so the number of calls is
/// bounded by the number of history entries the finished trace holds. The
/// widest golden entry holds **765** of them across its execution, its runs,
/// their attempts and its sub-executions — `valids/loop-nested`, whose 39
/// sub-executions are most of that. Measured over all of `golden.json`. The
/// factor of 4 is headroom for commands that write nothing (`ScheduleRetry`)
/// and for an implementation that is correct but takes more rounds than
/// Kestra's did.
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

/// The terminal state the golden recorded for the run at `ix`, as something
/// `RunState` can hold.
///
/// The measured vocabulary is wider than `RunState` (see its doc): anything
/// egret cannot represent is fed as `Failed`, which makes the entry mismatch —
/// visibly, in the report — rather than quietly turning into a `Success` that
/// happens to agree.
fn golden_outcome(golden: &Trace, ix: usize) -> RunState {
    match golden
        .runs
        .get(ix)
        .and_then(|r| r.state.last())
        .map(String::as_str)
    {
        Some("SUCCESS") => RunState::Success,
        Some("KILLED") => RunState::Killed,
        _ => RunState::Failed,
    }
}

/// Run `advance()` to a fixed point and build the trace it produced.
///
/// See this module's doc for the one thing this feeds in from the golden and
/// why that is not circular.
fn simulate(plan: &Plan, golden: &Trace) -> Result<Trace, String> {
    let mut state = ExecutionState::new(plan);
    // The step at which each run started and terminated. Integers, not clocks:
    // the happens-before relation is a claim about SERIALISATION, and a pure
    // state machine serialises in `advance()` rounds. Same predicate as
    // `normalise.py`'s, over step numbers instead of timestamps.
    let mut started: Vec<Option<usize>> = Vec::new();
    let mut ended: Vec<Option<usize>> = Vec::new();

    let bound = step_bound();
    let mut step = 0usize;
    loop {
        if step > bound {
            return Err(format!(
                "did not settle within {bound} advance() calls \
                 (4 x the widest golden trace's {MEASURED_MAX_HISTORY} history entries)"
            ));
        }

        // The environment: a runnable task the worker picked up finishes.
        for ix in 0..state.runs.len() {
            let r = &state.runs[ix];
            let is_runnable = matches!(
                plan.tasks.get(r.task).map(|t| &t.kind),
                Some(TaskKind::Runnable { .. })
            );
            if is_runnable && r.history.last() == Some(&RunState::Running) {
                let outcome = golden_outcome(golden, ix);
                apply(
                    &mut state,
                    &egret_workflow::executor::Command::SetRunState {
                        run: ix,
                        state: outcome,
                    },
                );
                ended[ix] = Some(step);
            }
        }

        let cmds = advance(plan, &state, Tick(0));
        if cmds.is_empty() {
            break;
        }
        for cmd in &cmds {
            use egret_workflow::executor::Command::*;
            match cmd {
                CreateRun { .. } => {
                    started.push(None);
                    ended.push(None);
                }
                StartRun { run } => started[*run] = Some(step),
                SetRunState { run, state: s } if s.is_terminal() => ended[*run] = Some(step),
                _ => {}
            }
            apply(&mut state, cmd);
        }
        step += 1;
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
            if let Some(rel) = step_relation(started[i], ended[i], started[j], ended[j]) {
                order.push((i, j, rel.to_string()));
            }
        }
    }

    Ok(Trace {
        parent_run: None,
        value: None,
        execution: state.history.iter().map(|s| s.wire().to_string()).collect(),
        runs,
        // Task 6 wires `Command::SpawnExecution` through; until then egret
        // spawns nothing and every entry with a sub-execution mismatches here.
        // `oracle-exec/README.md` records that hole with its count.
        order,
        spawned: Vec::new(),
    })
}

/// `normalise.py`'s predicate, over step numbers.
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
            (Verdict::Runs, Outcome::Trace(actual)) => {
                let expected = Trace::from(entry);
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

    let stale: Vec<&String> = baseline
        .iter()
        .filter(|id| !golden.iter().any(|e| &e.id == *id))
        .collect();
    let regressions: Vec<&String> = mismatched
        .iter()
        .filter(|id| !baseline.contains(*id))
        .collect();
    let banked: Vec<&String> = baseline
        .iter()
        .filter(|id| golden.iter().any(|e| &e.id == *id) && !mismatched.contains(*id))
        .collect();

    println!(
        "oracle-exec ratchet: {} golden entries, {} mismatching, {} baselined, {} broken",
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
