//! Reverse drift lint: every key in `egret_core::rbac::PERMISSIONS` must be
//! enforced by SOME call site in the running code, not just declared in the
//! catalog. This is a pure source-text scan (no DB, runs in CI) that catches
//! the class of bug this branch fixes — a catalog entry with zero
//! `require_permission`/`require_platform` call sites (e.g. the old
//! `database:*` / dead `workspace:*` false alarm).
//!
//! Most control-plane routes gate on `egret_controlplane::auth::require_permission`.
//! The `secret:*` keys are enforced by the PDP-backed
//! `egret_core::authz::require(state.pdp, ..)` PEP in `secrets.rs` (B1b-1 Task 4).
//! `workspace:*` keys are the one exception: they're enforced by the sibling
//! `egret_workspace::rbac::Authorizer::require_platform` in
//! `egret-workspace/src/routes.rs`, so that file is scanned too. `engine:*`
//! keys are enforced by `egret_auth::require_permission` in the
//! `egret-engine-mgmt` seam's `engines.rs`/`monitor.rs` (E1c), `policy:*` keys in
//! the `egret-policy` seam's `admin/mod.rs` (E2d), and `catalog_registry:*` keys
//! in both the engine seam's `catalogs.rs` and the `egret-catalog-engine` seam's
//! `warehouse_admin/mod.rs` (E3d-3) — all scanned. `warehouse_admin/mod.rs` was
//! `warehouse:*`-gated through SP3c-1; SP3c-2 retired that family onto
//! `catalog_registry:*` (migration 0153), so it no longer names a distinct key.
//!
//! Workspace-feature keys (`query:run`, `dataset:*`, `chart:*`, `dashboard:*`,
//! `workspace:manage_members`) are enforced by `egret_core::authz::require`/
//! `require_object` in the feature-route crates (`egret-bi`, `egret-query`).
//! Those routes usually pass a `const NAME: &str = "resource:action"` rather
//! than an inline literal, so the scan resolves const identifiers referenced
//! on a gate-call line back to their defining literal (see `enforced_in_file`).

use std::collections::HashSet;

/// Catalog keys intentionally not enforced anywhere yet. Must stay empty —
/// if you're tempted to add to this list, either wire up enforcement or
/// remove the catalog entry (see `egret_core::rbac::PERMISSIONS` doc comment).
const ALLOWED_UNENFORCED: &[&str] = &[];

/// Control-plane source files scanned for `require_permission(..., "key")`
/// call sites, relative to this crate's `src/`.
///
/// `tag:read`/`tag:manage` (Access P3.1) are enforced in `tags.rs` via
/// `authz::require`. Per-object tag assignment has no dedicated `tag:assign`
/// perm — it reuses object-manage (`require_object(manage_action(kind))`) in
/// `egret-bi/src/shares.rs`, so there is no catalog key to enforce there.
const CONTROLPLANE_FILES: &[&str] = &["storage.rs", "secrets.rs", "access/mod.rs", "tags.rs"];

