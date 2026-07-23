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
pub const FEATURE_KEYS: &[&str] = &["sql_lab", "datasets", "charts", "dashboards", "notebooks"];

/// True if `key` names a module in the catalog.
pub fn is_known(key: &str) -> bool {
    FEATURES.iter().any(|f| f.key == key)
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
    "members",
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

    /// Upsert `updates` for `ws` in one transaction. Rejects unknown keys before
    /// writing anything, so a batch containing a typo changes nothing.
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
