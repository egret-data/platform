//! Differential oracle: renders `oracle/corpus.json`'s expressions through
//! Kestra's real Pebble engine (committed as `oracle/golden.json` by the Java
//! harness in `oracle/`, see `oracle/README.md`) and, once egret has a
//! renderer, asserts the two agree.
//!
//! Task 1 (this file's first commit) wires the golden-file parsing and the
//! mismatch-reporting harness but leaves the actual comparison `#[ignore]`d:
//! egret-workflow has no renderer yet (`Property<T>::render` and
//! `environment()` land in Task 3 — see
//! docs/en/plans/2026-09-02-workflow-w2-expressions.md, Task 3 Step 3, which
//! removes the `#[ignore]`). Confirmed by hand before writing this file: a
//! version of this test that called a real (not-yet-existing) egret_workflow
//! render path failed with `error[E0433]: cannot find `runners` in
//! `egret_workflow`` under `cargo check -p egret-workflow --test oracle` — the
//! expected failure this task's brief calls for. This committed version does
//! not repeat that reference (so the crate keeps compiling and the suite
//! stays green per this task's gate); `render_via_egret` below is the single
//! seam Task 3 rewires.

use serde::Deserialize;
use serde_json::Value;

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

/// The seam Task 3 rewires: replace this body with a real call into
/// egret-workflow's renderer (`Property<T>::render` over an `environment()`
/// built from `context`) and remove the `#[ignore]` on
/// `render_corpus_matches_golden` below. Left as `unimplemented!()` rather
/// than a faked pass/fail so this can never be mistaken for a working port —
/// see task-1-brief.md: "Do not stub a renderer to make this pass."
fn render_via_egret(expr: &str, context: &Value) -> Result<String, String> {
    let _ = (expr, context);
    unimplemented!("Task 3 wires egret-workflow's renderer in here (crates/egret-workflow/src/models/property.rs Property::render)")
}

#[test]
#[ignore = "no renderer until Task 3 (crates/egret-workflow/src/models/property.rs Property::render + runners::environment()); see docs/en/plans/2026-09-02-workflow-w2-expressions.md Task 3 Step 3, which removes this ignore"]
fn render_corpus_matches_golden() {
    let golden_json = include_str!("../oracle/golden.json");
    let golden: Vec<GoldenEntry> = serde_json::from_str(golden_json)
        .expect("oracle/golden.json must parse as a JSON array of golden entries");
    assert!(!golden.is_empty(), "oracle/golden.json must not be empty");

    let mut mismatches = Vec::new();
    for entry in &golden {
        let actual = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_via_egret(&entry.expr, &entry.context)
        }));

        let actual_result: Result<String, String> = match actual {
            Ok(inner) => inner,
            Err(panic) => Err(panic_message(panic)),
        };

        let matches = match (&entry.result, &actual_result) {
            (GoldenResult::Ok(expected), Ok(actual)) => expected == actual,
            (GoldenResult::Err { error: expected }, Err(actual)) => expected == actual,
            _ => false,
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
            mismatches.push(format!(
                "  {id}: expr={expr:?} context={context}\n    expected: {expected_display}\n    actual:   {actual_display}",
                id = entry.id,
                expr = entry.expr,
                context = entry.context,
            ));
        }
    }

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
