//! Differential oracle: renders `oracle/corpus.json`'s expressions through
//! Kestra's real Pebble engine (committed as `oracle/golden.json` by the Java
//! harness in `oracle/`, see `oracle/README.md`) and asserts that egret's own
//! renderer agrees.
//!
//! # This is a RATCHET, not a pass/fail
//!
//! Most entries do not match yet, because Tasks 4-8 have not ported the
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
//! **A renderer PANIC is a mismatch in every one of those modes, and it fails
//! the test unconditionally** — the baseline does not cover it. A baselined id
//! means "egret does not reproduce this value yet"; it never means "egret may
//! crash here". A panic means the renderer could not run at all, and an
//! instrument that reports success for a renderer that cannot run is the one
//! failure this whole package exists to prevent. Proven, not asserted — see
//! `a_panicking_renderer_is_a_mismatch_in_every_mode`, and the two probes in
//! `task-3-report.md`'s fix-round-2 section.
//!
//! # `corpus.json` and `golden.json` must describe the same expressions
//!
//! Only `golden.json` is rendered here, so a corpus entry added without
//! regenerating would be **silently never tested**, and an edited `expr` would
//! keep being tested against the old expression while staying green. That is
//! the one failure mode the ratchet cannot see, and it points straight at
//! Tasks 4-6, which are told to add corpus entries first and regenerate
//! second. `corpus_and_golden_describe_the_same_expressions` binds the two.
//!
//! The error-class mode exists because a golden error value is a Java
//! exception string (`io.pebbletemplates.pebble.error.PebbleException: …`)
//! that no Rust port can ever reproduce byte for byte, so strict equality
//! would make zero unreachable rather than merely distant. **Three** properties
//! keep it from becoming a silent weakening: the mode is an explicit,
//! reviewable property **of the corpus entry**, never inferred at comparison
//! time from "the golden happens to look like an error"; the ids are printed
//! with a count on every run; and every one of them is **paired**, in
//! `oracle/error-class-pairs.txt`, with something that CAN fail — see
//! [`every_error_class_entry_is_paired_with_something_that_can_fail`]. The
//! Java message stays in the golden file as documentation of what Kestra
//! actually says.
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
    /// Parsed straight into [`RenderContext`], never through
    /// `serde_json::Value`: serde visits map entries in document order and a
    /// `serde_json::Value` would be a second chance to lose it. Its `Display`
    /// writes the bindings back as ordered JSON for the mismatch report.
    context: RenderContext,
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
fn render_via_egret(expr: &str, cx: &RenderContext) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_str(expr, cx))) {
        Ok(Ok(rendered)) => Outcome::Ok(rendered),
        Ok(Err(e)) => Outcome::Err(e.to_string()),
        Err(panic) => Outcome::Panic(panic_message(panic)),
    }
}

/// What egret's renderer did — with a **panic as its own case**.
///
/// Folding a panic into [`Outcome::Err`] is what made the oracle report
/// success for a renderer that could not run: an error-class entry compares
/// `is_err()`, and a shape-checked entry's `(Err, Err)` arm returned `true`
/// unconditionally, so a `render_str` that panicked on the eight
/// service-backed entries turned the whole suite green. A panic is a
/// mismatch everywhere, and it says so in the report.
#[derive(Debug)]
enum Outcome {
    Ok(String),
    Err(String),
    Panic(String),
}

impl Outcome {
    fn display(&self) -> String {
        match self {
            Outcome::Ok(s) => format!("Ok({s:?})"),
            Outcome::Err(e) => format!("Err({e:?})"),
            Outcome::Panic(p) => format!("PANIC({p:?})"),
        }
    }
}

