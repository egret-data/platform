//! Per-workspace feature entitlements: the code-defined catalog of modules, the
//! request-path matcher that says which module a request belongs to, and the
//! `WorkspaceRepo` queries that read and write the on/off rows.
//!
//! An **absent row means enabled**. Only explicit `enabled = false` rows turn a
//! module off, so a fresh install and every pre-existing workspace behave
//! exactly as they did before this feature existed.
//!
//! # What this catalog does NOT cover
//!
//! The gate in `WsContext` matches on the request path, so it reaches a route
//! only if that route builds a `WsContext`. Two kernel-subprocess callbacks
//! never do — they are authenticated by an `EGRET_KERNEL_TOKEN` bearer with no
//! browser session — and so assert the entitlement by hand instead, against the
//! workspace their kernel token is bound to:
//!
//! - `notebook_runtime_token` (`POST /api/ws/{workspace}/notebook-runtime/token`)
//! - `engine_token` (`POST /api/kernel/engine-token`, not workspace-scoped by
//!   URL at all, so no catalog rule could match it)
//!
//! Both live in `egret-kernel`'s `routes.rs`. Auditing this feature by reading
//! the catalog alone would miss them, which is why they are named here. Anything
//! else that mints a capability without a `WsContext` belongs on this list.

use std::collections::HashSet;

use egret_core::{Error, Result, UserId, WorkspaceId};

use crate::workspaces::{db_err, WorkspaceRepo};

/// Every module key, in the order the UI lists them.
pub const FEATURE_KEYS: &[&str] = &[
    "sql_lab",
    "datasets",
    "charts",
    "dashboards",
    "notebooks",
    "alerts",
    "data_api",
];

/// True if `key` names a module in the catalog.
pub fn is_known(key: &str) -> bool {
    FEATURES.iter().any(|f| f.key == key)
}

/// Cross-module dependencies: `(dependent, requires, message)`. `dependent`
/// cannot be enabled while `requires` is disabled.
///
/// A dashboard still renders its saved cards through the `dashboards`-gated
/// surface, but its interactive filter dropdowns and edit-mode card previews
/// both run through `POST /charts/run`, which is `charts`-gated. `dashboards`
/// ON with `charts` OFF is therefore a broken configuration, not just an
/// unusual one, so it is rejected rather than merely discouraged.
///
/// Expressed as data so a future second dependency is a one-line addition to
/// this list, not new control flow in `violates_dependencies`.
const DEPENDENCIES: &[(&str, &str, &str)] = &[(
    "dashboards",
    "charts",
    "dashboards requires charts to be enabled",
)];

/// The message for the first violated dependency in `disabled` — the set of
/// feature keys that would be OFF in the resulting state — or `None` if none
/// are violated.
///
/// Takes the *resulting* disabled set, not a request body: callers must merge
/// their change onto the current state first, since a body naming only
/// `charts` can still produce a `dashboards`-on/`charts`-off result if
/// `dashboards` was already on.
pub fn violates_dependencies(disabled: &HashSet<String>) -> Option<&'static str> {
    DEPENDENCIES
        .iter()
        .find_map(|(dependent, requires, message)| {
            let dependent_enabled = !disabled.contains(*dependent);
            let requires_disabled = disabled.contains(*requires);
            (dependent_enabled && requires_disabled).then_some(*message)
        })
}

/// How a rule's `path` is compared against a request's path suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// The suffix equals `path`, or starts with `path` followed by `/`.
    Prefix,
    /// The suffix equals `path` exactly.
    Exact,
}

/// One gated HTTP surface under `/api/ws/{workspace}/`.
#[derive(Debug, Clone, Copy)]
pub struct GateRule {
    pub path: &'static str,
    pub kind: Match,
    /// Uppercase HTTP methods this rule covers; `None` = every method.
    pub methods: Option<&'static [&'static str]>,
}

/// A module a workspace can be entitled to.
#[derive(Debug, Clone, Copy)]
pub struct Feature {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub rules: &'static [GateRule],
}

const fn prefix(path: &'static str) -> GateRule {
    GateRule {
        path,
        kind: Match::Prefix,
        methods: None,
    }
}