/// Extra source files (outside this crate) that enforce catalog permissions
/// through a different gate function. Path is relative to this crate's
/// `CARGO_MANIFEST_DIR`.
const EXTRA_FILES: &[&str] = &[
    "../egret-workspace/src/routes.rs",
    "../egret-engine-mgmt/src/engines.rs",
    "../egret-engine-mgmt/src/monitor.rs",
    // `catalog_registry:*` (SP3) are gated in the engine seam's registry routes.
    "../egret-engine-mgmt/src/catalogs.rs",
    "../egret-policy/src/admin/mod.rs",
    // Warehouse admin router (E3d-3), in the Catalog seam. Its own permission
    // family (`warehouse:*`) was retired in SP3c-2; it now enforces
    // `catalog_registry:*`, the same keys `catalogs.rs` above already gates.
    "../egret-catalog-engine/src/warehouse_admin/mod.rs",
    // Workspace-feature keys are gated via `authz::require`/`require_object`
    // in the feature-route crates (mostly via `const` args, resolved below).
    "../egret-bi/src/routes.rs",
    // `alert:manage` is gated via `authz::require` in the Alerts & Reports seam.
    "../egret-automation/src/routes.rs",
    "../egret-query/src/execution/routes.rs",
    "../egret-query/src/registry/routes.rs",
    // `service_account:*` (Data API P2) is gated via `authz::require` in the
    // Data API seam's management router, using the `SA_READ`/`SA_MANAGE` consts
    // the scan resolves below.
    "../egret-dataapi/src/routes.rs",
    // `data_api_endpoint:invoke` is gated on the SERVING path, which has a JWT
    // principal rather than a `WorkspaceContext` and so calls `Pdp::authorize`
    // directly. `can_invoke` is that call and is the one thing in this file; see
    // `GATE_FNS`.
    "../egret-dataapi/src/invoke.rs",
    // `workflow:read`/`workflow:manage` (Workflow W1 Task 7) and
    // `workflow:use_secrets` (W5b fix round 1) are gated via `authz::require`
    // in the Workflow seam's management router, using the
    // `WF_READ`/`WF_MANAGE`/`WF_USE_SECRETS` consts the scan resolves below.
    "../egret-workflow/src/routes.rs",
    // `app:read` (run mode) and `app:write` (edit mode) are gated on the Data
    // Apps query gateway via `authz::require` with the `APP_READ`/`APP_WRITE`
    // consts. The caller holds an app-session bearer token, not a
    // `WorkspaceContext`, so the gateway passes the core PEP its own
    // `AuthTarget` rather than calling `Pdp::authorize` by hand.
    "../egret-apps/src/gateway.rs",
    // `app:read` / `app:write` (Data Apps P2) on the cookie-session app API:
    // `authz::require` for list/create and `require_object` on every `{id}` route,
    // with the same `APP_READ`/`APP_WRITE` consts.
    "../egret-apps/src/routes.rs",
    // `app:manage` (Data Apps P2) is gated by the object-share API through
    // `require_object(manage_action(kind))`; `manage_action` is a literal table
    // of every share kind's manage key, listed in `PERM_TABLE_FNS`.
    "../egret-bi/src/shares.rs",
];

/// Gate function names whose permission-key argument (an inline
/// `resource:action` literal, or a `const` identifier that resolves to one) is
/// a catalog permission key.
const GATE_CALLS: &[&str] = &[
    "require_permission(",
    "require_platform(",
    "require_workspace(",
    // PDP-backed policy enforcement point (B1b-1): `secret:*` gates migrated
    // from `require_permission` to `egret_core::authz::require(state.pdp, ..)`.
    "authz::require(",
    // Object-scope PEP (B2-1a): per-object BI handlers gate on
    // `egret_core::authz::require_object(state.pdp, ctx, PERM, obj)` — e.g.
    // `dataset:manage` on the dataset `{id}` handlers moved off `authz::require`.
    "require_object(",
];

/// Functions that exist ONLY to map a route parameter to the permission keys a
/// gate call then uses, so the literal never appears on a gate line.
///
/// The generic folder router is the case: `kind_perms("alerts")` returns
/// `("alert", "alert:read", "alert:manage")`, and the handler gates with
/// `authz::require(&*st.pdp, &ctx, read)` — a variable. Every literal in such a
/// function is a key that IS enforced; without this the scan reports
/// `alert:read` (and would report any future kind added the same way) as
/// enforced nowhere, which is a false alarm on a test whose whole value is that
/// it does not cry wolf.
///
/// Deliberately a NAMED list rather than "any tuple of permission literals":
/// the point of the test is that a key nobody gates is caught, and a rule broad
/// enough to match any table would let a genuinely dead key hide in one.
///
/// `manage_action` in `egret-bi/src/shares.rs` is the second case: the share and
/// tag handlers gate with `require_object(.., manage_action_for(&kind)?, obj)`, so
/// `app:manage` never appears on a gate line.
const PERM_TABLE_FNS: &[&str] = &["fn kind_perms(", "fn manage_action(", "fn read_action("];