/// The shape check for [`NON_DETERMINISTIC`] entries: same kind of outcome as
/// the golden, and — for the ones with a pinnable shape — a plausible value.
///
/// Deliberately weak on purpose and deliberately NOT reused for anything else.
/// Today every one of these renders as an unknown-function error, since none
/// of the eight is ported yet; the shapes below are what Task 7 has to satisfy.
fn shape_ok(id: &str, golden: &GoldenResult, actual: &Outcome) -> bool {
    // Both must at least agree on succeeded-vs-failed -- and a PANIC agrees
    // with nothing. It used to fall into the `(Err, Err)` arm below and pass.
    let value = match (golden, actual) {
        (_, Outcome::Panic(_)) => return false,
        (GoldenResult::Ok(_), Outcome::Ok(v)) => v,
        (GoldenResult::Err { .. }, Outcome::Err(_)) => return true,
        _ => return false,
    };

    match id {
        "function-randomint" => value.parse::<i64>().is_ok_and(|n| (1..10).contains(&n)),
        "function-randomport" => value.parse::<u16>().is_ok_and(|p| p > 0),
        "function-uuid" => value.len() == 36 && value.matches('-').count() == 4,
        "function-ksuid" => value.len() == 27,
        "function-nanoid" => value.len() == 21,
        // **Not `== 22`, and this is measured rather than relaxed.** `id()` is
        // `FriendlyId`, which base-62-encodes a random UUIDv4 with **no
        // padding**, so the digit count is `ceil(log62(value))`: `62^21` is
        // 4.37e37 against a 2^128 = 3.40e38 space, so 12.8% of ids are 21
        // characters and 0.2% are 20. The golden file's own earlier value was
        // `iUBruItK7qj9tMOMNVPAr` -- 21 characters, minted by Kestra -- so
        // `== 22` would have failed about one run in eight against a faithful
        // port. What is actually guaranteed is the alphabet and the ceiling.
        "function-id" => {
            !value.is_empty()
                && value.len() <= 22
                && value.bytes().all(|b| b.is_ascii_alphanumeric())
        }
        // A timestamp: pin only that it is non-empty, so this never turns
        // into a second, weaker copy of the real comparison.
        "function-now" => !value.is_empty(),
        // **`!value.is_empty()` was what this used to be, and it could not see
        // the format at all.** A shape check sees a length, and the length is
        // exactly what carries the format here: 12 IV bytes + 12 ciphertext
        // bytes for `'hello-secret'` + a 16-byte GCM tag is 40 bytes, which is
        // 56 base64 characters. A port with an 8-byte IV, or a 96-bit tag, or
        // no IV at all, satisfied "non-empty" and fails this.
        //
        // What a shape check still cannot see is that the ciphertext is
        // *right*. `function-encrypt-decrypt-round-trip` is what pins that,
        // and it is an ordinary exact-compared entry precisely because the
        // round trip is deterministic where the ciphertext is not.
        "function-encrypt" => {
            value.len() == 56
                && value.ends_with("==")
                && value[..54]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        }
        other => unreachable!("{other} is on NON_DETERMINISTIC but has no shape check"),
    }
}

/// Does egret's outcome match this entry's golden, under the entry's mode?
///
/// Factored out of the differential test so the modes can be exercised
/// directly — in particular so that "a panic is never a match" is a claim with
/// a test behind it rather than a comment.
fn entry_matches(entry: &GoldenEntry, actual: &Outcome) -> bool {
    // Checked FIRST, before any mode is consulted, so no mode can be taught to
    // forgive one. A panic means the renderer could not run; an instrument
    // that reports success for that is worse than no instrument.
    if matches!(actual, Outcome::Panic(_)) {
        return false;
    }

    if NON_DETERMINISTIC.contains(&entry.id.as_str()) {
        return shape_ok(&entry.id, &entry.result, actual);
    }

    if entry.compare == CompareMode::ErrorClass {
        return matches!(actual, Outcome::Err(_));
    }

    match (&entry.result, actual) {
        (GoldenResult::Ok(expected), Outcome::Ok(actual)) => expected == actual,
        (GoldenResult::Err { error: expected }, Outcome::Err(actual)) => expected == actual,
        _ => false,
    }
}