pub const FEATURES: &[Feature] = &[
    Feature {
        key: "sql_lab",
        label: "SQL Lab",
        description: "Write and run free-form SQL, browse engine schemas, and save queries.",
        rules: &[
            prefix("sqllab"),
            // Submitting free-form SQL. Reading back, listing, and cancelling
            // stay open: the chart builder runs through those same paths.
            GateRule {
                path: "queries",
                kind: Match::Exact,
                methods: Some(&["POST"]),
            },
        ],
    },
    Feature {
        key: "datasets",
        label: "Datasets",
        description: "Model physical tables and SQL into reusable datasets with metrics.",
        rules: &[prefix("datasets")],
    },
    Feature {
        key: "charts",
        label: "Charts",
        description: "Build and view charts on top of datasets.",
        rules: &[prefix("charts")],
    },
    Feature {
        key: "dashboards",
        label: "Dashboards",
        description: "Arrange charts into dashboards with filters and tabs.",
        rules: &[prefix("dashboards")],
    },
    Feature {
        key: "notebooks",
        label: "Notebooks",
        description: "Python and SQL notebooks backed by a per-user kernel.",
        rules: &[
            prefix("notebooks"),
            prefix("kernel"),
            prefix("notebook-runtime"),
            prefix("notebook-settings"),
        ],
    },
    Feature {
        key: "alerts",
        label: "Alerts & Reports",
        description: "Schedule SQL alerts and recurring report deliveries.",
        // Claims `egret-automation`'s router (P1). Declared in P0, one phase
        // before the routes existed, because an unmatched prefix rule is inert
        // while a route that lands before its rule fails OPEN.
        rules: &[prefix("alerts")],
    },
    Feature {
        key: "data_api",
        label: "Data API",
        description: "Publish saved queries as HTTP endpoints, called by service accounts.",
        // Claims the Data API seam's management router (P2 Task 5): service
        // accounts and the OAuth2 clients that authenticate as them. Declared
        // with the credential plane rather than waiting for P3's endpoints, for
        // the same reason `alerts` was declared a phase early — an unmatched
        // prefix rule is inert, but a route that lands before its rule fails
        // OPEN, and this particular surface mints credentials.
        //
        // The SERVING surface (P4) is deliberately NOT covered: it lives at
        // `/data-api/v1/{ws}/{path}`, outside `/api/ws/`, and authenticates with
        // a token rather than a session. Turning the module off stops new
        // credentials being minted here; stopping traffic is `state='disabled'`
        // on the endpoint (spec §5.3).
        rules: &[prefix("data-api")],
    },
];

/// Workspace-scoped first path segments that are deliberately never gated.
///
/// `folders` and `objects` are content-type-neutral (a folder holds charts *and*
/// notebooks; `objects` is the sharing surface) and are already authorized
/// per-object by the PDP. The rest are core workspace-management surfaces that
/// belong to no single module, and so must not be held hostage to any module's
/// on/off state.
pub const UNGATED: &[&str] = &[
    "access-requests",
    "databases",
    "engines",
    "folders",
    // Starting an impersonation is a ws_admin support action, not a data module —
    // it stays reachable regardless of which feature modules a workspace has.
    "impersonate",
    "members",
    // Per-workspace SMTP. Deliberately NOT claimed by `alerts`, even though
    // alerts are its only consumer today: it is workspace-level configuration
    // reached from Workspace settings, authorized by `workspace:manage_members`
    // rather than by a module entitlement, and P4's reports will send through
    // the same relay. Gating it on `alerts` would mean an admin could not
    // prepare (or clean up) mail settings for a module they had switched off.
    "notifications",
    "objects",
    "roles",
    "shares",
];

