//! Differential oracle: renders `oracle/corpus.json`'s expressions through
//! Kestra's real Pebble engine (committed as `oracle/golden.json` by the Java
//! harness in `oracle/`, see `oracle/README.md`) and asserts that egret's own
//! renderer agrees.
//!
//! # This is a RATCHET, not a pass/fail
//!
//! 84 of 122 entries do not match yet, because Tasks 4-8 have not ported the
//! filters and functions. A test that is simply red for five tasks measures
//! nothing: a NEW mismatch would be invisible in the noise. So the currently
//! mismatching ids are committed in `oracle/baseline.txt`, and this test fails
//! in **both** directions:
//!
//! - an id that is **not** in the baseline mismatches — a regression;
//! - an id that **is** in the baseline now matches — progress that has to be
//!   banked by deleting that line, in the same commit that earned it.
//!
//! The second direction is what makes it a ratchet rather than a suppression
//! list. A task cannot make progress without shrinking the file, and cannot
//! claim progress it did not make. The current mismatch count is printed on
//! every run, pass or fail, so the number never stops being visible.
//!
//! # Three comparison modes, all of them named in the output
//!
//! | mode | applies to | rule |
//! |---|---|---|
//! | exact | everything not below | byte equality. No trimming, no normalisation, no fuzzy matching. |
//! | shape | the eight ids in [`NON_DETERMINISTIC`] | length / range / non-empty. They produce a different value on every render, by construction. |
//! | error-class | entries the **corpus** marks `"compare": "error-class"` | egret must also fail. The message is not compared. |
//!
//! The error-class mode exists because a golden error value is a Java
//! exception string (`io.pebbletemplates.pebble.error.PebbleException: …`)
//! that no Rust port can ever reproduce byte for byte, so strict equality
//! would make zero unreachable rather than merely distant. Two properties
//! keep it from becoming a silent weakening: the mode is an explicit,
//! reviewable property **of the corpus entry**, never inferred at comparison
//! time from "the golden happens to look like an error"; and the ids are
//! printed with a count on every run. The Java message stays in the golden
//! file as documentation of what Kestra actually says.
//!
//! # How to read a failure
//!
//! Three kinds of line appear in a mismatch report, and they are not equally
//! interesting:
//!
//! - `unknown filter` / `unknown function` — not yet ported. Tasks 4-8.
//! - a value that differs in *substance* — a real semantic divergence. Those
//!   are the ones worth stopping for.
//! - `filter indent` / `range` complaining about an *argument* rather than
//!   being unknown — minijinja has its own filter/function of that name with a
//!   different signature. A port must override, not assume.

use std::collections::{BTreeMap, BTreeSet};

use egret_workflow::{render_str, RenderContext};
use serde::Deserialize;
use serde_json::Value;

/// The eight corpus entries no implementation — Kestra's own included — can
/// reproduce on a second run, and why. Committed as a list rather than
/// discovered as red output, so that a NINTH one showing up is visible as new
/// information about the corpus rather than lost in the noise.
///
/// Each is compared for **shape** (see [`shape_ok`]), never for value, and
/// each is named in the test's output as shape-checked. A silent pass here
/// would defeat the point of the whole oracle.
const NON_DETERMINISTIC: &[&str] = &[
    // Wall clock.
    "function-now",
    // `SecureRandom` / time-ordered UUIDv7.
    "function-uuid",
    // `IdUtils.create()`, backed by a random.
    "function-id",
    // KSUID: a timestamp prefix plus 16 random bytes.
    "function-ksuid",
    // NanoId: 21 random characters.
    "function-nanoid",
    // `randomInt(lower=1, upper=10)`.
    "function-randomint",
    // A free local port, whatever the OS hands out.
    "function-randomport",
    // A fresh random IV on every call, so the ciphertext differs each time.
    // `function-decrypt` is NOT here: its corpus entry hardcodes a ciphertext
    // generated once, precisely so it stays deterministic.
    "function-encrypt",
];