/// **C1.** A renderer panic must be a mismatch in **every** mode.
///
/// This is not hypothetical. Before the [`Outcome`] type existed, a panic was
/// flattened into `Err`, and two paths then accepted it: the error-class mode
/// compares `is_err()`, and [`shape_ok`]'s `(Err, Err)` arm returned `true`
/// unconditionally. A `render_str` that panicked on the eight service-backed
/// entries turned the whole oracle **green**, with the printed counts
/// unchanged — the instrument reporting success for a renderer that could not
/// run at all.
///
/// Each mode below is checked with a golden this renderer would otherwise have
/// satisfied, so the assertion fails if any mode is ever taught to forgive a
/// panic again.
#[test]
fn a_panicking_renderer_is_a_mismatch_in_every_mode() {
    let golden: Vec<GoldenEntry> = serde_json::from_str(include_str!("../oracle/golden.json"))
        .expect("oracle/golden.json must parse");
    let by_id = |id: &str| {
        golden
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("{id} must be in oracle/golden.json"))
    };

    let panic = Outcome::Panic("renderer exploded".to_string());

    // Error-class: the mode that accepts any failure. `function-secret`'s
    // golden is a PebbleException and egret's real `unknown function` error
    // matches it, so this entry is the one a panic could hide behind.
    let error_class = by_id("function-secret");
    assert_eq!(error_class.compare, CompareMode::ErrorClass);
    assert!(entry_matches(
        error_class,
        &Outcome::Err("unknown function".into())
    ));
    assert!(
        !entry_matches(error_class, &panic),
        "a panic must not satisfy an error-class entry"
    );

    // Shape: `shape_ok`'s (Err, Err) arm used to return true unconditionally.
    let shaped = by_id("function-uuid");
    assert!(NON_DETERMINISTIC.contains(&shaped.id.as_str()));
    assert!(
        !entry_matches(shaped, &panic),
        "a panic must not satisfy a shape-checked entry"
    );

    // Value: never accepted a panic, pinned so it stays that way.
    let exact = by_id("ternary");
    assert_eq!(exact.compare, CompareMode::Exact);
    assert!(
        !entry_matches(exact, &panic),
        "a panic must not satisfy an exact-comparison entry"
    );
}

/// `corpus.json` and `golden.json` must describe the same expressions, in the
/// same order.
///
/// Nothing else in this file reads `corpus.json` — the differential test
/// renders `golden.json` — so without this a corpus entry added and never
/// regenerated is **silently never tested**, and an edited `expr` keeps being
/// checked against the old expression while the suite stays green. Neither
/// the ratchet nor the mismatch count can see that: both are computed from
/// `golden.json` alone.
///
/// This matters most for Tasks 4-6, whose brief tells them to add corpus
/// entries FIRST and regenerate second. Forgetting the second step is the
/// natural mistake, and it produces a green run.
///
/// The four fields compared are everything the harness copies through and the
/// comparison depends on: `id`, `expr`, `context`, `compare`. The `result` is
/// the harness's output and is deliberately not compared to anything.
#[test]
fn corpus_and_golden_describe_the_same_expressions() {
    #[derive(Debug, Deserialize)]
    struct Described {
        id: String,
        expr: String,
        /// **Not `serde_json::Map`.** `serde_json`'s `preserve_order` is on
        /// workspace-wide (transitively, via `utoipa` and
        /// `cedar-policy-core`), which makes that type an `IndexMap` — and
        /// **`IndexMap`'s `PartialEq` is order-independent**. Comparing
        /// contexts that way let a corpus entry's keys be REORDERED with no
        /// regeneration and every test still pass, on `map-key-order`, whose
        /// entire purpose is key order.
        ///
        /// [`RenderContext`] keeps document order and its `Display` writes it
        /// back as ordered JSON, so comparing those strings is order-sensitive
        /// by construction rather than by a property of whichever map type a
        /// dependency's feature flags happen to select.
        #[serde(default)]
        context: RenderContext,
        #[serde(default)]
        compare: CompareMode,
    }

    impl Described {
        /// Everything the harness copies through and the comparison depends
        /// on, as one comparable, order-sensitive value.
        fn comparable(&self) -> (&str, &str, String, CompareMode) {
            (&self.id, &self.expr, self.context.to_string(), self.compare)
        }
    }

    let corpus: Vec<Described> = serde_json::from_str(include_str!("../oracle/corpus.json"))
        .expect("oracle/corpus.json must parse as a JSON array");
    let golden: Vec<Described> = serde_json::from_str(include_str!("../oracle/golden.json"))
        .expect("oracle/golden.json must parse as a JSON array");

    assert_eq!(
        corpus.len(),
        golden.len(),
        "oracle/corpus.json has {} entries and oracle/golden.json has {} -- regenerate the \
         golden file (see oracle/README.md)",
        corpus.len(),
        golden.len()
    );

    let mut drifted = Vec::new();
    for (c, g) in corpus.iter().zip(golden.iter()) {
        if c.comparable() != g.comparable() {
            drifted.push(format!(
                "  corpus: {:?}\n  golden: {:?}",
                c.comparable(),
                g.comparable()
            ));
        }
    }
    assert!(
        drifted.is_empty(),
        "{} corpus entr(y/ies) do not match oracle/golden.json -- regenerate the golden file \
         (see oracle/README.md), and review the diff:\n{}",
        drifted.len(),
        drifted.join("\n")
    );
}

