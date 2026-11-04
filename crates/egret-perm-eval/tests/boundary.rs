//! egret-perm-eval is pure and target-neutral.
#[test]
fn the_evaluator_depends_on_no_database_no_runtime_and_no_target() {
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest.split("[dependencies]").nth(1).unwrap();
    let deps = deps.split("[dev-dependencies]").next().unwrap();
    for forbidden in [
        "sqlx",
        "tokio",
        "egret-policy",
        "egret-ranger-eval",
        "egret-perm-compile",
        "reqwest",
        "axum",
    ] {
        assert!(
            !deps.contains(forbidden),
            "egret-perm-eval must not depend on {forbidden}"
        );
    }
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Drops `//` comment lines (doc comments included) so prose may name a forbidden item.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn src_does_no_io() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&dir, &mut files);
    assert!(!files.is_empty(), "no .rs files found under {dir:?}");
    let forbidden = [
        "std::fs",
        "std::net",
        "std::process",
        "std::env",
        "std::io::stdin",
        "std::io::stdout",
        "std::io::stderr",
        "std::time::SystemTime",
        "SystemTime",
        "tokio",
        "sqlx",
        "reqwest",
        "hyper",
        "File::",
        "fs::",
        "net::",
        "env::",
        "process::",
    ];
    for p in files {
        let text = code_only(&std::fs::read_to_string(&p).unwrap());
        for word in forbidden {
            assert!(!text.contains(word), "{p:?} uses {word}");
        }
        // Grouped imports: `use std::{fs, net};`
        for line in text.lines().filter(|l| l.contains("std::{")) {
            for tok in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
                assert!(
                    !["fs", "net", "process", "env"].contains(&tok),
                    "{p:?} imports std::{tok}"
                );
            }
        }
    }
}
