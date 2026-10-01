//! No `just` recipe may run `cargo test` while `EGRET_ALLOW_DB_SKIP` is inherited.
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
//! # The bound: which spellings of "run the tests" this can see
//!
//! Review round 2 found the scan matched the literal `cargo test` alone, so the first
//! `cargo nextest run` recipe anyone adds would slip straight past it — a guard whose
//! blind spot is a different invocation of the same thing. [`TEST_RUNNERS`] is now that
//! bound, written as a list rather than left implicit in a `contains`, so widening it is
//! an edit in one place and the gap is visible without reading the filter.

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
const TEST_RUNNERS: [&str; 2] = ["cargo test", "cargo nextest"];

/// Whether a recipe line runs the test suite at all.
fn runs_tests(line: &str) -> bool {
    TEST_RUNNERS.iter().any(|r| line.contains(r))
}

/// `crates/egret-test-db` → the workspace root, which is where `justfile` lives.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<crate> is two levels below the workspace root")
        .to_path_buf()
}

/// Expand `{{ name }}` using `justfile`'s own top-level `name := "value"` assignments.
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

#[test]
fn no_just_recipe_runs_cargo_test_with_the_skip_inherited() {
    let justfile = repo_root().join("justfile");
    let text = std::fs::read_to_string(&justfile)
        .unwrap_or_else(|e| panic!("read {}: {e}", justfile.display()));

    let vars: Vec<(String, String)> = text
        .lines()
        .filter(|l| !l.starts_with(char::is_whitespace))
        .filter_map(|l| {
            let (name, rest) = l.split_once(":=")?;
            let value = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
            Some((name.trim().to_string(), value.to_string()))
        })
        .collect();
    assert!(
        vars.iter().any(|(n, _)| n == "no_db_skip"),
        "justfile no longer defines `no_db_skip`; the recipes that must not inherit the \
         opt-out have nothing to inherit the scrub from"
    );

    // Recipe bodies are indented. Join `\`-continued lines so a prefix on one physical
    // line still covers the `cargo test` on the next — which is how the StarRocks recipe
    // is written.
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

    let offenders: Vec<String> = logical
        .iter()
        .filter(|(_, l)| runs_tests(l))
        .map(|(n, l)| (n, expand(l, &vars)))
        .filter(|(_, l)| {
            !l.contains("env -u EGRET_ALLOW_DB_SKIP") && !l.contains("EGRET_ALLOW_DB_SKIP=")
        })
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