/// Functions whose whole body IS the gate, so the permission is named inside
/// them rather than on the line that calls the gate.
///
/// `egret-dataapi`'s `can_invoke` is the case, and it is a deliberate seam
/// rather than an oversight. The serving path holds a `ServingPrincipal` — three
/// fields read out of a JWT — not the `WorkspaceContext` every `authz::require*`
/// wrapper takes, so it calls `Pdp::authorize` directly, which IS the decision
/// point the wrappers are a convenience over (`serving/router.rs`'s
/// `invoke_allowed` states the reasoning at length). Two separate properties of
/// that call defeat the line-at-a-time scan below: it spans six lines, so the
/// `.authorize(` and the action never share a line, and it names the action as
/// `crate::routes::EP_INVOKE`, a const defined in a DIFFERENT file.
///
/// The consequence was not theoretical. `data_api_endpoint:invoke` read as
/// enforced nowhere while `serving_authz.rs` was proving, against a live PDP,
/// that it is checked on every served request — so this test was red on `main`
/// for the whole Data API programme, and stayed invisible because
/// `cargo test --workspace` stops at the first failing binary and one in
/// `egret-automation` (alphabetically earlier) was failing ahead of it.
///
/// Named, like `PERM_TABLE_FNS` and for the same reason: "any function that
/// calls `authorize`" would let a genuinely dead key hide behind any PDP call.
const GATE_FNS: &[&str] = &["fn can_invoke("];

/// Permission keys named inside the body of one of `fns` — inline literals, and
/// `const` identifiers resolved through `consts`.
///
/// The body is taken from the `fn` line to the first line that closes it at
/// column 0 (`}`), which is how every function in these files is formatted.
fn fn_body_keys(src: &str, fns: &[&str], consts: &[(String, String)]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut inside = false;
    for line in src.lines() {
        if !inside {
            inside = fns.iter().any(|f| line.contains(f));
            continue;
        }
        if line == "}" {
            inside = false;
            continue;
        }
        out.extend(
            line.split('"')
                .skip(1)
                .step_by(2)
                .filter(|s| is_resource_action_shaped(s))
                .map(str::to_string),
        );
        for (ident, lit) in consts {
            if references_ident_as_word(line, ident) {
                out.insert(lit.clone());
            }
        }
    }
    out
}

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

/// Parse a `const IDENT: &str = "resource:action";` definition, returning
/// `(IDENT, "resource:action")` when the literal is permission-shaped. This lets
/// gate calls that pass a const (rather than an inline literal) still resolve.
///
/// A VISIBILITY MODIFIER IS SKIPPED, and that was not cosmetic: this used to
/// require the line to START with `const`, so `pub(crate) const EP_INVOKE` was
/// not a const definition as far as the scan was concerned and every gate that
/// named it resolved to nothing. A const's visibility says who may read it, and
/// has nothing to do with whether the key it holds is enforced.
fn perm_const_def(line: &str) -> Option<(String, String)> {
    let decl = line.trim();
    let decl = decl.strip_prefix("pub ").unwrap_or(decl);
    let decl = match decl.find(") ") {
        // `pub(crate) `, `pub(super) `, `pub(in path) ` — take what follows.
        Some(end) if decl.starts_with("pub(") => &decl[end + 2..],
        _ => decl,
    };
    let rest = decl.strip_prefix("const ")?;
    let (ident, _) = rest.split_once(':')?;
    let ident = ident.trim();
    if ident.is_empty()
        || !ident
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let lit = rest.split('"').nth(1)?;
    is_resource_action_shaped(lit).then(|| (ident.to_string(), lit.to_string()))
}

