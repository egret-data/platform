//! Differential oracle: renders `oracle/corpus.json`'s expressions through
//! Kestra's real Pebble engine (committed as `oracle/golden.json` by the Java
//! harness in `oracle/`, see `oracle/README.md`) and asserts that egret's own
//! renderer agrees, byte for byte.
//!
//! Task 1 built the harness and the golden file and left the comparison
//! `#[ignore]`d, because egret had no renderer. Task 3 built the renderer
//! (`egret_workflow::render_str`, `Property<T>::render`) and removed the
//! ignore. **This test is expected to FAIL until Tasks 4-8 land the 35 filters
//! and 34 functions.** Its failure message is those tasks' worklist: every
//! mismatching id, with the golden value beside egret's.
//!
//! # How to read a failure
//!
//! The message groups nothing and sorts nothing — it is corpus order, which is
//! operators, then filters, then functions, then tests, then Task 3's own
//! additions. Three kinds of line appear, and they are not equally
//! interesting:
//!
//! - `unknown filter` / `unknown function` — not yet ported. Tasks 4-8.
//! - a value that differs in *formatting* only (`True` vs `true`, `[1, 2]` vs
//!   `[1,2]`) — a render-layer difference, not a missing filter. See
//!   `task-3-report.md`.
//! - a value that differs in *substance* — a real semantic divergence. Those
//!   are the ones worth stopping for.
//!
//! # What is NOT compared
//!
//! [`NON_DETERMINISTIC`] names the eight entries that produce a different
//! value on every render, by construction. They are never compared for value.
//! Everything else is compared for exact byte equality: no trimming, no
//! normalisation, no fuzzy matching.

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

/// Every golden entry, compared byte for byte against egret's renderer.
///
/// Driven entirely by the file's contents — there is no expected entry count
/// here, because the corpus grows as filters and functions get ported and a
/// hardcoded count would just be a second thing to update.
#[test]
fn render_corpus_matches_golden() {
    let golden_json = include_str!("../oracle/golden.json");
    let golden: Vec<GoldenEntry> = serde_json::from_str(golden_json)
        .expect("oracle/golden.json must parse as a JSON array of golden entries");
    assert!(!golden.is_empty(), "oracle/golden.json must not be empty");

    // A stale name on the exclusion list would silently stop excluding
    // anything, and the entry it was meant to cover would read as a real
    // mismatch. Catch that here rather than in a confused reading of the
    // output.
    for id in NON_DETERMINISTIC {
        assert!(
            golden.iter().any(|e| e.id == *id),
            "NON_DETERMINISTIC names {id}, which is not in oracle/golden.json"
        );
    }

    let mut mismatches = Vec::new();
    let mut shape_checked = Vec::new();

    for entry in &golden {
        let actual = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_via_egret(&entry.expr, &entry.context)
        }));

        let actual_result: Result<String, String> = match actual {
            Ok(inner) => inner,
            Err(panic) => Err(panic_message(panic)),
        };

        let non_deterministic = NON_DETERMINISTIC.contains(&entry.id.as_str());
        let matches = if non_deterministic {
            shape_checked.push(entry.id.clone());
            shape_ok(&entry.id, &entry.result, &actual_result)
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
            let note = if non_deterministic {
                " [SHAPE-CHECKED, not compared by value]"
            } else {
                ""
            };
            mismatches.push(format!(
                "  {id}{note}: expr={expr:?} context={context}\n    expected: {expected_display}\n    actual:   {actual_display}",
                id = entry.id,
                expr = entry.expr,
                context = entry.context,
            ));
        }
    }

    // Printed unconditionally, pass or fail: the exclusions have to be visible
    // in the output, or "the oracle is green" stops meaning what it looks like
    // it means.
    println!(
        "{} of {} entries are non-deterministic and were compared for SHAPE, not value: {}",
        shape_checked.len(),
        golden.len(),
        shape_checked.join(", ")
    );

    assert!(
        mismatches.is_empty(),
        "{} of {} golden entries did not match egret's renderer:\n{}",
        mismatches.len(),
        golden.len(),
        mismatches.join("\n")
    );
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
