//! No `just` recipe may run `cargo test` while a service-skip opt-out is inherited.
//!
//! # Why this is a test and not a comment
//!
//! `EGRET_ALLOW_DB_SKIP` turns every DB-gated suite in the workspace back into a skip, and
//! a skip reports `ok` having asserted nothing — measured at 315 empty tests in
//! `egret-dataapi` alone. The skip's own `eprintln!` is swallowed by libtest, because the
//! test *passes*. So a `just check` that inherited the variable would print a full green
//! and there would be nothing on the terminal to say otherwise.
//!
//! `egret_test_db` refuses an opt-out that a `.env` file sets, which closes the route
//! `justfile`'s `set dotenv-load := true` opens. It cannot close the other one: a variable
//! exported in a shell profile is, inside the process, indistinguishable from one the
//! operator typed. Only the recipe can drop that, with `env -u`.
//!
//! Per-recipe scrubbing is forgettable, which is the shape of defect this repo keeps
//! shipping — so the rule is machine-checked here instead of written down and hoped for.
//! Add a recipe that runs `cargo test` without either scrubbing the variable or setting it
//! deliberately, and this fails naming the line.
//!
//! Not DB-gated: it reads a file in the repository and runs everywhere, including on a
//! laptop with no Postgres.
//!
//! # Two axes, and why the second one is here
//!
//! SP4d Task 8 found the same hole one axis over. `EGRET_ALLOW_STORE_SKIP` gates all
//! fourteen tests in `crates/egret-dataapi/tests/serving_shared_store.rs` — the only suite
//! that judges whether serving state is actually shared across replicas — and `just
//! check`, which decided the DB, ENGINE and DOCKER axes, left that one undecided. The
//! workspace run therefore ended `0 passed; 14 failed` (measured), and the panic's own
//! remedy text tells the reader to export `EGRET_ALLOW_STORE_SKIP=1`; from that export
//! onward every `check` skipped all fourteen with the `eprintln!` swallowed. Worse-shaped
//! than the DB hole, because the recipe forced its own defeat.
//!
//! [`no_unfiltered_cargo_test_leaves_the_store_axis_undecided`] is that axis, and its
//! reach is deliberately narrower than the DB rule's — see its own doc comment.
//!
//! # The bound: which spellings of "run the tests" this can see
//!
//! Review round 2 found the scan matched the literal `cargo test` alone, so the first
//! `cargo nextest run` recipe anyone adds would slip straight past it — a guard whose
//! blind spot is a different invocation of the same thing. [`TEST_RUNNERS`] is now that
//! bound, written as a list rather than left implicit in a `contains`, so widening it is
//! an edit in one place and the gap is visible without reading the filter.
//!
//! Widening the list did not close the hole, and SP4d Task 8 re-measured what still slips
//! rather than restating the earlier claim. Substring matching on `cargo test` cannot see:
//!
//! * `cargo t` — the built-in alias. Not added to [`TEST_RUNNERS`] on purpose: `cargo t`
//!   is a prefix of `cargo tree`, which is not a test run, and a guard that flags innocent
//!   lines gets widened by deleting it.
//! * `cargo +nightly test`, `cargo --config … test`, `cargo -Z … test` — anything between
//!   the binary and the subcommand. Pinned as a fact by
//!   [`the_runner_scan_still_misses_these_spellings`] below, so nobody reads this guard as
//!   total.
//! * `$CARGO test`, `./scripts/whatever.sh`, `make test` — a recipe that shells out at
//!   all. The command this scan reads is a string in a `justfile`, not a process tree.
//!
//! Closing those needs a tokeniser over the recipe line (binary, flags, subcommand)
//! instead of `contains`. That is a bigger change than this plan's remaining budget, and
//! the gap is recorded rather than quietly carried.

use std::path::{Path, PathBuf};

