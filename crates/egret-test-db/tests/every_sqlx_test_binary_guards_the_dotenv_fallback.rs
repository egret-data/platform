//! Every test binary containing `#[sqlx::test]` must carry the dotenv-fallback guard.
//!
//! # The hole this closes, measured rather than imagined
//!
//! `#[sqlx::test]` uses nothing in this crate. It resolves `DATABASE_URL` itself, through
//! `dotenvy::dotenv()`, **unconditionally** — so an unexported variable does not make those
//! tests skip and does not make them fail. It makes them read `.env`, and this repo's
//! `.env` (like its `.env.example`) points `DATABASE_URL` at the **dev** database. The
//! scratch databases then land on the dev server with a `_sqlx_test` bookkeeping schema
//! inside the dev database itself, and the suite reports `ok`.
//!
//! That is not hypothetical. A run with `DATABASE_URL` unset had
//! `egret-automation/tests/report_tick.rs` (7 tests) and `tick_end_to_end.rs` (11) **pass**,
//! and `_sqlx_test.databases` inside the dev `egret` showed `n_tup_ins=338`.
//!
//! `egret_test_db::assert_database_url_is_exported()` catches it — but it was in 5 of the
//! 24 files that need it, and "remember to add the guard" is precisely the kind of rule
//! this repo has watched decay. So the rule is checked here: a file with `#[sqlx::test]`
//! whose binary has no guard fails this test, naming the file.
//!
//! # What it does and does not buy
//!
//! Detection, not prevention: the guard is a sibling `#[test]` in the same binary, and
//! libtest gives no ordering, so a run that trips it still creates its scratch databases
//! before going red. Prevention would need a hook that runs before any test in the process
//! — sqlx offers none, a `.cargo/config.toml` `[env]` default for `DATABASE_URL` would
//! redirect `cargo run -p egret-bin` (the dev server) as well, and a pre-main constructor
//! only runs if the linker keeps a crate nothing references. Red-and-noisy beats
//! green-and-silent, which is this package's whole thesis, and that is what this buys.
//!
//! Not DB-gated: it reads the source tree and runs everywhere.

use std::path::{Path, PathBuf};

const GUARD: &str = "assert_database_url_is_exported";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<crate> is two levels below the workspace root")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `#[sqlx::test]` as an attribute, never as the same text inside a doc comment — this
/// file and `egret-test-db/src/lib.rs` both discuss the macro at length without using it.
fn declares_sqlx_test(src: &str) -> bool {
    src.lines()
        .any(|l| l.trim_start().starts_with("#[sqlx::test"))
}

/// The test binary a file compiles into: one per `crates/<c>/tests/<name>.rs`, and one
/// shared `--lib` binary for everything under `crates/<c>/src/`. The guard has to be in
/// the same binary to run in the same process, so this is the unit the rule applies to.
fn binary_key(rel: &Path) -> Option<String> {
    let parts: Vec<_> = rel
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    let krate = parts.get(1)?;
    match parts.get(2)?.as_str() {
        "src" => Some(format!("{krate} --lib")),
        "tests" => Some(format!(
            "{krate} --test {}",
            parts.get(3)?.replace(".rs", "")
        )),
        _ => None,
    }
}

#[test]
fn every_binary_using_sqlx_test_asserts_database_url_is_exported() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);

    let mut needs: Vec<(String, PathBuf)> = Vec::new();
    let mut guarded: Vec<String> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let Some(key) = binary_key(rel) else {
            continue;
        };
        if src.contains(&format!("{GUARD}()")) {
            guarded.push(key.clone());
        }
        if declares_sqlx_test(&src) {
            needs.push((key, rel.to_path_buf()));
        }
    }

    assert!(
        needs.len() >= 20,
        "found only {} files using `#[sqlx::test]`; the scan is matching almost nothing \
         and would pass whatever the tree contained",
        needs.len()
    );

    let mut missing: Vec<String> = needs
        .iter()
        .filter(|(key, _)| !guarded.contains(key))
        .map(|(key, rel)| format!("  {} (binary: cargo test -p {key})", rel.display()))
        .collect();
    missing.sort();
    missing.dedup();

    assert!(
        missing.is_empty(),
        "these `#[sqlx::test]` binaries have no `egret_test_db::{GUARD}()`, so with \
         DATABASE_URL unexported they would silently create their scratch databases inside \
         the DEV database `egret` and report `ok`:\n{}\n\nAdd to each binary (in the \
         `tests/<name>.rs` itself, or anywhere in `src/` for a `--lib` binary):\n\n    \
         #[test]\n    fn database_url_is_exported_rather_than_read_from_dotenv() {{\n        \
         egret_test_db::{GUARD}();\n    }}",
        missing.join("\n")
    );
}