/// The committed set of ids allowed to be compared by error class.
///
/// Same shape as [`baseline`]: one id per line, `#` comments, blank lines
/// ignored.
fn error_class_allowlist() -> BTreeSet<String> {
    include_str!("../oracle/error-class.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// `compare: "error-class"` is a **free pass**, so adding one must be a
/// deliberate, reviewable act rather than a field edit.
///
/// An error-class entry matches on any error at all — including egret's
/// `unknown function` against Kestra's `PebbleException` — so it is born
/// matching, never enters `oracle/baseline.txt`, and can never fail. Eleven
/// entries match that way today and eight of them only because nothing is
/// ported. A task that could not reproduce a filter could mark its entry
/// error-class and turn the oracle green on work it did not do; nothing in
/// the ratchet would notice, because the entry never reaches the ratchet.
///
/// `oracle/error-class.txt` is that set, committed. A new error-class entry is
/// now a one-line diff in a 16-line file next to `baseline.txt`, instead of
/// one field among 138 JSON objects — the same discipline the ratchet already
/// applies to mismatches.
#[test]
fn error_class_entries_are_exactly_the_committed_allowlist() {
    let golden: Vec<GoldenEntry> = serde_json::from_str(include_str!("../oracle/golden.json"))
        .expect("oracle/golden.json must parse");

    let marked: BTreeSet<String> = golden
        .iter()
        .filter(|e| e.compare == CompareMode::ErrorClass)
        .map(|e| e.id.clone())
        .collect();
    let allowed = error_class_allowlist();

    let added: Vec<&String> = marked.difference(&allowed).collect();
    let removed: Vec<&String> = allowed.difference(&marked).collect();

    assert!(
        added.is_empty(),
        "{} entr(y/ies) are marked \"compare\": \"error-class\" but are not in \
         oracle/error-class.txt. Error-class comparison forgives ANY failure, so it can hide \
         unported work -- add the id there deliberately, or use exact comparison: {added:?}",
        added.len()
    );
    assert!(
        removed.is_empty(),
        "oracle/error-class.txt names {} id(s) that are no longer marked error-class in the \
         corpus -- delete them: {removed:?}",
        removed.len()
    );
}

/// The error-class ids with **nothing ported behind them** — now none.
///
/// It held eight through Tasks 4-7: `secret`, `kv`, `read`, `fileURI`,
/// `fileSize`, `fileExists`, `isFileEmpty` and `http`, each matching only
/// because egret said `unknown function: secret is unknown` where Kestra says
/// `Function or Macro [secret] does not exist` — the hollow pass the whole
/// error-class fence exists to make visible. **The two are indistinguishable
/// to this file**, by construction, which is why the debt had to be declared
/// here rather than measured.
///
/// W2 Task 8 emptied it: all eight now refuse with a message that names the
/// function and the package that will implement it, asserted by
/// `runners::functions::the_twelve_refusals_name_themselves_and_their_owner`,
/// and their pairings moved from `unported:task-8` to that test in the same
/// commit. (W9a Task 4 renamed it `the_eight_refusals_…` and moved the four
/// file functions' pairings to
/// `without_a_scope_the_four_are_refused_by_name_saying_they_need_an_execution`;
/// W9b Task 4 renamed it `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven`
/// and moved `fileURI`'s pairing to the four's test; W9c Task 4 renamed it
/// `the_refused_list_is_six_and_kv_is_not_in_it` and moved `kv`'s pairing to
/// `kv_without_a_scope_refuses_by_name`; package #5b Task B8 renamed it
/// `the_refused_list_is_three_and_subflow_names_row_12` once B6 and B7 had
/// left three.)
///
/// **Leave it empty.** The constant stays because it is what stops a new
/// `unported:` being added to `oracle/error-class-pairs.txt` quietly: growing
/// the list means editing a test, not a data file.
const UNPORTED_ERROR_CLASS: &[&str] = &[];

/// Every `#[test] fn` name declared in the crate's own sources.
///
/// Read at compile time with `include_str!`, the same way the baseline and
/// the allowlist are, so the check is against the committed text rather than
/// against a runtime registry libtest does not expose to another binary.
fn declared_test_names() -> BTreeSet<String> {
    // **Every module in `src/runners/`.** The list used to be three files
    // while `oracle/README.md` said "in `src/runners/`", so a pairing naming
    // a `javafmt` test would have been rejected as nonexistent while
    // existing. `every_runners_module_is_scanned_for_declared_tests` is what
    // keeps this list and the directory the same.
    const SOURCES: &[&str] = &[
        include_str!("../src/runners/arith.rs"),
        include_str!("../src/runners/cmp_rewrite.rs"),
        include_str!("../src/runners/compare.rs"),
        include_str!("../src/runners/decimalformat.rs"),
        include_str!("../src/runners/env.rs"),
        include_str!("../src/runners/files.rs"),
        include_str!("../src/runners/filters.rs"),
        include_str!("../src/runners/functions.rs"),
        include_str!("../src/runners/ionfmt.rs"),
        include_str!("../src/runners/javafmt.rs"),
        include_str!("../src/runners/javaformat.rs"),
        include_str!("../src/runners/javare.rs"),
        include_str!("../src/runners/jqsandbox.rs"),
        include_str!("../src/runners/nested.rs"),
        include_str!("../src/runners/error_logs.rs"),
        include_str!("../src/runners/operators.rs"),
        include_str!("../src/runners/pebble_tests.rs"),
        include_str!("../src/runners/rewrite.rs"),
        include_str!("../src/runners/secrets.rs"),
        include_str!("../src/runners/yamlfmt.rs"),
    ];

    let mut names = BTreeSet::new();
    for source in SOURCES {
        let mut rest: &str = source;
        while let Some(at) = rest.find("#[test]") {
            rest = &rest[at + "#[test]".len()..];
            // Stop at the next attribute, so a `#[test]` that is somehow not
            // followed by a function cannot claim the following one's name.
            let window = match rest.find("#[test]") {
                Some(next) => &rest[..next],
                None => rest,
            };
            if let Some(fn_at) = window.find("fn ") {
                let name: String = window[fn_at + 3..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    names.insert(name);
                }
            }
        }
    }
    names
}

/// **The pairing is mechanical, not a convention.**
///
/// `oracle/error-class.txt` makes *adding* a free pass a visible diff. It does
/// not make the entry answerable: an error-class entry matches on any failure
/// at all, so nothing in the ratchet notices if the refusal behind it turns
/// into a success, or into a different refusal for a different reason.
///
/// Until this test, what closed that was a convention — "each one is paired
/// with a unit test asserting egret's own message" — written in prose in three
/// places. Tasks 6-8 will add more entries (Task 8's entire deliverable is
/// refusals), and a convention nobody checks is not a constraint. So
/// `oracle/error-class-pairs.txt` commits the pairing and this asserts it:
///
/// - the two files name **exactly** the same ids, in both directions;
/// - every `test:<name>` names a `#[test] fn` that **exists** in the crate;
/// - every `baselined` claim is **true** of `oracle/baseline.txt`;
/// - the `unported:` set is exactly [`UNPORTED_ERROR_CLASS`].
///
/// An error-class entry added without a pair fails here **by name**, the way a
/// stale baseline line already does.
/// The `test:` scan must cover every module in `src/runners/`, or a pairing
/// naming a real test in an unscanned file is rejected as nonexistent.
///
/// The list is a `const` and the directory is not, so nothing but this
/// notices when a module is added — as four were in W2 Task 6.
#[test]
fn every_runners_module_is_scanned_for_declared_tests() {
    let mut on_disk: Vec<String> =
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/runners"))
            .expect("src/runners must be readable")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().into_owned();
                (name.ends_with(".rs") && name != "mod.rs").then_some(name)
            })
            .collect();
    on_disk.sort();

    let scanned = include_str!("oracle.rs");
    for module in &on_disk {
        assert!(
            scanned.contains(&format!("include_str!(\"../src/runners/{module}\")")),
            "`src/runners/{module}` is not in `declared_test_names`' SOURCES, so a \
             `test:` pairing naming one of its tests would be rejected as nonexistent"
        );
    }
    // 20 since package #5b Task B7 (2026-10-29) added `error_logs.rs`; 19
    // since its Task B6 added `nested.rs` (18 since
    // its Task B4 added `decimalformat.rs`, 17
    // since its Task B2 added `pebble_tests.rs`, 16
    // since package #5a Task A6 added `operators.rs`, 15 since its Task A3
    // added `arith.rs`, 14 since W9c Task 4 fix round 1 added `cmp_rewrite.rs`
    // and `compare.rs`, 12 since W9a Task 4's `files.rs`, 11 since W5b Task
    // 4's `secrets.rs`).
    assert_eq!(on_disk.len(), 20, "a module was added or removed");
}