/// Every spelling of "run the Rust tests" this scan recognises.
///
/// `cargo test` was the only one until review round 2 pointed out that
/// `cargo nextest run` — a drop-in this repo could adopt at any time — reads
/// `EGRET_ALLOW_DB_SKIP` from the same environment and would have been invisible here.
/// Both are prefixes, so `cargo nextest run …` and `cargo test --workspace …` match.
///
/// This list IS the guard's reach. Anything absent from it is unguarded, which is why it
/// is a named constant the failure message can print rather than a string inside a filter.
/// The module doc lists the spellings measured to slip past it.
const TEST_RUNNERS: [&str; 2] = ["cargo test", "cargo nextest"];

/// Whether a recipe line runs the test suite at all.
fn runs_tests(line: &str) -> bool {
    TEST_RUNNERS.iter().any(|r| line.contains(r))
}

/// Whether the command names which test targets to build, rather than running every test
/// binary its package selection has.
///
/// This is what makes the STORE rule narrower than the DB rule, and the asymmetry is
/// deliberate: `EGRET_ALLOW_DB_SKIP` is read by `egret_test_db::url()`, which suites all
/// over the workspace call, so "which targets" barely narrows the blast radius. The store
/// opt-out is read by exactly one test file, so a line that names its targets either
/// selects that file (and then it supplies the URL, which decides the axis) or cannot
/// reach it at all. Policing those lines would add four noisy tokens to recipes where the
/// variable is unreachable, and noise is how the DB rule nearly got deleted.
fn names_its_targets(line: &str) -> bool {
    [
        "--test ",
        "--lib",
        "--bins",
        "--bin ",
        "--doc",
        "--benches",
        "--examples",
    ]
    .iter()
    .any(|f| line.contains(f))
}

/// Whether a line has DECIDED what happens to `EGRET_ALLOW_DB_SKIP`.
fn decides_db(line: &str) -> bool {
    line.contains("env -u EGRET_ALLOW_DB_SKIP") || line.contains("EGRET_ALLOW_DB_SKIP=")
}

/// Whether a line has DECIDED the store axis, by any of the three means that settle it.
///
/// Supplying `EGRET_TEST_SHARED_STORE_URL` counts, and it is not a loophole: the suite's
/// `store_url()` consults the opt-out only when the URL is missing or blank, so a line
/// that names a store cannot be made vacuous by an exported variable.
fn decides_store(line: &str) -> bool {
    line.contains("env -u EGRET_ALLOW_STORE_SKIP")
        || line.contains("EGRET_ALLOW_STORE_SKIP=")
        || line.contains("EGRET_TEST_SHARED_STORE_URL=")
}

/// `crates/egret-test-db` → the workspace root, which is where `justfile` lives.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<crate> is two levels below the workspace root")
        .to_path_buf()
}

fn justfile_text() -> String {
    let justfile = repo_root().join("justfile");
    std::fs::read_to_string(&justfile)
        .unwrap_or_else(|e| panic!("read {}: {e}", justfile.display()))
}