/// True if `segment` — the first path segment under `/api/ws/{workspace}/` — is
/// something the catalog knows about: either a module claims it, or it is
/// explicitly ungated.
///
/// Note this is deliberately coarser than `gated_feature`. A segment can be
/// *known* while only some of its paths are *gated* — `queries` is claimed by
/// `sql_lab` for POST only, because submitting free-form SQL is the privileged
/// act while reading back or cancelling your own query is not. The method-level
/// semantics of such a rule are pinned by this module's own unit tests; this
/// function exists for the route-coverage test in `egret-server`, whose job is
/// to catch a whole module being mounted with nobody classifying it.
pub fn is_classified_segment(segment: &str) -> bool {
    UNGATED.contains(&segment)
        || FEATURES.iter().any(|f| {
            f.rules
                .iter()
                .any(|r| r.path.split('/').next() == Some(segment))
        })
}

/// The part of a request path after `/api/ws/{slug}/`, or `None` if the path is
/// not workspace-scoped.
///
/// Accepts the path with or without the leading `/api`: axum's `nest("/api", …)`
/// strips the prefix from the inner router's view of the URI, and we want the
/// same answer either way.
pub fn ws_path_suffix(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api").unwrap_or(path);
    let rest = rest.strip_prefix("/ws/")?;
    let (_slug, suffix) = rest.split_once('/')?;
    if suffix.is_empty() {
        return None;
    }
    Some(suffix)
}

/// The feature key gating `suffix` for `method`, or `None` when the surface is
/// ungated. `method` is the uppercase HTTP method (`parts.method.as_str()`).
///
/// When several rules match, the one with the longest `path` wins, so a specific
/// sub-path can override a broader prefix without reordering the catalog.
///
/// An unmatched path deliberately returns `None` (fail-open): a route nobody
/// classified must not be blocked in production. Task 3's OpenAPI coverage
/// test is what catches a catalog omission, at CI time rather than at request
/// time.
pub fn gated_feature(suffix: &str, method: &str) -> Option<&'static str> {
    gated_feature_in(FEATURES, suffix, method)
}

/// Same as [`gated_feature`], but over a caller-supplied catalog. Split out
/// purely so tests can exercise the matching logic (in particular the
/// longest-prefix tie-break) against small, deliberately overlapping
/// catalogs, without the production `FEATURES` list needing an overlap of
/// its own just to get coverage.
fn gated_feature_in(features: &[Feature], suffix: &str, method: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for feature in features {
        for rule in feature.rules {
            let path_matches = match rule.kind {
                Match::Exact => suffix == rule.path,
                Match::Prefix => {
                    suffix == rule.path
                        || (suffix.len() > rule.path.len()
                            && suffix.starts_with(rule.path)
                            && suffix.as_bytes()[rule.path.len()] == b'/')
                }
            };
            if !path_matches {
                continue;
            }
            if let Some(methods) = rule.methods {
                if !methods.contains(&method) {
                    continue;
                }
            }
            // Written as a match rather than `map_or` so clippy's pedantic
            // lints (CI runs with `-D warnings`) have nothing to say.
            match best {
                Some((len, _)) if rule.path.len() <= len => {}
                _ => best = Some((rule.path.len(), feature.key)),
            }
        }
    }
    best.map(|(_, key)| key)
}