#[test]
fn every_error_class_entry_is_paired_with_something_that_can_fail() {
    let pairs: Vec<(String, String)> = include_str!("../oracle/error-class-pairs.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next().unwrap_or_default().to_string();
            let justification = parts.next().unwrap_or_default().to_string();
            assert!(
                parts.next().is_none(),
                "{id}: a pairing line is `<id> <justification>` and nothing else"
            );
            (id, justification)
        })
        .collect();

    let paired: BTreeSet<String> = pairs.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        paired.len(),
        pairs.len(),
        "oracle/error-class-pairs.txt has duplicate ids"
    );

    let allowed = error_class_allowlist();
    let unpaired: Vec<&String> = allowed.difference(&paired).collect();
    let orphaned: Vec<&String> = paired.difference(&allowed).collect();
    assert!(
        unpaired.is_empty(),
        "{} error-class entr(y/ies) have no line in oracle/error-class-pairs.txt. An \
         error-class entry can never fail, so something else has to be able to -- name a \
         test that asserts egret's own message for it: {unpaired:?}",
        unpaired.len()
    );
    assert!(
        orphaned.is_empty(),
        "oracle/error-class-pairs.txt names {} id(s) that are no longer error-class -- \
         delete them: {orphaned:?}",
        orphaned.len()
    );

    let declared = declared_test_names();
    // A canary plus a floor, so a scan that silently stops working is
    // distinguishable from a test that was legitimately renamed. Without
    // this, `declared` going empty would turn every `test:` pair below into a
    // failure that reads like a rename, and a scan that returned everything
    // would make them all pass vacuously.
    assert!(
        declared.contains("render_corpus_matches_golden_is_not_in_this_crates_lib")
            || declared.contains("an_undefined_is_in_operand_raises_rather_than_answering_false"),
        "the `#[test]` scan lost a name it is known to contain, so it has stopped working \
         rather than the crate having lost its tests"
    );
    assert!(
        declared.len() > 50,
        "the `#[test]` scan found only {} names across the three `runners` modules; it \
         found 93 when this was written",
        declared.len()
    );

    let baseline = baseline();
    let mut unported = BTreeSet::new();
    for (id, justification) in &pairs {
        match justification.split_once(':') {
            Some(("test", name)) => assert!(
                declared.contains(name),
                "{id} is paired with `{name}`, which is not a `#[test] fn` anywhere in this \
                 crate. A renamed or deleted test leaves the entry unanswerable"
            ),
            Some(("unported", task)) => {
                assert!(
                    !task.is_empty(),
                    "{id}: `unported:` must name the task that owns it"
                );
                unported.insert(id.clone());
            }
            None if justification == "baselined" => assert!(
                baseline.contains(id),
                "{id} is paired as `baselined`, but it is not in oracle/baseline.txt -- so it \
                 IS born matching and the claim is false"
            ),
            _ => panic!(
                "{id}: unknown justification {justification:?}. Use `test:<name>`, \
                 `baselined`, or `unported:<task>`"
            ),
        }
    }

    let expected_unported: BTreeSet<String> =
        UNPORTED_ERROR_CLASS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        unported, expected_unported,
        "the set of `unported:` error-class entries changed. These are the entries with \
         NOTHING ported behind them, so growing the set is the one move that quietly weakens \
         the oracle -- edit UNPORTED_ERROR_CLASS in this file, deliberately, if that is what \
         you mean"
    );
}