/// Mirrors the Java harness's golden-file schema (see `oracle/README.md`):
/// `{ "id", "expr", "context", "result": String | { "error": String } }`.
#[derive(Debug, Deserialize)]
struct GoldenEntry {
    id: String,
    expr: String,
    context: Value,
    result: GoldenResult,
    /// Copied through from `corpus.json` by the Java harness. Absent means
    /// [`CompareMode::Exact`] — the weaker comparison is opt-in, per entry,
    /// and visible in the corpus diff that introduces it.
    #[serde(default)]
    compare: CompareMode,
}

/// How one entry is compared. See this module's doc for why `ErrorClass`
/// exists and what stops it becoming a silent weakening.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CompareMode {
    #[default]
    Exact,
    ErrorClass,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GoldenResult {
    Ok(String),
    Err { error: String },
}

/// Renders one corpus entry through egret's renderer.
///
/// The whole template, not just an expression: `render_str` is
/// [`egret_workflow::runners::env::render_str`], the same function
/// `Property<T>::render` calls before converting. Errors are flattened to
/// their `Display` because that is the only shape a differential comparison
/// against a Java exception string can take.
fn render_via_egret(expr: &str, context: &Value) -> Result<String, String> {
    let cx = RenderContext::from_json_object(context)
        .unwrap_or_else(|| panic!("corpus context must be a JSON object, got: {context}"));
    render_str(expr, &cx).map_err(|e| e.to_string())
}

/// The shape check for [`NON_DETERMINISTIC`] entries: same kind of outcome as
/// the golden, and — for the ones with a pinnable shape — a plausible value.
///
/// Deliberately weak on purpose and deliberately NOT reused for anything else.
/// Today every one of these renders as an unknown-function error, since none
/// of the eight is ported yet; the shapes below are what Task 7 has to satisfy.
fn shape_ok(id: &str, golden: &GoldenResult, actual: &Result<String, String>) -> bool {
    // Both must at least agree on succeeded-vs-failed.
    let value = match (golden, actual) {
        (GoldenResult::Ok(_), Ok(v)) => v,
        (GoldenResult::Err { .. }, Err(_)) => return true,
        _ => return false,
    };

    match id {
        "function-randomint" => value.parse::<i64>().is_ok_and(|n| (1..10).contains(&n)),
        "function-randomport" => value.parse::<u16>().is_ok_and(|p| p > 0),
        "function-uuid" => value.len() == 36 && value.matches('-').count() == 4,
        "function-ksuid" => value.len() == 27,
        "function-nanoid" => value.len() == 21,
        "function-id" => value.len() == 22,
        // A timestamp and a base64 blob: pin only that they are non-empty, so
        // this never turns into a second, weaker copy of the real comparison.
        "function-now" | "function-encrypt" => !value.is_empty(),
        other => unreachable!("{other} is on NON_DETERMINISTIC but has no shape check"),
    }
}

