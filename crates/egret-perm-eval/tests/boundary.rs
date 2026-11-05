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

/// Cuts every line at its first `//` (comments, doc comments, trailing comments).
/// Naive: a `//` inside a string literal also cuts the line, which can only hide
/// text after it, so src/ must not put forbidden items after a string with `//`.
fn code_only(text: &str) -> String {
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when `tok` occurs at an identifier boundary: not preceded by an identifier
/// char and, when it ends in one, not followed by one (so `refs::` and `hyperlink`
/// do not match `fs::` and `hyper`).
fn has_token(text: &str, tok: &str) -> bool {
    let ends_ident = tok.chars().last().is_some_and(is_ident);
    text.match_indices(tok).any(|(i, _)| {
        let before = text[..i].chars().last();
        let after = text[i + tok.len()..].chars().next();
        !before.is_some_and(is_ident) && !(ends_ident && after.is_some_and(is_ident))
    })
}

/// Identifiers named inside every `std::{ ... }` group, across lines.
fn std_group_idents(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("std::{") {
        let body = &text[i + "std::{".len()..];
        let mut depth = 1;
        let mut end = body.len();
        for (j, c) in body.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.extend(
            body[..end]
                .split(|c: char| !is_ident(c))
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
        );
    }
    out
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
            assert!(!has_token(&text, word), "{p:?} uses {word}");
        }
        // Grouped imports, also over several lines: `use std::{fs, net};`
        for tok in std_group_idents(&text) {
            assert!(
                !["fs", "net", "process", "env"].contains(&tok.as_str()),
                "{p:?} imports std::{tok}"
            );
        }
    }
}
