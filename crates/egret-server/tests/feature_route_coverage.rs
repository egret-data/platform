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
    // roughly 75 workspace-scoped paths in the document today; a floor of 50
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