/// `justfile`'s own top-level `name := "value"` assignments.
///
/// Only string literals: an assignment built from a function call has no value this can
/// substitute, so `{{ … }}` is left unexpanded, which fails a check rather than passing
/// one — the safe direction for a guard.
fn justfile_vars(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.starts_with(char::is_whitespace))
        .filter_map(|l| {
            let (name, rest) = l.split_once(":=")?;
            let value = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
            Some((name.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// Expand `{{ name }}` using `justfile`'s own top-level assignments.
///
/// One level of substitution, which is all the recipes here use — and if a future recipe
/// nests them, the unexpanded `{{ … }}` simply fails the check rather than passing it
/// silently, which is the safe direction for a guard.
fn expand(line: &str, vars: &[(String, String)]) -> String {
    let mut out = line.to_string();
    for (name, value) in vars {
        for spelling in [format!("{{{{ {name} }}}}"), format!("{{{{{name}}}}}")] {
            out = out.replace(&spelling, value);
        }
    }
    out
}

/// Recipe bodies, one entry per logical command, as `(1-based line number, text)`.
///
/// Recipe bodies are indented. `\`-continued lines are joined so a prefix on one physical
/// line still covers the `cargo test` on the next — which is how `check` and the StarRocks
/// recipes are written.
fn logical_recipe_lines(text: &str) -> Vec<(usize, String)> {
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (i, raw) in text.lines().enumerate() {
        if !raw.starts_with(char::is_whitespace) {
            pending = None;
            continue;
        }
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        let (start, mut acc) = pending.take().unwrap_or((i + 1, String::new()));
        if !acc.is_empty() {
            acc.push(' ');
        }
        match line.strip_suffix('\\') {
            Some(head) => {
                acc.push_str(head.trim_end());
                pending = Some((start, acc));
            }
            None => {
                acc.push_str(line);
                logical.push((start, acc));
            }
        }
    }
    if let Some(unterminated) = pending {
        logical.push(unterminated);
    }
    logical
}

#[test]
fn no_just_recipe_runs_cargo_test_with_the_skip_inherited() {
    let text = justfile_text();
    let vars = justfile_vars(&text);
    assert!(
        vars.iter().any(|(n, _)| n == "no_db_skip"),
        "justfile no longer defines `no_db_skip`; the recipes that must not inherit the \
         opt-out have nothing to inherit the scrub from"
    );

    let logical = logical_recipe_lines(&text);

    let offenders: Vec<String> = logical
        .iter()
        .filter(|(_, l)| runs_tests(l))
        .map(|(n, l)| (n, expand(l, &vars)))
        .filter(|(_, l)| !decides_db(l))
        .map(|(n, l)| format!("  justfile:{n}: {l}"))
        .collect();

    assert!(
        offenders.is_empty(),
        "these `just` recipes run the tests ({}) without deciding what happens to \
         EGRET_ALLOW_DB_SKIP, so an exported one would make every DB-gated suite in their \
         reach report `ok` having asserted nothing:\n{}\n\nPrefix the command with \
         `{{{{ no_db_skip }}}}` to run the tests for real, or set EGRET_ALLOW_DB_SKIP=1 on \
         the line if skipping is what the recipe is for (see `test-no-db`).",
        TEST_RUNNERS.join(" / "),
        offenders.join("\n")
    );

    // The check has to be able to fail, and a scan whose pattern silently matches nothing
    // is the failure mode this repo has recorded most often. Two recipes are known to run
    // the suite; if that ever drops to zero the assertion above is vacuous.
    let scanned = logical.iter().filter(|(_, l)| runs_tests(l)).count();
    assert!(
        scanned >= 2,
        "expected to have scanned at least the `check` and `test-no-db` recipes, found \
         {scanned} lines matching {TEST_RUNNERS:?} — the scan is matching nothing and would \
         pass whatever the justfile said"
    );

    // And the reach itself, stated as an assertion rather than left to the list's spelling:
    // a `cargo nextest` recipe must be SEEN by this scan. Without this the widening could be
    // reverted to the single literal and nothing here would notice.
    assert!(
        runs_tests("    {{ no_db_skip }} cargo nextest run --workspace"),
        "a `cargo nextest` recipe must be inside this guard's reach, or the next runner this \
         repo adopts is unguarded in exactly the way `cargo test` once was"
    );
    assert!(
        !runs_tests("    cargo build --workspace --all-targets"),
        "and the scan must not claim every cargo invocation — a guard that flags `cargo \
         build` gets widened by deleting it"
    );
}

/// The STORE axis: a recipe that runs every test binary it selects must decide
/// `EGRET_ALLOW_STORE_SKIP`.
///
/// SP4d Task 8. `just check` decided three service axes and left this one undecided, so
/// its workspace run ended `0 passed; 14 failed` on
/// `crates/egret-dataapi/tests/serving_shared_store.rs` — and the panic text that failure
/// prints names `EGRET_ALLOW_STORE_SKIP=1` as the remedy, so the recipe's own output
/// recruits the reader into hollowing it out permanently and silently.
///
/// # Reach, which is narrower than the DB rule's and says so
///
/// Only lines that do NOT name their test targets are policed — see [`names_its_targets`]
/// for why. What that leaves outside: a line like
/// `cargo test -p egret-dataapi --test serving_shared_store` is unpoliced. It is also the
/// one spelling that cannot be hurt, because selecting that suite by name without
/// supplying a store is a run whose fourteen failures are the loudest output in the
/// terminal — the opposite of a silent green. The hole this rule exists to close is the
/// unfiltered run, where fourteen skips vanish into three thousand passes.
#[test]
fn no_unfiltered_cargo_test_leaves_the_store_axis_undecided() {
    let text = justfile_text();
    let vars = justfile_vars(&text);
    let logical = logical_recipe_lines(&text);

    let unfiltered: Vec<(usize, String)> = logical
        .iter()
        .filter(|(_, l)| runs_tests(l) && !names_its_targets(l))
        .map(|(n, l)| (*n, expand(l, &vars)))
        .collect();

    let offenders: Vec<String> = unfiltered
        .iter()
        .filter(|(_, l)| !decides_store(l))
        .map(|(n, l)| format!("  justfile:{n}: {l}"))
        .collect();

    assert!(
        offenders.is_empty(),
        "these `just` recipes run every test binary they select without deciding what \
         happens to EGRET_ALLOW_STORE_SKIP, which gates all fourteen tests in \
         crates/egret-dataapi/tests/serving_shared_store.rs:\n{}\n\nDecide it on the line, \
         one of three ways: supply `EGRET_TEST_SHARED_STORE_URL=…` to RUN them (what \
         `check` does — see `just up`), add `{{{{ no_store_skip }}}}` to refuse an inherited \
         opt-out, or set `EGRET_ALLOW_STORE_SKIP=1` if skipping is what the recipe is for \
         (see `test-no-db`). Undecided means the run either fails on fourteen tests or, \
         once somebody exports the opt-out the panic text recommends, passes them \
         vacuously forever.",
        offenders.join("\n")
    );

    // Same anti-vacuity floor as the DB rule: `check` and `test-no-db` are both unfiltered
    // workspace runs, so a count below two means this scan stopped seeing recipes.
    assert!(
        unfiltered.len() >= 2,
        "expected at least the `check` and `test-no-db` workspace runs to be unfiltered, \
         found {} — the scan is matching nothing and would pass whatever the justfile said",
        unfiltered.len()
    );

    // The predicate has to be able to say BOTH things, on this branch, over the exact
    // spellings the justfile uses. A guard on this programme called two byte-equal bodies
    // "differing" and only its accept-control caught it.
    assert!(
        !decides_store("    cargo test --workspace"),
        "an undecided line must be an offender, or this rule is vacuous"
    );
    for decided in [
        "    env -u EGRET_ALLOW_STORE_SKIP cargo test --workspace",
        "    EGRET_ALLOW_STORE_SKIP=1 cargo test --workspace",
        "    EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:6399 cargo test --workspace",
    ] {
        assert!(
            decides_store(decided),
            "this line decides the store axis and must not be an offender: {decided}"
        );
    }
    // And the filter that narrows the rule: these two classifications are what keep it off
    // the target-named recipes, so they are asserted rather than assumed.
    assert!(!names_its_targets("    cargo test --workspace"));
    assert!(names_its_targets(
        "    cargo test -p egret-policy --test ranger_engine_gate --no-fail-fast"
    ));
    assert!(names_its_targets(
        "    cargo test -p egret-bin -p egret-workflow --lib --test oracle --no-fail-fast"
    ));
}

/// The runner scan's blind spot, pinned as a fact so the guard above cannot be misread as
/// total.
///
/// Both rules find test runs by substring, so anything between `cargo` and the subcommand
/// hides the run from them. Closing it needs a tokeniser over the recipe line, which is a
/// larger change than the plan that widened [`TEST_RUNNERS`] had budget for. If somebody
/// does close it, THIS test fails — which is the right way for a recorded bound to end.
#[test]
fn the_runner_scan_still_misses_these_spellings() {
    for slips in [
        "    cargo +nightly test --workspace",
        "    cargo --config 'build.jobs=2' test --workspace",
        "    cargo t --workspace",
        "    ./scripts/run-the-tests.sh",
        "    $CARGO test --workspace",
    ] {
        assert!(
            !runs_tests(slips),
            "`{slips}` is now inside the scan's reach — good. Delete it from this list (and \
             from the module doc's bound) rather than widening the list back."
        );
    }
}