/// True if `ident` appears as a whole-word token in `line` (so `PERM` does not
/// match inside `PERM_MANAGE`).
fn references_ident_as_word(line: &str, ident: &str) -> bool {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(pos) = line[from..].find(ident) {
        let start = from + pos;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_word(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Every `resource:action` key enforced within a single source file: inline
/// literals passed to a gate call, plus `const` identifiers referenced on a
/// gate-call line (or inside a `GATE_FNS`/`PERM_TABLE_FNS` body) resolved back
/// to their defining literal.
///
/// `consts` is the WHOLE scanned tree's const table, not this file's. A const
/// and the gate that uses it are allowed to live apart — `EP_INVOKE` is declared
/// in `egret-dataapi/src/routes.rs` beside its two siblings, precisely so the
/// three action strings are one list, and used in `invoke.rs` — and a
/// per-file table cannot see across that. It stays narrow because a const is
/// only ever resolved when its identifier is referenced ON a gate line or inside
/// a named gate function, never merely because it exists.
fn enforced_in_file(src: &str, consts: &[(String, String)]) -> HashSet<String> {
    let mut out = fn_body_keys(src, PERM_TABLE_FNS, consts);
    out.extend(fn_body_keys(src, GATE_FNS, consts));
    for line in src.lines() {
        if !GATE_CALLS.iter().any(|g| line.contains(g)) {
            continue;
        }
        out.extend(resource_action_literals_in_line(line));
        for (ident, lit) in consts {
            if references_ident_as_word(line, ident) {
                out.insert(lit.clone());
            }
        }
    }
    out
}

/// Absolute paths of every file the scan reads, in the two flavours of relative
/// root the two lists use.
fn scanned_paths() -> Vec<String> {
    let cp = CONTROLPLANE_FILES
        .iter()
        .map(|rel| format!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/{}"), rel));
    let extra = EXTRA_FILES
        .iter()
        .map(|rel| format!(concat!(env!("CARGO_MANIFEST_DIR"), "/{}"), rel));
    cp.chain(extra).collect()
}

/// Every `resource:action` key enforced somewhere in the scanned source tree.
///
/// TWO PASSES, because const definitions and the gates that use them cross file
/// boundaries: collect the const table from every scanned file first, then scan
/// each file for gates against that whole table.
fn enforced_keys() -> HashSet<String> {
    let sources: Vec<String> = scanned_paths()
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
        })
        .collect();
    let consts: Vec<(String, String)> = sources
        .iter()
        .flat_map(|src| src.lines().filter_map(perm_const_def))
        .collect();
    sources
        .iter()
        .flat_map(|src| enforced_in_file(src, &consts))
        .collect()
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

/// Keys that are enforced and CAN be held, without being in `PERMISSIONS`. Each
/// needs its reason here; "it failed the test" is not one.
///
/// - `catalog:read`: the workspace-scope key from migration 0024, held by every
///   workspace role and gated in egret-query's registry routes. It is kept out of
///   the catalog on purpose -- `egret_core::rbac` pins `!is_known("catalog:read")`
///   so it cannot be confused with the platform `catalog_registry:*` family.
const HELD_OUTSIDE_THE_CATALOG: &[&str] = &["catalog:read"];

/// The other direction: every key the code ENFORCES must be one somebody can hold.
///
/// A gate on a key no role can be granted cannot be satisfied, so the route
/// behind it refuses everyone. `folder:manage` was exactly that for the whole life
/// of folder sharing -- `shares.rs` asked for it, it was in no catalog and no
/// migration, and workspace admins got 403. The scan above could not see it,
/// because it only asks whether each catalog key has a gate.
///
/// "Can hold" is three sets: the catalog, the admin-graph actions (not grantable,
/// but the PDP permits them to `platform_admin`), and the named exceptions above.
/// Same scan, same files, read the other way.
#[test]
fn every_enforced_key_can_be_held_by_someone() {
    let holdable: HashSet<&str> = egret_core::rbac::PERMISSIONS
        .iter()
        .map(|p| p.key)
        .chain(egret_core::rbac::ADMIN_GRAPH_ACTIONS.iter().copied())
        .chain(HELD_OUTSIDE_THE_CATALOG.iter().copied())
        .collect();
    let mut unknown: Vec<String> = enforced_keys()
        .into_iter()
        .filter(|k| !holdable.contains(k.as_str()))
        .collect();
    unknown.sort();
    assert!(
        unknown.is_empty(),
        "gate(s) on a key nobody can hold -- not in egret_core::rbac::PERMISSIONS, not an \
         admin-graph action -- so the route refuses everyone: {unknown:?}. Add the key to the \
         catalog and grant it in a migration, or gate on a key that exists."
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
        "catalog_registry:read",
        // Workspace-feature keys resolved through a `const` arg (the other
        // five above are inline literals, already exercising that path).
        "query:run",
        "dashboard:read",
    ] {
        assert!(
            enforced.contains(key),
            "expected the source scan to find a call site for {key}"
        );
    }
}
