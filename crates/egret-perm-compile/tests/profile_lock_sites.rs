//! #792's exhaustiveness check: every statement in the repository that takes a
//! row lock on `policy.authz_services` (a `FOR UPDATE` / `FOR SHARE`, an
//! `UPDATE`, a `DELETE`, in Rust or in a migration's trigger) is listed below
//! with the reason it cannot lock a profile this transaction did not lock
//! before the compile-mint lock.
//!
//! The guard (`trigger::lock_profile_rows_in_tx`) refuses a NEW profile lock
//! once the mint lock is held, but only for the locks that go through it. A
//! site added later that locks a profile some other way, after the mint lock,
//! would reopen the deadlock without failing any behaviour test. So a new site
//! fails THIS test, and whoever adds it has to say here why it is safe (or
//! route it through the guard). The scan is textual: a line that is not a
//! comment and holds `UPDATE policy.authz_services`, `DELETE FROM
//! policy.authz_services`, or a `FOR UPDATE` / `FOR SHARE` / `FOR NO KEY` /
//! `FOR KEY SHARE` within three lines of `authz_services`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `(file, number of lock sites in it, why each is safe after the mint lock)`.
const SITES: &[(&str, usize, &str)] = &[
    (
        "crates/egret-perm-compile/src/trigger.rs",
        2,
        "`origin = 'generated'` and `bump_policy_version` write only a profile of the sweep, \
         which `apply_diff_in_tx` locked through the guard first; `infra::converge_infra_in_tx` \
         bumps through `bump_policy_version` only the profile its caller locked (the compile's \
         own, or `lock_for_admin_sync_in_tx`)",
    ),
    (
        "crates/egret-policy/src/store.rs",
        12,
        "`lock_service_rows_via` is the statement the guard calls (and the policy editor, which \
         takes no mint lock before it); the `policy_version` bumps of the policy CRUD write the \
         policy's own profile, which every caller holds (the compiler's sweep, the `egret-admin` \
         sync of a locked profile, the migration undo of the route's own locked profile); \
         `delete_service` and `set_tag_service_via` are admin routes that take no mint lock",
    ),
    (
        "crates/egret-policy/src/default_policies.rs",
        1,
        "the `egret-admin` sync bumps the profile it syncs, which its caller locked \
         (`lock_services_in_tx` / `lock_for_admin_sync_in_tx`)",
    ),
    (
        "crates/egret-catalog-registry/src/tag_version.rs",
        2,
        "`bump_tag_versions_for_warehouses` runs after the mint lock only in \
         `ViewReach::retag_bump_and_recompile` and `TagViewReach::retag_and_bump`, both of \
         which call `lock_services_in_tx` over the same profiles first, so its locks are re-takes",
    ),
    (
        "crates/egret-catalog-engine/src/view_hold.rs",
        2,
        "test code inside `#[cfg(test)]` (a held profile row and its cleanup)",
    ),
    (
        "migrations/0187_view_lineage.sql",
        3,
        "`engines_binding_bumps_profiles` fires on `engine_mgmt.engines` writes, which no \
         writer makes after the mint lock",
    ),
    (
        "migrations/0182_tag_service_bump_reaches_its_linkers.sql",
        1,
        "replaced by 0191's function (same trigger name, `CREATE OR REPLACE`)",
    ),
    (
        "migrations/0191_tag_service_is_what_a_profile_names.sql",
        1,
        "the cascade from a tag service's `policy_version` bump to its linkers: \
         `lock_services_in_tx` locks every linker of the set it locks (one hop, as the trigger \
         goes), so these are re-takes",
    ),
    (
        "migrations/0176_tag_service_requires_schema_level.sql",
        1,
        "one-off data migration",
    ),
    (
        "migrations/0190_tag_block_serves_the_tag_def.sql",
        1,
        "one-off data migration",
    ),
    (
        "migrations/0195_perm_engine_grants.sql",
        1,
        "one-off data migration",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn is_lock_site(lines: &[&str], i: usize) -> bool {
    let line = lines[i];
    if line.contains("UPDATE policy.authz_services")
        || line.contains("DELETE FROM policy.authz_services")
    {
        return true;
    }
    let locking = ["FOR UPDATE", "FOR SHARE", "FOR NO KEY", "FOR KEY SHARE"]
        .iter()
        .any(|k| line.contains(k));
    locking
        && lines[i.saturating_sub(3)..=i]
            .iter()
            .any(|l| l.contains("authz_services"))
}

fn scan_file(path: &Path, comment: &str, root: &Path, out: &mut BTreeMap<String, Vec<usize>>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    for i in 0..lines.len() {
        if lines[i].trim_start().starts_with(comment) {
            continue;
        }
        if is_lock_site(&lines, i) {
            let rel = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .into_owned();
            out.entry(rel).or_default().push(i + 1);
        }
    }
}

fn walk_src(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<usize>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_src(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            scan_file(&path, "//", root, out);
        }
    }
}

#[test]
fn every_profile_row_lock_is_accounted_for() {
    let root = repo_root();
    let mut found: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for krate in std::fs::read_dir(root.join("crates"))
        .expect("crates")
        .flatten()
    {
        // Production code only: `src/`, not `tests/` or `benches/`.
        walk_src(&krate.path().join("src"), &root, &mut found);
    }
    for m in std::fs::read_dir(root.join("migrations"))
        .expect("migrations")
        .flatten()
    {
        if m.path().extension().is_some_and(|e| e == "sql") {
            scan_file(&m.path(), "--", &root, &mut found);
        }
    }

    let expected: BTreeMap<&str, usize> = SITES.iter().map(|(f, n, _)| (*f, *n)).collect();
    let actual: BTreeMap<&str, usize> = found.iter().map(|(f, l)| (f.as_str(), l.len())).collect();
    assert_eq!(
        actual, expected,
        "a `policy.authz_services` row lock was added, moved or removed (lines: {found:?}). \
         A profile locked after the compile-mint lock without the guard \
         (`trigger::lock_profile_rows_in_tx`) can deadlock (#792): route it through the guard, \
         or record in `SITES` why it cannot run after the mint lock on a profile not locked first"
    );
}