/// What one oracle run found wrong, before it is turned into a message.
struct Findings<'a> {
    /// Ids whose render panicked. **Not** covered by the baseline.
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
/// A pure function on purpose. The differential test's own assertion is
/// `assert!(failures.is_empty())`, so **every** rule about what fails lives
/// here and can be exercised without a renderer — in particular the rule that
/// a panic fails outside the baseline, which is otherwise unreachable by any
/// test: deleting that block leaves the whole suite green while restoring a
/// silent exit-0 on a panic over any baselined entry.
fn assemble_failures(f: &Findings<'_>) -> String {
    let detail_for = |ids: &[&String]| {
        ids.iter()
            .filter_map(|id| f.detail.get(*id).map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut failures = String::new();

    // Unconditional, and NOT subject to the baseline. A baselined id means
    // "egret does not reproduce this value yet"; it never means "egret may
    // crash here". Without this, a panic on an already-baselined entry is
    // reported in the printed summary and still exits 0 -- the same class of
    // silence the `Outcome::Panic` variant was introduced to end.
    if !f.panics.is_empty() {
        failures.push_str(&format!(
            "\nPANIC: the renderer panicked on {} entr(y/ies). A panic is never an acceptable \
             outcome, baselined or not:\n{}\n",
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
            "\noracle/baseline.txt names {} id(s) that are not in golden.json at all -- \
             delete them: {:?}\n",
            f.stale.len(),
            f.stale
        ));
    }
    if !f.regressions.is_empty() {
        failures.push_str(&format!(
            "\nREGRESSION: {} entr(y/ies) mismatch that were NOT in oracle/baseline.txt. \
             Fix them, do not add them to the baseline:\n{}\n",
            f.regressions.len(),
            detail_for(f.regressions)
        ));
    }
    if !f.banked.is_empty() {
        failures.push_str(&format!(
            "\nPROGRESS NOT BANKED: {} entr(y/ies) in oracle/baseline.txt now MATCH. \
             Delete these lines from oracle/baseline.txt in the same commit that fixed \
             them:\n  {}\n",
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
/// `a_panicking_renderer_is_a_mismatch_in_every_mode` covers `entry_matches`
/// — the *mode* half. This covers the other half: with no regressions, no
/// stale lines and nothing banked (i.e. a run that is green by the ratchet's
/// own reckoning, because every panicking id was already baselined), a panic
/// must still produce a failure. Deleting the `panics` block in
/// [`assemble_failures`] compiles cleanly and leaves every other test green;
/// this is what dies.
#[test]
fn a_panic_fails_even_when_the_ratchet_is_satisfied() {
    let panicked = vec!["gt-mixed".to_string()];
    let mut detail = BTreeMap::new();
    detail.insert("gt-mixed".to_string(), "  gt-mixed: PANIC(...)".to_string());

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
        message.contains("gt-mixed"),
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
/// Until the merge gate this arm was dead code to the test suite. A green run
/// sits exactly at baseline, so `stale`, `regressions` and `banked` are all
/// empty on every passing run, and nothing else constructs a [`Findings`]:
/// measured, neutering all three (`if false && !f.stale.is_empty()`, and
/// likewise for the other two) left
/// `cargo test -p egret-workflow --test oracle` at **7 passed, 0 failed**.
/// The whole branch's authority rests on this function, so each arm now has a
/// test that dies when it is neutered — verified one arm at a time, not as a
/// set.
///
/// `stale` is the arm that keeps `baseline.txt` honest against a corpus that
/// has moved: a line naming an id `golden.json` no longer contains is a free
/// pass that can never be spent, and silently accumulating those is how a
/// ratchet becomes a suppression list.
#[test]
fn a_stale_baseline_line_fails_and_names_it() {
    let gone = "an-id-that-golden-json-does-not-have".to_string();
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
        message.contains("an-id-that-golden-json-does-not-have"),
        "the message must NAME the stale id, or a maintainer cannot act on it; got: {message:?}"
    );

    // And the same findings with nothing stale are a pass, so the assertions
    // above are about `stale` and not about the function always complaining.
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
/// The direction the ratchet exists for: an id that mismatches and is *not*
/// baselined. See [`a_stale_baseline_line_fails_and_names_it`] for why this
/// needs its own test at all.
///
/// The detail assertion is deliberate. `detail_for` is a separate closure from
/// the `panics` arm's inline lookup, and a regression reported by bare id —
/// with no expected/actual — is most of the way to unactionable.
#[test]
fn a_regression_outside_the_baseline_fails_and_carries_its_detail() {
    let id = "kestra-null-coalesce".to_string();
    let regressions = [&id];
    let mut detail = BTreeMap::new();
    detail.insert(
        id.clone(),
        "  kestra-null-coalesce: expected Ok(\"a\") actual Ok(\"b\")".to_string(),
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
        message.contains("kestra-null-coalesce"),
        "the message must name the entry; got: {message:?}"
    );
    assert!(
        message.contains("expected Ok(\"a\") actual Ok(\"b\")"),
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
/// baselined id that now MATCHES must fail until its line is deleted. See
/// [`a_stale_baseline_line_fails_and_names_it`] for why this needs its own
/// test at all.
///
/// This is not hypothetical. The merge gate's own `arbitrary_precision` fix
/// banked `context-integer-past-u64` and its two siblings, and this arm is
/// what demanded the deletion; with the arm neutered the fix would have
/// landed with three permanent free passes still in the file, each carrying a
/// justification that had become false.
#[test]
fn progress_that_is_not_banked_fails_and_names_it() {
    let fixed = "context-integer-past-u64".to_string();
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
        message.contains("context-integer-past-u64"),
        "the message must name the line to delete; got: {message:?}"
    );
    assert!(
        message.contains("oracle/baseline.txt"),
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
    let mut panics = Vec::new();

    for entry in &golden {
        let actual_result = render_via_egret(&entry.expr, &entry.context);

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

        let panicked = matches!(actual_result, Outcome::Panic(_));
        if panicked {
            panics.push(entry.id.clone());
        } else if non_deterministic {
            shape_checked.push(entry.id.clone());
        } else if entry.compare == CompareMode::ErrorClass {
            error_class.push(entry.id.clone());
        }

        let matches = entry_matches(entry, &actual_result);

        if !matches {
            let expected_display = match &entry.result {
                GoldenResult::Ok(s) => format!("Ok({s:?})"),
                GoldenResult::Err { error } => format!("Err({error:?})"),
            };
            let actual_display = actual_result.display();
            let note = match (panicked, non_deterministic, entry.compare) {
                (true, _, _) => " [PANIC -- never a match, in any mode]",
                (false, true, _) => " [SHAPE-CHECKED, not compared by value]",
                (false, false, CompareMode::ErrorClass) => " [ERROR-CLASS: egret must also fail]",
                (false, false, CompareMode::Exact) => "",
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
    if !panics.is_empty() {
        println!(
            "  {} PANICKED and are mismatches regardless of mode: {}",
            panics.len(),
            panics.join(", ")
        );
    }

    let baseline = baseline();

    let stale: Vec<&String> = baseline
        .iter()
        .filter(|id| !ids.contains(id.as_str()))
        .collect();
    let regressions: Vec<&String> = mismatching.difference(&baseline).collect();
    let banked: Vec<&String> = baseline.difference(&mismatching).collect();

    let failures = assemble_failures(&Findings {
        panics: &panics,
        stale: &stale,
        regressions: &regressions,
        banked: &banked,
        detail: &detail,
    });

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
