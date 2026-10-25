//! D2 (StarRocks whole-table classification, spec §5.5): recompile the bound
//! StarRocks profiles when a table that carries a whole-table classification
//! changes schema.
//!
//! # Why
//!
//! On StarRocks a classification on a whole table compiles one `column: *`
//! deny, which covers columns added later, and one NULL mask per column of the
//! table's CURRENT schema ([`egret_perm_compile::table_columns`]). The mask is
//! what holds through a view: the live spike measured that the deny alone lets
//! a view read real values. So when the columns change, the masks must follow
//! at once (user decision U2), and no other egret code runs when an Iceberg
//! commit changes a table.
//!
//! # Where it runs
//!
//! An axum layer in front of lakekeeper, inside egret's
//! [`crate::mount::CatalogGuard`], beside the view hold. It sees:
//!
//! | request | path | counts when |
//! |---|---|---|
//! | create | `POST /{prefix}/namespaces/{ns}/tables` | always |
//! | register | `POST /{prefix}/namespaces/{ns}/register` | always |
//! | update | `POST /{prefix}/namespaces/{ns}/tables/{table}` | `add-schema` or `set-current-schema` |
//! | multi-table commit | `POST /{prefix}/transactions/commit` | any table change as for update |
//! | rename | `POST /{prefix}/tables/rename` | always |
//! | drop | `DELETE /{prefix}/namespaces/{ns}/tables/{table}` | always |
//! | drop namespace | `DELETE /{prefix}/namespaces/{ns}` | always |
//!
//! Every other request passes untouched, and an update with no schema change
//! (an append) passes with no database query at all (C12).
//!
//! # Order of operations
//!
//! 1. Resolve the prefix to the egret warehouse; find the StarRocks profiles
//!    bound to it, if the table (old or new name) carries a whole-table
//!    attachment of a classification with a role-bearing clearance
//!    ([`egret_perm_compile::affected_starrocks_profiles_for_tables`]). None:
//!    pass.
//! 2. Forward to lakekeeper. A non-2xx answer is returned as it is.
//! 3. After a 2xx, recompile each affected profile in its own transaction,
//!    bounded by `lock_timeout`, before the response is returned. The
//!    Iceberg commit has happened and cannot be refused any more (C10): a
//!    failure is logged and recorded in `perm.compile_runs` (the Governance
//!    Overview shows it), the boot pass retries it, and the 2xx passes. Until
//!    the recompile lands, the `*` deny refuses an uncleared direct read of a
//!    new column; only its view protection waits.
//!
//! The hook does not move a table's classifications on a rename or a drop
//! (spec §11.1): after a rename, the recompile reports the old name as not a
//! table in the catalog and the new name carries no classification.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use egret_metastore::Db;
use serde_json::Value;
use tracing::Instrument as _;

use crate::view_hold::{decode_segment, is_multi_level, rename_of, resolve};

/// Largest table request body the hook reads.
const MAX_BODY: usize = 16 << 20;

/// How long one profile's recompile may wait for a lock.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// The hook (module doc). Install it with
/// [`crate::mount::mount_catalog_engine`], or around a router with [`layer`].
#[derive(Clone)]
pub struct TableHook {
    db: Db,
    lock_wait: Duration,
}

impl TableHook {
    /// A hook whose recompiles wait at most [`DEFAULT_LOCK_WAIT`] for a lock.
    pub fn new(db: Db) -> Self {
        Self {
            db,
            lock_wait: DEFAULT_LOCK_WAIT,
        }
    }

    /// The same hook with another lock wait (tests).
    #[must_use]
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }
}

/// `inner` with the hook in front of it.
pub fn layer(inner: axum::Router, hook: Arc<TableHook>) -> axum::Router {
    inner.layer(axum::middleware::from_fn_with_state(
        hook,
        hook_table_commits,
    ))
}

/// The table request a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Create { ns: String },
    Register { ns: String },
    Update { ns: String, table: String },
    Transaction,
    Rename,
    Drop { ns: String, table: String },
    DropNamespace { ns: String },
}

/// `(prefix, target)` for a request the hook handles, else `None`.
fn classify(method: &Method, path: &str) -> Option<(String, Target)> {
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let d = decode_segment;
    if method == Method::DELETE {
        return match segs.as_slice() {
            [prefix, "namespaces", ns, "tables", table] => Some((
                d(prefix)?,
                Target::Drop {
                    ns: d(ns)?,
                    table: d(table)?,
                },
            )),
            [prefix, "namespaces", ns] => Some((d(prefix)?, Target::DropNamespace { ns: d(ns)? })),
            _ => None,
        };
    }
    if method != Method::POST {
        return None;
    }
    match segs.as_slice() {
        [prefix, "namespaces", ns, "tables"] => Some((d(prefix)?, Target::Create { ns: d(ns)? })),
        [prefix, "namespaces", ns, "register"] => {
            Some((d(prefix)?, Target::Register { ns: d(ns)? }))
        }
        [prefix, "namespaces", ns, "tables", table] => Some((
            d(prefix)?,
            Target::Update {
                ns: d(ns)?,
                table: d(table)?,
            },
        )),
        [prefix, "transactions", "commit"] => Some((d(prefix)?, Target::Transaction)),
        [prefix, "tables", "rename"] => Some((d(prefix)?, Target::Rename)),
        _ => None,
    }
}