impl WorkspaceRepo {
    /// The module keys explicitly turned OFF for `ws`. Everything not in the
    /// returned set is on.
    pub async fn disabled_features(&self, ws: WorkspaceId) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT feature FROM workspace_features \
             WHERE workspace_id = $1 AND enabled = false",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(f,)| f).collect())
    }

    /// Disabled feature keys for several workspaces at once, keyed by workspace
    /// id. One query for a whole list — the workspace switcher must not issue a
    /// query per workspace.
    pub async fn disabled_features_for(
        &self,
        ws_ids: &[uuid::Uuid],
    ) -> Result<std::collections::HashMap<uuid::Uuid, HashSet<String>>> {
        let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT workspace_id, feature FROM workspace_features \
             WHERE enabled = false AND workspace_id = ANY($1)",
        )
        .bind(ws_ids)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        let mut out: std::collections::HashMap<uuid::Uuid, HashSet<String>> =
            std::collections::HashMap::new();
        for (ws, feature) in rows {
            out.entry(ws).or_default().insert(feature);
        }
        Ok(out)
    }

    /// Upsert `updates` for `ws` in one transaction. Rejects unknown keys, and
    /// a resulting state that violates a cross-module dependency (see
    /// [`violates_dependencies`]), before writing anything, so a batch
    /// containing a typo — or one that merges onto the current state into a
    /// broken combination — changes nothing.
    pub async fn set_features(
        &self,
        ws: WorkspaceId,
        updates: &[(String, bool)],
        actor: Option<UserId>,
    ) -> Result<()> {
        for (key, _) in updates {
            if !is_known(key) {
                return Err(Error::InvalidInput(format!("unknown feature: {key}")));
            }
        }

        // `PUT` merges: the body may name only a subset of keys, so the
        // dependency rule must be checked against the state that would
        // *result* from applying `updates` onto what is currently stored, not
        // against `updates` in isolation. Read this before opening the
        // transaction so a violation is rejected before any write, mirroring
        // the unknown-key check above.
        let mut resulting = self.disabled_features(ws).await?;
        for (key, enabled) in updates {
            if *enabled {
                resulting.remove(key);
            } else {
                resulting.insert(key.clone());
            }
        }
        if let Some(message) = violates_dependencies(&resulting) {
            return Err(Error::InvalidInput(message.to_string()));
        }

        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        for (key, enabled) in updates {
            sqlx::query(
                "INSERT INTO workspace_features (workspace_id, feature, enabled, updated_by) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (workspace_id, feature) DO UPDATE \
                 SET enabled = EXCLUDED.enabled, \
                     updated_at = now(), \
                     updated_by = EXCLUDED.updated_by",
            )
            .bind(ws.0)
            .bind(key)
            .bind(enabled)
            .bind(actor.map(|u| u.0))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_is_extracted_with_and_without_the_api_prefix() {
        // axum's `nest("/api", …)` strips the prefix from the inner router's
        // view of the URI, so the gate must accept both spellings.
        assert_eq!(ws_path_suffix("/api/ws/sales/notebooks"), Some("notebooks"));
        assert_eq!(
            ws_path_suffix("/ws/sales/notebooks/abc"),
            Some("notebooks/abc")
        );
        assert_eq!(ws_path_suffix("/api/auth/login"), None);
        assert_eq!(ws_path_suffix("/ws/sales"), None);
    }

    #[test]
    fn module_prefixes_map_to_their_feature() {
        assert_eq!(gated_feature("notebooks", "GET"), Some("notebooks"));
        assert_eq!(
            gated_feature("notebooks/abc/cells", "POST"),
            Some("notebooks")
        );
        assert_eq!(gated_feature("kernel", "POST"), Some("notebooks"));
        assert_eq!(
            gated_feature("notebook-runtime/token", "POST"),
            Some("notebooks")
        );
        assert_eq!(gated_feature("charts/abc/data", "POST"), Some("charts"));
        assert_eq!(gated_feature("dashboards", "GET"), Some("dashboards"));
        assert_eq!(
            gated_feature("datasets/abc/columns", "GET"),
            Some("datasets")
        );
    }

    #[test]
    fn alerts_is_a_module_that_gates_its_own_surface() {
        // The catalog is what makes `alerts` reach the frontend at all: the
        // Explore nav filters on the keys `/me/workspaces` returns, which are
        // built from FEATURE_KEYS. A frontend-only move leaves the item
        // invisible in the browser while jsdom tests still pass.
        assert!(is_known("alerts"));
        assert!(FEATURE_KEYS.contains(&"alerts"));
        assert_eq!(gated_feature("alerts", "GET"), Some("alerts"));
        assert_eq!(gated_feature("alerts/abc/runs", "GET"), Some("alerts"));
    }

    #[test]
    fn sql_lab_gates_its_own_surface_and_only_free_form_submits() {
        assert_eq!(gated_feature("sqllab/queries", "GET"), Some("sql_lab"));
        assert_eq!(
            gated_feature("sqllab/databases/x/tables", "GET"),
            Some("sql_lab")
        );
        // Submitting free-form SQL is the privileged act...
        assert_eq!(gated_feature("queries", "POST"), Some("sql_lab"));
        // ...reading back or cancelling a query you already own is not, and
        // charts depend on those paths.
        assert_eq!(gated_feature("queries", "GET"), None);
        assert_eq!(gated_feature("queries/abc", "GET"), None);
        assert_eq!(gated_feature("queries/abc/results", "GET"), None);
        assert_eq!(gated_feature("queries/abc/cancel", "POST"), None);
    }

    #[test]
    fn content_neutral_and_admin_surfaces_are_never_gated() {
        for suffix in [
            "members",
            "roles",
            "shares",
            "objects",
            "folders/abc",
            "engines",
            "databases",
            "access-requests",
            // Workspace SMTP: reached from Workspace settings and authorized by
            // `workspace:manage_members`, so it must stay reachable with every
            // module switched off.
            "notifications/smtp",
        ] {
            assert_eq!(
                gated_feature(suffix, "GET"),
                None,
                "{suffix} must not be gated"
            );
        }
    }

    #[test]
    fn every_first_segment_is_classified_exactly_once() {
        // A segment claimed by a Feature must not also be listed as ungated,
        // or the coverage test in egret-server would pass while the runtime
        // behaviour is ambiguous.
        for f in FEATURES {
            for rule in f.rules {
                let seg = rule.path.split('/').next().unwrap();
                assert!(
                    !UNGATED.contains(&seg),
                    "{seg} is claimed by {} and also listed as ungated",
                    f.key
                );
            }
        }
    }

    #[test]
    fn catalog_keys_match_the_key_list() {
        let from_catalog: Vec<&str> = FEATURES.iter().map(|f| f.key).collect();
        assert_eq!(from_catalog, FEATURE_KEYS);
    }

    #[test]
    fn an_unclassified_path_is_allowed_not_denied() {
        // Deliberate fail-OPEN: a route nobody classified must not 403 in
        // production. Task 3's coverage test over the OpenAPI document is what
        // catches the omission, at CI time rather than at request time.
        assert_eq!(gated_feature("something-nobody-declared", "GET"), None);
        assert_eq!(gated_feature("something-nobody-declared/sub", "POST"), None);
    }

    #[test]
    fn is_classified_segment_covers_module_and_ungated_segments_but_not_the_unknown() {
        // Claimed by a Feature (via its rules' first path segment).
        for seg in ["queries", "sqllab", "notebooks", "kernel"] {
            assert!(is_classified_segment(seg), "{seg} should be classified");
        }
        // Explicitly listed as ungated.
        for seg in ["members", "folders"] {
            assert!(is_classified_segment(seg), "{seg} should be classified");
        }
        // Nobody has declared this segment at all.
        assert!(!is_classified_segment("pipelines"));
    }

    #[test]
    fn dependency_rule_covers_all_four_charts_dashboards_combinations() {
        let disabled =
            |keys: &[&str]| -> HashSet<String> { keys.iter().map(|k| k.to_string()).collect() };

        // charts on, dashboards on: fine.
        assert_eq!(violates_dependencies(&disabled(&[])), None);
        // charts on, dashboards off: fine (dashboards may be off regardless).
        assert_eq!(violates_dependencies(&disabled(&["dashboards"])), None);
        // charts off, dashboards off: fine (turning both off together).
        assert_eq!(
            violates_dependencies(&disabled(&["charts", "dashboards"])),
            None
        );
        // charts off, dashboards on: the broken combination.
        assert_eq!(
            violates_dependencies(&disabled(&["charts"])),
            Some("dashboards requires charts to be enabled")
        );
    }

    #[test]
    fn longest_matching_prefix_wins() {
        // The production catalog has no two rules sharing a first segment, so
        // this exercises the tie-break against a small local catalog that
        // deliberately overlaps: "a/b" is more specific than "a" and must win.
        const OVERLAPPING: &[Feature] = &[
            Feature {
                key: "broad",
                label: "Broad",
                description: "",
                rules: &[prefix("a")],
            },
            Feature {
                key: "narrow",
                label: "Narrow",
                description: "",
                rules: &[prefix("a/b")],
            },
        ];
        assert_eq!(
            gated_feature_in(OVERLAPPING, "a/b/c", "GET"),
            Some("narrow")
        );
        assert_eq!(gated_feature_in(OVERLAPPING, "a/x", "GET"), Some("broad"));
    }
}
