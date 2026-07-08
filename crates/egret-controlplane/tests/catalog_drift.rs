//! Reverse drift lint: every key in `egret_core::rbac::PERMISSIONS` must be
//! enforced by SOME call site in the running code, not just declared in the
//! catalog. This is a pure source-text scan (no DB, runs in CI) that catches
//! the class of bug this branch fixes — a catalog entry with zero
//! `require_permission`/`require_platform` call sites (e.g. the old
//! `database:*` / dead `workspace:*` false alarm).
//!
//! Most control-plane routes gate on `egret_controlplane::auth::require_permission`.
//! `workspace:*` keys are the one exception: they're enforced by the sibling
//! `egret_workspace::rbac::Authorizer::require_platform` in
//! `egret-workspace/src/routes.rs`, so that file is scanned too.

use std::collections::HashSet;

/// Catalog keys intentionally not enforced anywhere yet. Must stay empty —
/// if you're tempted to add to this list, either wire up enforcement or
/// remove the catalog entry (see `egret_core::rbac::PERMISSIONS` doc comment).
const ALLOWED_UNENFORCED: &[&str] = &[];

/// Control-plane source files scanned for `require_permission(..., "key")`
/// call sites, relative to this crate's `src/`.
const CONTROLPLANE_FILES: &[&str] = &[
    "engines.rs",
    "warehouses.rs",
    "storage.rs",
    "secrets.rs",
    "monitor.rs",
    "policies/mod.rs",
    "access/mod.rs",
];

/// Extra source files (outside this crate) that enforce catalog permissions
/// through a different gate function. Path is relative to this crate's
/// `CARGO_MANIFEST_DIR`.
const EXTRA_FILES: &[&str] = &["../egret-workspace/src/routes.rs"];

/// Gate function names whose first-string-literal argument is a
/// `resource:action` permission key.
const GATE_CALLS: &[&str] = &["require_permission(", "require_platform("];

/// Pull every `"resource:action"`-shaped quoted literal out of a line, but
/// only from lines that actually invoke one of `GATE_CALLS` — avoids false
/// hits from unrelated string literals (error messages, etc.) elsewhere in
/// the file.
fn resource_action_literals_in_line(line: &str) -> Vec<String> {
    if !GATE_CALLS.iter().any(|g| line.contains(g)) {
        return Vec::new();
    }
    line.split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| is_resource_action_shaped(s))
        .map(|s| s.to_string())
        .collect()
}

fn is_resource_action_shaped(s: &str) -> bool {
    let Some((res, action)) = s.split_once(':') else {
        return false;
    };
    let ident_ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    ident_ok(res) && ident_ok(action) && !action.contains(':')
}

/// Every `resource:action` key enforced somewhere in the scanned source tree.
fn enforced_keys() -> HashSet<String> {
    let mut out = HashSet::new();
    for rel in CONTROLPLANE_FILES {
        let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/{}"), rel);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        for line in src.lines() {
            out.extend(resource_action_literals_in_line(line));
        }
    }
    for rel in EXTRA_FILES {
        let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/{}"), rel);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        for line in src.lines() {
            out.extend(resource_action_literals_in_line(line));
        }
    }
    out
}

#[test]
fn allowed_unenforced_is_empty() {
    assert!(
        ALLOWED_UNENFORCED.is_empty(),
        "ALLOWED_UNENFORCED should be empty: every catalog permission is expected to be \
         enforced somewhere. If this legitimately grows, document why in a comment here."
    );
}

#[test]
fn every_catalog_permission_is_enforced_somewhere() {
    let enforced = enforced_keys();
    let allowed: HashSet<&str> = ALLOWED_UNENFORCED.iter().copied().collect();

    let dead: Vec<&str> = egret_core::rbac::PERMISSIONS
        .iter()
        .map(|p| p.key)
        .filter(|k| !enforced.contains(*k) && !allowed.contains(k))
        .collect();

    assert!(
        dead.is_empty(),
        "catalog permission(s) enforced nowhere (no require_permission/require_platform \
         call site found in {:?} or {:?}): {dead:?}. Either wire up enforcement, add the key \
         to ALLOWED_UNENFORCED with a reason, or remove it from egret_core::rbac::PERMISSIONS.",
        CONTROLPLANE_FILES,
        EXTRA_FILES,
    );
}

/// Sanity check on the scan itself: a handful of well-known keys must show up,
/// so a change to `GATE_CALLS`/file list that silently stops matching
/// anything doesn't slip through as a vacuously-true pass above.
#[test]
fn scan_actually_finds_known_call_sites() {
    let enforced = enforced_keys();
    for key in [
        "engine:read",
        "engine:kill",
        "policy:read",
        "workspace:read",
    ] {
        assert!(
            enforced.contains(key),
            "expected the source scan to find a call site for {key}"
        );
    }
}