/// Does a table commit's `updates` change the schema?
fn changes_schema(updates: Option<&Value>) -> bool {
    updates
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|u| {
            matches!(
                u.get("action").and_then(Value::as_str),
                Some("add-schema" | "set-current-schema")
            )
        })
}

/// The `(namespace, table)` an Iceberg `TableIdentifier` names, the levels
/// joined with lakekeeper's `\u{1f}`.
fn identifier(v: &Value) -> Option<(String, String)> {
    let parts: Vec<&str> = v
        .get("namespace")?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    if parts.is_empty() {
        return None;
    }
    Some((parts.join("\u{1f}"), v.get("name")?.as_str()?.to_string()))
}

/// The tables a request's body says it changes the schema of, as
/// `(namespace, Some(table))`. Empty when it changes no schema, or when the
/// body is of a shape lakekeeper refuses anyway.
fn tables_of(target: &Target, body: &Value) -> Vec<(String, Option<String>)> {
    let name = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_string);
    match target {
        Target::Create { ns } | Target::Register { ns } => name(body)
            .map(|t| vec![(ns.clone(), Some(t))])
            .unwrap_or_default(),
        Target::Update { ns, table } => {
            if changes_schema(body.get("updates")) {
                vec![(ns.clone(), Some(table.clone()))]
            } else {
                Vec::new()
            }
        }
        Target::Transaction => body
            .get("table-changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| changes_schema(c.get("updates")))
            .filter_map(|c| identifier(c.get("identifier")?))
            .map(|(ns, t)| (ns, Some(t)))
            .collect(),
        Target::Rename => rename_of(body)
            .map(|(s, d)| vec![(s.0, Some(s.1)), (d.0, Some(d.1))])
            .unwrap_or_default(),
        Target::Drop { ns, table } => vec![(ns.clone(), Some(table.clone()))],
        Target::DropNamespace { ns } => vec![(ns.clone(), None)],
    }
}

/// A refusal before the commit. Every one is the Iceberg `409`.
struct Refusal(String);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let message = format!("egret refused the table change: {}", self.0);
        tracing::warn!(%message, "table hook refused a table commit");
        (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({
                "error": {"message": message, "type": "CommitFailedException", "code": 409}
            })),
        )
            .into_response()
    }
}

impl TableHook {
    /// The StarRocks profiles a change of `tables` in the warehouse behind
    /// `prefix` can change the compile of (module doc, step 1).
    async fn affected(
        &self,
        prefix: &str,
        tables: &[(String, Option<String>)],
    ) -> Result<Vec<i64>, String> {
        let mut conn = self.db.pool.acquire().await.map_err(|e| e.to_string())?;
        let Some((warehouse_id, _catalog)) = resolve(&mut conn, prefix)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(Vec::new());
        };
        egret_perm_compile::affected_starrocks_profiles_for_tables(&mut conn, warehouse_id, tables)
            .await
            .map_err(|e| e.to_string())
    }

    /// Recompile each profile in its own transaction (module doc, step 3). A
    /// failure is logged and recorded, never returned.
    async fn recompile(&self, profiles: &[i64]) {
        for &service_id in profiles {
            let result: Result<(), egret_perm::CompileError> = async {
                let db = |e: sqlx::Error| egret_perm::CompileError::Db(e.to_string());
                let mut tx = self.db.pool.begin().await.map_err(db)?;
                sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                    .bind(format!("{}ms", self.lock_wait.as_millis()))
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                egret_perm_compile::recompile_service_in_tx(&mut tx, service_id).await?;
                tx.commit().await.map_err(db)?;
                Ok(())
            }
            .await;
            if let Err(e) = result {
                tracing::warn!(
                    service_id,
                    error = %e,
                    "a table change committed, but recompiling the StarRocks profile failed; \
                     the boot pass retries it"
                );
                if let Err(w) = egret_perm_compile::compile_runs::record_compile_failure(
                    &self.db.pool,
                    service_id,
                    &e.to_string(),
                )
                .await
                {
                    tracing::warn!(service_id, error = %w,
                        "could not record a failed compile in perm.compile_runs");
                }
            }
        }
    }
}