/// The baseline: the ids that do not match yet.
///
/// One id per line; `#` starts a comment and blank lines are ignored. Deleting
/// a line is how a task banks progress, and the test refuses to pass if a line
/// is still there for an entry that now matches — see this module's doc.
fn baseline() -> BTreeSet<String> {
    include_str!("../oracle/baseline.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The ratchet. See this module's doc for the two directions it fails in.
#[test]
fn render_corpus_matches_golden() {
    let golden_json = include_str!("../oracle/golden.json");
    let golden: Vec<GoldenEntry> = serde_json::from_str(golden_json)
        .expect("oracle/golden.json must parse as a JSON array of golden entries");
    assert!(!golden.is_empty(), "oracle/golden.json must not be empty");

    let ids: BTreeSet<&str> = golden.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        golden.len(),
        "oracle/golden.json has duplicate ids"
    );

    // A stale name on the exclusion list would silently stop excluding
    // anything, and the entry it was meant to cover would read as a real
    // mismatch. Catch that here rather than in a confused reading of output.
    for id in NON_DETERMINISTIC {
        assert!(
            ids.contains(id),
            "NON_DETERMINISTIC names {id}, which is not in oracle/golden.json"
        );
    }

    let mut mismatching = BTreeSet::new();
    let mut detail: BTreeMap<String, String> = BTreeMap::new();
    let mut shape_checked = Vec::new();
    let mut error_class = Vec::new();

    for entry in &golden {
        let actual = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_via_egret(&entry.expr, &entry.context)
        }));

        let actual_result: Result<String, String> = match actual {
            Ok(inner) => inner,
            Err(panic) => Err(panic_message(panic)),
        };

        let non_deterministic = NON_DETERMINISTIC.contains(&entry.id.as_str());

        // An entry marked error-class whose golden is NOT an error would apply
        // the weaker comparison to a value comparison, which is exactly the
        // silent-weakening failure the mode is fenced against. That is a
        // corpus bug, so it is an assertion, not a mismatch.
        if entry.compare == CompareMode::ErrorClass {
            assert!(
                matches!(entry.result, GoldenResult::Err { .. }),
                "{} is marked \"compare\": \"error-class\" but its golden value is not an error",
                entry.id
            );
        }

        let matches = if non_deterministic {
            shape_checked.push(entry.id.clone());
            shape_ok(&entry.id, &entry.result, &actual_result)
        } else if entry.compare == CompareMode::ErrorClass {
            error_class.push(entry.id.clone());
            actual_result.is_err()
        } else {
            match (&entry.result, &actual_result) {
                (GoldenResult::Ok(expected), Ok(actual)) => expected == actual,
                (GoldenResult::Err { error: expected }, Err(actual)) => expected == actual,
                _ => false,
            }
        };

        if !matches {
            let expected_display = match &entry.result {
                GoldenResult::Ok(s) => format!("Ok({s:?})"),
                GoldenResult::Err { error } => format!("Err({error:?})"),
            };
            let actual_display = match &actual_result {
                Ok(s) => format!("Ok({s:?})"),
                Err(e) => format!("Err({e:?})"),
            };
            let note = match (non_deterministic, entry.compare) {
                (true, _) => " [SHAPE-CHECKED, not compared by value]",
                (false, CompareMode::ErrorClass) => " [ERROR-CLASS: egret must also fail]",
                (false, CompareMode::Exact) => "",
            };
            mismatching.insert(entry.id.clone());
            detail.insert(
                entry.id.clone(),
                format!(
                    "  {id}{note}: expr={expr:?} context={context}\n    expected: {expected_display}\n    actual:   {actual_display}",
                    id = entry.id,
                    expr = entry.expr,
                    context = entry.context,
                ),
            );
        }
    }

    // Printed unconditionally, pass or fail. The count is the number Tasks 4-8
    // drive to zero; the two exclusion lists have to stay visible or "the
    // oracle is green" stops meaning what it looks like it means.
    println!(
        "oracle: {} of {} entries mismatch (baseline: {})",
        mismatching.len(),
        golden.len(),
        baseline().len()
    );
    println!(
        "  {} compared for SHAPE, not value (non-deterministic): {}",
        shape_checked.len(),
        shape_checked.join(", ")
    );
    println!(
        "  {} compared for ERROR CLASS, not message (corpus-marked): {}",
        error_class.len(),
        error_class.join(", ")
    );

    let baseline = baseline();

    let stale: Vec<&String> = baseline
        .iter()
        .filter(|id| !ids.contains(id.as_str()))
        .collect();
    let regressions: Vec<&String> = mismatching.difference(&baseline).collect();
    let banked: Vec<&String> = baseline.difference(&mismatching).collect();

    let mut failures = String::new();
    if !stale.is_empty() {
        failures.push_str(&format!(
            "\noracle/baseline.txt names {} id(s) that are not in golden.json at all -- \
             delete them: {stale:?}\n",
            stale.len()
        ));
    }
    if !regressions.is_empty() {
        failures.push_str(&format!(
            "\nREGRESSION: {} entr(y/ies) mismatch that were NOT in oracle/baseline.txt. \
             Fix them, do not add them to the baseline:\n{}\n",
            regressions.len(),
            regressions
                .iter()
                .map(|id| detail[*id].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !banked.is_empty() {
        failures.push_str(&format!(
            "\nPROGRESS NOT BANKED: {} entr(y/ies) in oracle/baseline.txt now MATCH. \
             Delete these lines from oracle/baseline.txt in the same commit that fixed \
             them:\n  {}\n",
            banked.len(),
            banked
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ));
    }

    assert!(failures.is_empty(), "{failures}");
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "renderer panicked with a non-string payload".to_string()
    }
}
