//! Every workspace-scoped path segment must be known to the feature catalog:
//! either a module claims it, or it is explicitly listed as ungated.
//!
//! The gate lives in one place (`WsContext`), which only works if the catalog
//! knows about every route. This test is what keeps that true — mount a new
//! module under `/api/ws/{workspace}/` without declaring it here and this
//! fails. Classification is checked at first-segment granularity (see
//! `is_classified_segment`'s doc comment for why): the risk this test defends
//! against is a whole module's routes going unclassified, not sub-path drift
//! within an already-classified segment — that narrower risk has its own
//! dedicated unit tests in `egret-workspace`'s `features` module (e.g.
//! `sql_lab_gates_its_own_surface_and_only_free_form_submits` pins exactly
//! which `queries` paths are and are not gated).
//!
//! No database and no network: it reads the in-process OpenAPI document.

use std::collections::BTreeSet;

use egret_workspace::features::{is_classified_segment, ws_path_suffix};

#[test]
fn every_workspace_scoped_route_is_classified() {
    let doc = egret_server::openapi_doc();

    let mut unclassified = BTreeSet::new();
    let mut ws_scoped_paths_seen = 0usize;
    for path in doc.paths.paths.keys() {
        let Some(suffix) = ws_path_suffix(path) else {
            continue;
        };
        ws_scoped_paths_seen += 1;
        let first = suffix.split('/').next().unwrap();
        if !is_classified_segment(first) {
            unclassified.insert(first.to_string());
        }
    }

    // Guard against a vacuous pass: if `ws_path_suffix` regressed to matching
    // nothing (or the OpenAPI document shrank), `unclassified` would be empty
    // for the wrong reason and this test would prove nothing. There are
    // 79 workspace-scoped paths in the document today; a floor of 50
    // is conservative enough to tolerate normal route churn while still
    // failing loudly if extraction silently breaks.
    assert!(
        ws_scoped_paths_seen >= 50,
        "only saw {ws_scoped_paths_seen} workspace-scoped paths in the OpenAPI document, \
         expected at least 50 — either the document shrank or `ws_path_suffix` stopped matching"
    );

    assert!(
        unclassified.is_empty(),
        "these workspace-scoped path segments are known to neither a Feature nor UNGATED: {unclassified:?}\n\
         Add them to crates/egret-workspace/src/features.rs — either to the module that owns them, \
         or to UNGATED if they must stay reachable when every module is off."
    );
}

/// The gate's other, quieter precondition: it can only recognise a path that
/// begins `/api/ws/`.
///
/// `WsContext` reads `OriginalUri`, so the answer no longer depends on how
/// deeply a router is nested — but `ws_path_suffix` still accepts only
/// `/api/ws/{slug}/…` (or the bare `/ws/{slug}/…` an unnested router produces).
/// A workspace-scoped route mounted anywhere else — `/api/v2/ws/{workspace}/…`,
/// say — parses as nothing, is matched by no `GateRule`, and fails OPEN, while
/// `every_workspace_scoped_route_is_classified` above keeps passing because the
/// catalog still classifies the segment. That combination is exactly how a
/// silent hole would get in, so the convention is asserted rather than assumed.
#[test]
fn every_workspace_scoped_route_is_mounted_under_api_ws() {
    let doc = egret_server::openapi_doc();

    let mut misplaced = BTreeSet::new();
    let mut seen = 0usize;
    for path in doc.paths.paths.keys() {
        if !path.contains("{workspace}") {
            continue;
        }
        seen += 1;
        if !path.starts_with("/api/ws/") {
            misplaced.insert(path.clone());
        }
    }

    // Same anti-vacuity floor as the test above: if the document shrank or the
    // `{workspace}` parameter were renamed, `misplaced` would be empty for the
    // wrong reason.
    assert!(
        seen >= 50,
        "only saw {seen} paths carrying a {{workspace}} parameter, expected at least 50 — \
         either the OpenAPI document shrank or the parameter was renamed"
    );

    assert!(
        misplaced.is_empty(),
        "these workspace-scoped paths are not mounted under /api/ws/: {misplaced:?}\n\
         The feature gate in WsContext recognises a workspace-scoped path only by its \
         /api/ws/ (or /ws/) prefix, so a route mounted elsewhere is NOT gated — it fails open \
         no matter what the feature catalog says about it. Either mount it under /api/ws/, \
         or widen ws_path_suffix in crates/egret-workspace/src/features.rs to accept the new \
         shape and add a case to its unit tests."
    );
}