/// The axum middleware (module doc). Every request it does not handle passes
/// through unchanged.
pub async fn hook_table_commits(
    State(hook): State<Arc<TableHook>>,
    req: Request,
    next: Next,
) -> Response {
    let Some((prefix, target)) = classify(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };
    let (req, tables) = if req.method() == Method::POST {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
            Ok(b) => b,
            Err(e) => {
                return Refusal(format!("the request body cannot be read: {e}")).into_response()
            }
        };
        // Not JSON: lakekeeper refuses it, and nothing changes.
        let tables = serde_json::from_slice::<Value>(&bytes)
            .map(|json| tables_of(&target, &json))
            .unwrap_or_default();
        (Request::from_parts(parts, Body::from(bytes)), tables)
    } else {
        let tables = tables_of(&target, &Value::Null);
        (req, tables)
    };
    // The Ranger resource has one schema level: a multi-level namespace is
    // never addressed by a compiled policy.
    let tables: Vec<(String, Option<String>)> = tables
        .into_iter()
        .filter(|(ns, _)| !is_multi_level(ns))
        .collect();
    if tables.is_empty() {
        return next.run(req).await;
    }
    let profiles = match hook.affected(&prefix, &tables).await {
        Ok(p) => p,
        Err(e) => {
            return Refusal(format!(
                "egret could not check which classifications the table carries ({e}); retry"
            ))
            .into_response()
        }
    };
    if profiles.is_empty() {
        return next.run(req).await;
    }
    // Uncancellable: a commit lakekeeper made is recompiled even when the
    // client has gone (as the view hold's after-commit steps).
    let work = async move {
        let response = next.run(req).await;
        if response.status().is_success() {
            hook.recompile(&profiles).await;
        }
        response
    };
    match tokio::spawn(work.in_current_span()).await {
        Ok(response) => response,
        Err(e) => Refusal(format!("the table hook's task failed: {e}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn post(path: &str) -> Option<(String, Target)> {
        classify(&Method::POST, path)
    }

    #[test]
    fn each_table_request_is_classified() {
        let p = "wh";
        assert_eq!(
            post("/wh/namespaces/sales/tables"),
            Some((p.into(), Target::Create { ns: "sales".into() }))
        );
        assert_eq!(
            post("/wh/namespaces/sales/register"),
            Some((p.into(), Target::Register { ns: "sales".into() }))
        );
        assert_eq!(
            post("/wh/namespaces/sales/tables/orders"),
            Some((
                p.into(),
                Target::Update {
                    ns: "sales".into(),
                    table: "orders".into()
                }
            ))
        );
        assert_eq!(
            post("/wh/transactions/commit"),
            Some((p.into(), Target::Transaction))
        );
        assert_eq!(post("/wh/tables/rename"), Some((p.into(), Target::Rename)));
        assert_eq!(
            classify(&Method::DELETE, "/wh/namespaces/sales/tables/orders"),
            Some((
                p.into(),
                Target::Drop {
                    ns: "sales".into(),
                    table: "orders".into()
                }
            ))
        );
        assert_eq!(
            classify(&Method::DELETE, "/wh/namespaces/sales"),
            Some((p.into(), Target::DropNamespace { ns: "sales".into() }))
        );
    }

    #[test]
    fn reads_and_view_requests_are_not_this_hooks() {
        assert_eq!(
            classify(&Method::GET, "/wh/namespaces/sales/tables/orders"),
            None
        );
        assert_eq!(classify(&Method::GET, "/wh/namespaces/sales/tables"), None);
        assert_eq!(post("/wh/namespaces/sales/views"), None);
        assert_eq!(post("/wh/namespaces/sales/views/v"), None);
        assert_eq!(post("/wh/views/rename"), None);
        assert_eq!(
            classify(&Method::DELETE, "/wh/namespaces/sales/views/v"),
            None
        );
        assert_eq!(post("/wh/namespaces/sales/tables/orders/metrics"), None);
    }

    #[test]
    fn an_update_counts_only_when_it_changes_the_schema() {
        let update = Target::Update {
            ns: "sales".into(),
            table: "orders".into(),
        };
        let append = json!({"updates": [
            {"action": "add-snapshot", "snapshot": {}},
            {"action": "set-snapshot-ref", "ref-name": "main"}
        ]});
        assert!(tables_of(&update, &append).is_empty());
        for action in ["add-schema", "set-current-schema"] {
            let body = json!({"updates": [{"action": action}]});
            assert_eq!(
                tables_of(&update, &body),
                vec![("sales".to_string(), Some("orders".to_string()))],
                "{action}"
            );
        }
    }

    #[test]
    fn a_multi_table_commit_names_only_the_tables_whose_schema_changes() {
        let body = json!({"table-changes": [
            {"identifier": {"namespace": ["sales"], "name": "orders"},
             "updates": [{"action": "add-schema"}]},
            {"identifier": {"namespace": ["sales"], "name": "events"},
             "updates": [{"action": "add-snapshot"}]},
        ]});
        assert_eq!(
            tables_of(&Target::Transaction, &body),
            vec![("sales".to_string(), Some("orders".to_string()))]
        );
    }

    #[test]
    fn a_rename_names_both_sides_and_a_namespace_drop_every_table_in_it() {
        let body = json!({
            "source": {"namespace": ["sales"], "name": "orders"},
            "destination": {"namespace": ["sales"], "name": "orders_v2"},
        });
        assert_eq!(
            tables_of(&Target::Rename, &body),
            vec![
                ("sales".to_string(), Some("orders".to_string())),
                ("sales".to_string(), Some("orders_v2".to_string()))
            ]
        );
        assert_eq!(
            tables_of(&Target::DropNamespace { ns: "sales".into() }, &Value::Null),
            vec![("sales".to_string(), None)]
        );
    }
}
