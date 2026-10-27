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
//! An undrop (the recovery page) reaches lakekeeper in process, not through this
//! layer; it goes through [`TableHook::undrop`] instead.
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
//! 2. A schema-GROWING request (a create, a rename, an update or multi-table
//!    commit that sets a new current schema) is dry-run first (C11, user
//!    decision U5): the real recompile of the affected profiles, in a
//!    transaction that is rolled back, with the columns the commit would
//!    leave overlaid on the column source
//!    ([`egret_perm_compile::table_columns::PENDING_COLUMNS_SETTING`]). If that
//!    compile is refused (a ceiling, an undecidable mask fold, a new tie), the
//!    request is refused with a 409 naming the reason, and nothing commits: a
//!    profile is never wedged by a schema change. A drop, which only shrinks,
//!    is not dry-run. A register's schema is in the metadata file it points
//!    to, which the hook does not read, so it is checked right AFTER its
//!    commit instead: its recompile runs in one transaction, and if that is
//!    refused the table is dropped again (files kept) and the request is
//!    answered 409 with the refusal.
//! 3. Forward to lakekeeper. A non-2xx answer is returned as it is.
//! 4. After a 2xx, recompile each affected profile in its own transaction,
//!    bounded by `lock_timeout`, before the response is returned. The
//!    Iceberg commit has happened and cannot be refused any more (C10): a
//!    failure is logged and recorded in `perm.compile_runs` (the Governance
//!    Overview shows it), the next recompile of the profile (or the next boot)
//!    retries it, and the 2xx passes. Until
//!    the recompile lands, the `*` deny refuses an uncleared direct read of a
//!    new column; only its view protection waits.
//!
//! # A classified rename moves its classifications (rename spec §5)
//!
//! A table rename whose source carries classification rows does not take the
//! path above. Its rows are moved to the new name BEFORE the commit, under a
//! hold modelled on the view hold ([`crate::view_hold`]):
//!
//! 1. a permit of the hook's own cap ([`DEFAULT_MAX_HOLDS`]), then
//!    `pg_try_advisory_lock` on both table names in the view hold's keyspace
//!    (busy refuses at once), then the per-profile locks over every profile
//!    bound to the warehouse;
//! 2. uncancellable from here: one transaction copies the rows to the new name
//!    (`ON CONFLICT DO NOTHING`, so it unions with rows already there), re-tags
//!    the views that read the new name, bumps the tag version and recompiles
//!    every bound profile with the source marked leaving in the column overlay
//!    (counted once against the ceilings), and refuses a pair it creates. A
//!    refusal is a 409 and nothing is renamed. This transaction is the
//!    pre-commit compile; no dry run runs;
//! 3. wait, within one deadline ([`crate::view_hold::default_hold_limit`]), for
//!    the Trino plugins bound to the warehouse to report the new tag version
//!    and the StarRocks plugins the new policy version. An engine type with
//!    nothing bound is not waited for; a bound one whose plugins stay silent
//!    refuses the move at the deadline and undoes the copy;
//! 4. forward. A non-2xx undoes exactly the rows the copy inserted; a 2xx
//!    deletes the source's rows, re-tags the views that read the old name, and
//!    recompiles with no overlay.
//!
//! A schema commit (an update or a multi-table commit) that renames a
//! classified column moves that column's rows the same way. A rename is a field
//! id present in both the stored current schema and the commit's new one under
//! another folded name; it is classified when the old name carries a column
//! row, or when the table carries a whole-table row and a StarRocks engine is
//! bound (StarRocks masks per column name). During the hold the table compiles
//! with its old and new column names, the old ones leaving. A field id that
//! vanished (a dropped column) keeps its rows dormant.
//!
//! A table drop keeps its rows (an undrop restores the name). A recursive
//! namespace drop (`?recursive=true`), which hard-deletes every tabular in it
//! and cannot be undropped, removes them: after its 2xx, one transaction
//! deletes every row under the namespace, the lineage of its views, and the
//! rows its tables gave views elsewhere, and records `tag:purge`. A plain
//! namespace drop only ever removes an empty namespace (lakekeeper refuses
//! one that holds any tabular, live or soft-deleted), so it changes nothing.

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

use egret_perm_compile::table_columns::{
    column_names, current_columns_via, current_fields_via, has_dropped_tabular_via,
    live_tabular_id_via, stored_schema_fields_via, table_states_via, top_level_columns,
    top_level_fields, PendingColumns, TableKey, TableState, PENDING_COLUMNS_SETTING,
};
use uuid::Uuid;

use crate::mount::CatalogCaller;
use crate::view_hold::{
    compile_is_lock_timeout, decode_segment, default_hold_limit, delete_inserted_rows, delete_rows,
    is_lock_timeout, is_multi_level, lock_key, rename_of, request_metadata, resolve, seconds,
    trino_tag_versions, uncancellable, Assignment, DEFAULT_MAX_HOLDS, MIN_LOCK_WAIT,
};
use egret_perm_compile::follow;
use egret_perm_compile::view_tags::{ViewReach, ViewReachError};
use egret_policy::{wait_for_versions, PolicyStore, WaitError};
use sqlx::Connection as _;
use tokio::sync::Semaphore;
use tokio::time::Instant;

/// Largest table request body the hook reads.
const MAX_BODY: usize = 16 << 20;

/// How long one profile's recompile may wait for a lock.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// The hook (module doc). Install it with
/// [`crate::mount::mount_catalog_engine`], or around a router with [`layer`].
#[derive(Clone)]
pub struct TableHook {
    db: Db,
    policy: PolicyStore,
    lock_wait: Duration,
    /// The deadline of one held move, from the request's arrival.
    limit: Duration,
    /// The cap on moves in flight, of the hook's own (rename spec C16).
    holds: Arc<Semaphore>,
}

impl TableHook {
    /// A hook whose recompiles wait at most [`DEFAULT_LOCK_WAIT`] for a lock,
    /// whose moves are held at most [`default_hold_limit`], at most
    /// [`DEFAULT_MAX_HOLDS`] at once.
    pub fn new(db: Db) -> Self {
        Self {
            policy: PolicyStore::new(db.clone()),
            db,
            lock_wait: DEFAULT_LOCK_WAIT,
            limit: default_hold_limit(),
            holds: Arc::new(Semaphore::new(DEFAULT_MAX_HOLDS)),
        }
    }

    /// The same hook with another move deadline (tests).
    #[must_use]
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    /// The same hook with another lock wait (tests).
    #[must_use]
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }
}

/// `inner` with the hook in front of it. The hook drops a refused register
/// through `inner` itself, with no hook in front of it.
pub fn layer(inner: axum::Router, hook: Arc<TableHook>) -> axum::Router {
    let loader = inner.clone();
    inner.layer(axum::middleware::from_fn_with_state(
        (hook, loader),
        hook_table_commits,
    ))
}

/// The table request a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Create {
        ns: String,
    },
    Register {
        ns: String,
    },
    TableCommit {
        ns: String,
        table: String,
    },
    Transaction,
    Rename,
    Drop {
        ns: String,
        table: String,
    },
    /// `recursive=true` hard-deletes every tabular in it (rename spec E13).
    DropNamespace {
        ns: String,
        recursive: bool,
    },
}

/// `(prefix, target)` for a request the hook handles, else `None`.
fn classify(method: &Method, path: &str, query: Option<&str>) -> Option<(String, Target)> {
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
            [prefix, "namespaces", ns] => Some((
                d(prefix)?,
                Target::DropNamespace {
                    ns: d(ns)?,
                    recursive: query
                        .into_iter()
                        .flat_map(|q| q.split('&'))
                        .any(|kv| kv.eq_ignore_ascii_case("recursive=true")),
                },
            )),
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
            Target::TableCommit {
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
        Target::TableCommit { ns, table } => {
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
        Target::DropNamespace { ns, .. } => vec![(ns.clone(), None)],
    }
}

/// The top-level fields `(id, name)` a table commit's `updates` leave
/// current, replayed as the table builder applies them: `set-current-schema
/// -1` is the schema added last before it, an explicit id an added schema of
/// that id or else a stored one. `None` when the commit leaves the current
/// schema alone, or names one that does not exist (lakekeeper refuses that).
async fn new_current_fields(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    ns: &str,
    table: &str,
    updates: Option<&Value>,
) -> Result<Option<Vec<(i64, String)>>, String> {
    let mut added: Vec<&Value> = Vec::new();
    let mut current: Option<i64> = None;
    let mut current_is_last_added: Option<usize> = None;
    for u in updates.and_then(Value::as_array).into_iter().flatten() {
        match u.get("action").and_then(Value::as_str) {
            Some("add-schema") => {
                if let Some(schema) = u.get("schema") {
                    added.push(schema);
                }
            }
            Some("set-current-schema") => match u.get("schema-id").and_then(Value::as_i64) {
                Some(-1) => {
                    current = None;
                    current_is_last_added = added.len().checked_sub(1);
                }
                Some(id) => {
                    current = Some(id);
                    current_is_last_added = None;
                }
                None => {}
            },
            _ => {}
        }
    }
    if let Some(i) = current_is_last_added {
        return Ok(Some(top_level_fields(added[i])));
    }
    let Some(id) = current else {
        return Ok(None);
    };
    if let Some(schema) = added
        .iter()
        .find(|s| s.get("schema-id").and_then(Value::as_i64) == Some(id))
    {
        return Ok(Some(top_level_fields(schema)));
    }
    stored_schema_fields_via(conn, warehouse_id, ns, table, id)
        .await
        .map_err(|e| e.to_string())
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
    ) -> Result<Option<(Uuid, Vec<i64>)>, String> {
        let mut conn = self.db.pool.acquire().await.map_err(|e| e.to_string())?;
        let Some((warehouse_id, _catalog)) = resolve(&mut conn, prefix)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let profiles = egret_perm_compile::affected_starrocks_profiles_for_tables(
            &mut conn,
            warehouse_id,
            tables,
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(Some((warehouse_id, profiles)))
    }

    /// The columns a schema-growing request would leave each table with
    /// (module doc, step 2); empty for every other request.
    async fn pending(
        &self,
        warehouse_id: Uuid,
        target: &Target,
        body: &Value,
    ) -> Result<Vec<PendingColumns>, String> {
        let mut conn = self.db.pool.acquire().await.map_err(|e| e.to_string())?;
        let entry = |ns: &str, table: &str, columns: Option<Vec<String>>| PendingColumns {
            warehouse_id,
            schema: ns.to_string(),
            table: table.to_string(),
            columns,
            leaving: Vec::new(),
            leaving_table: false,
        };
        let mut out = Vec::new();
        match target {
            Target::Create { ns } => {
                // A staged create is judged at its committing update.
                if body.get("stage-create").and_then(Value::as_bool) != Some(true) {
                    if let (Some(name), Some(schema)) =
                        (body.get("name").and_then(Value::as_str), body.get("schema"))
                    {
                        out.push(entry(ns, name, Some(top_level_columns(schema))));
                    }
                }
            }
            Target::TableCommit { ns, table } => {
                if let Some(fields) =
                    new_current_fields(&mut conn, warehouse_id, ns, table, body.get("updates"))
                        .await?
                {
                    out.push(entry(ns, table, Some(column_names(&fields))));
                }
            }
            Target::Transaction => {
                for change in body
                    .get("table-changes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some((ns, table)) = change.get("identifier").and_then(identifier) else {
                        continue;
                    };
                    if let Some(fields) = new_current_fields(
                        &mut conn,
                        warehouse_id,
                        &ns,
                        &table,
                        change.get("updates"),
                    )
                    .await?
                    {
                        out.push(entry(&ns, &table, Some(column_names(&fields))));
                    }
                }
            }
            Target::Rename => {
                if let Some(((src_ns, src), (dst_ns, dst))) = rename_of(body) {
                    let current = current_columns_via(&mut conn, warehouse_id, &src_ns, &src)
                        .await
                        .map_err(|e| e.to_string())?;
                    if let Some(c) = current {
                        out.push(entry(&dst_ns, &dst, Some(c.columns)));
                        out.push(entry(&src_ns, &src, None));
                    }
                }
            }
            Target::Register { .. } | Target::Drop { .. } | Target::DropNamespace { .. } => {}
        }
        out.retain(|p| !is_multi_level(&p.schema));
        Ok(out)
    }

    /// Recompile `profiles` in one transaction with `pending` overlaid, and
    /// roll it back (module doc, step 2).
    async fn dry_run(&self, profiles: &[i64], pending: &[PendingColumns]) -> Result<(), Refusal> {
        let internal = |e: &dyn std::fmt::Display| {
            Refusal(format!(
                "egret could not dry-run the recompile the change causes ({e}); retry"
            ))
        };
        let overlay = serde_json::to_string(pending).map_err(|e| internal(&e))?;
        let mut tx = self.db.pool.begin().await.map_err(|e| internal(&e))?;
        sqlx::query("SELECT set_config($1, $2, true), set_config('lock_timeout', $3, true)")
            .bind(PENDING_COLUMNS_SETTING)
            .bind(overlay)
            .bind(format!("{}ms", self.lock_wait.as_millis()))
            .execute(&mut *tx)
            .await
            .map_err(|e| internal(&e))?;
        // Every profile locked first, in ascending order, then one pre-mint:
        // the compiler's lock order (profiles, then `users` rows). A loop of
        // `recompile_service_in_tx` would interleave them (review finding 4).
        if let Err(e) = egret_perm_compile::recompile_services_in_tx(&mut tx, profiles).await {
            return Err(if compile_is_lock_timeout(&e) {
                Refusal(
                    "egret could not dry-run the recompile in time, because other permission \
                     changes were being written; retry"
                        .to_string(),
                )
            } else {
                Refusal(e.to_string())
            });
        }
        tx.rollback().await.map_err(|e| internal(&e))?;
        Ok(())
    }

    /// Recompile each profile in its own transaction (module doc, step 4). A
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
                     the next recompile of the profile, or the next boot, retries it"
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

impl TableHook {
    /// Undrop a soft-deleted table or view (the recovery page), which reaches
    /// lakekeeper in process, past the Iceberg mount and so past this layer
    /// (review finding 1). A table that comes back carrying a whole-table
    /// classification grows the profile by its columns, so it is treated like
    /// a schema-growing commit: dry-run first (a refusal is a 409 and nothing
    /// is undropped, U5), then undropped, then recompiled at once so its NULL
    /// masks are back before this returns.
    pub async fn undrop(
        &self,
        engine_state: crate::ApiContext<crate::EngineState>,
        system_user: &str,
        prefix: &str,
        kind: crate::recovery::TabularKindName,
        id: Uuid,
    ) -> Result<(), crate::CatalogEngineError> {
        let (profiles, pending) = match kind {
            crate::recovery::TabularKindName::Table => self
                .undrop_dry_run_inputs(prefix, id)
                .await
                .map_err(crate::CatalogEngineError::Internal)?,
            crate::recovery::TabularKindName::View => (Vec::new(), Vec::new()),
        };
        if profiles.is_empty() {
            return crate::recovery::undrop_tabular(engine_state, system_user, prefix, kind, id)
                .await;
        }
        let refused = |r: Refusal| {
            crate::CatalogEngineError::Conflict(format!("egret refused the undrop: {}", r.0))
        };
        let locks = self.lock_profiles(&profiles).await.map_err(refused)?;
        if let Err(r) = self.dry_run(&profiles, &pending).await {
            locks.release().await;
            return Err(refused(r));
        }
        let undropped =
            crate::recovery::undrop_tabular(engine_state, system_user, prefix, kind, id).await;
        if undropped.is_ok() {
            self.recompile(&profiles).await;
        }
        locks.release().await;
        undropped
    }

    /// The affected profiles and the overlay of undropping table `id`.
    async fn undrop_dry_run_inputs(
        &self,
        prefix: &str,
        id: Uuid,
    ) -> Result<(Vec<i64>, Vec<PendingColumns>), String> {
        let mut conn = self.db.pool.acquire().await.map_err(|e| e.to_string())?;
        let Some((warehouse_id, _)) = resolve(&mut conn, prefix)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        let Some(dropped) = egret_perm_compile::table_columns::dropped_table_columns_via(
            &mut conn,
            warehouse_id,
            id,
        )
        .await
        .map_err(|e| e.to_string())?
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        let profiles = egret_perm_compile::affected_starrocks_profiles_for_tables(
            &mut conn,
            warehouse_id,
            &[(dropped.schema.clone(), Some(dropped.table.clone()))],
        )
        .await
        .map_err(|e| e.to_string())?;
        let pending = vec![PendingColumns {
            warehouse_id,
            schema: dropped.schema,
            table: dropped.table,
            columns: Some(dropped.columns),
            leaving: Vec::new(),
            leaving_table: false,
        }];
        Ok((profiles, pending))
    }
}

impl TableHook {
    /// Recompile `profiles` in ONE transaction (the profiles locked in
    /// ascending order first), committing only if every one compiles.
    async fn recompile_or_refuse(&self, profiles: &[i64]) -> Result<(), egret_perm::CompileError> {
        let db = |e: sqlx::Error| egret_perm::CompileError::Db(e.to_string());
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(format!("{}ms", self.lock_wait.as_millis()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        egret_perm_compile::recompile_services_in_tx(&mut tx, profiles).await?;
        tx.commit().await.map_err(db)
    }

    /// Record `error` as each profile's failed compile (`perm.compile_runs`).
    async fn record_failure(&self, profiles: &[i64], error: &egret_perm::CompileError) {
        for &service_id in profiles {
            if let Err(w) = egret_perm_compile::compile_runs::record_compile_failure(
                &self.db.pool,
                service_id,
                &error.to_string(),
            )
            .await
            {
                tracing::warn!(service_id, error = %w,
                    "could not record a failed compile in perm.compile_runs");
            }
        }
    }
}

/// How long a table change waits for another schema change of the same
/// profiles (its dry run, its commit and its recompile) to finish.
pub const COMMIT_LOCK_WAIT: Duration = Duration::from_secs(30);

/// Session-level advisory locks, one per affected profile, on a connection
/// detached from the pool: closing it releases them whatever happens.
pub(crate) struct ProfileLocks(sqlx::PgConnection);

impl ProfileLocks {
    async fn release(self) {
        use sqlx::Connection as _;
        let mut conn = self.0;
        let _ = sqlx::query("SELECT pg_advisory_unlock_all()")
            .execute(&mut conn)
            .await;
        let _ = conn.close().await;
    }
}

impl TableHook {
    /// Take the per-profile schema-change locks, ascending (one order, so two
    /// changes over overlapping profiles cannot deadlock), waiting at most
    /// [`COMMIT_LOCK_WAIT`].
    async fn lock_profiles(&self, profiles: &[i64]) -> Result<ProfileLocks, Refusal> {
        let busy = || {
            Refusal(
                "another change to a table these classifications cover is being committed; retry"
                    .to_string(),
            )
        };
        let internal =
            |e: sqlx::Error| Refusal(format!("egret could not lock the change ({e}); retry"));
        let mut conn = self.db.pool.acquire().await.map_err(internal)?.detach();
        sqlx::query("SELECT set_config('lock_timeout', $1, false)")
            .bind(format!("{}ms", COMMIT_LOCK_WAIT.as_millis()))
            .execute(&mut conn)
            .await
            .map_err(internal)?;
        let mut ids = profiles.to_vec();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if let Err(e) = sqlx::query(
                "SELECT pg_advisory_lock(hashtextextended('egret-table-hook\u{1e}' || $1, 0))",
            )
            .bind(id.to_string())
            .execute(&mut conn)
            .await
            {
                let refusal = if crate::view_hold::is_lock_timeout(&e) {
                    busy()
                } else {
                    internal(e)
                };
                ProfileLocks(conn).release().await;
                return Err(refusal);
            }
        }
        Ok(ProfileLocks(conn))
    }
}

/// Drop a table just registered, through lakekeeper with no hook in front,
/// keeping its files (`purgeRequested=false`: they are the client's).
async fn drop_registered(
    loader: &axum::Router,
    path: String,
    metadata: lakekeeper::api::RequestMetadata,
) -> Result<(), String> {
    use tower::ServiceExt as _;
    let mut req = Request::builder()
        .method(Method::DELETE)
        .uri(path)
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    req.extensions_mut().insert(metadata);
    let response = loader
        .clone()
        .oneshot(req)
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("the catalog answered {}", response.status()))
    }
}

/// Percent-encode one path segment (everything but RFC 3986 unreserved).
fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The fold every engine and resolver matches names with
/// (`egret_ranger_eval::resource::fold_identifier`, SQL `lower()`).
fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// One coordinate a move re-points: a whole table, or one of its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Coordinate {
    schema: String,
    table: String,
    column: Option<String>,
}

impl Coordinate {
    /// Does `row` sit on this coordinate (rename spec C18: folded)? A table
    /// coordinate holds every row of the table, at either level.
    fn holds(&self, row: &Assignment) -> bool {
        fold(&row.1) == fold(&self.schema)
            && fold(&row.2) == fold(&self.table)
            && self.column.as_ref().is_none_or(|c| fold(&row.3) == fold(c))
    }

    fn label(&self) -> String {
        match &self.column {
            Some(c) => format!("{}.{}.{c}", self.schema, self.table),
            None => format!("{}.{}", self.schema, self.table),
        }
    }
}

/// The moves one request makes (rename spec §5.2), each `from → to`.
struct MovePlan {
    warehouse_id: Uuid,
    moves: Vec<(Coordinate, Coordinate)>,
    /// The column source the copy's compile sees: the destinations' columns,
    /// the sources marked leaving (C8).
    overlay: Vec<PendingColumns>,
    /// What the catalog holds once the change has committed: read when
    /// lakekeeper answers non-2xx, before the copy is undone (review
    /// finding 4).
    committed_if: Vec<Committed>,
    /// The sources carry classification rows (#757's part of the plan).
    classified: bool,
    /// What the moves carry of the permission model, per move (grants-follow
    /// spec §5.2): read at plan time, read again under the locks.
    perm: PermPart,
}

/// The permission-model part of a move plan (grants-follow spec §5.3).
#[derive(Default)]
struct PermPart {
    /// The registry catalog over the warehouse, `(id, name)`.
    catalog: Option<(Uuid, String)>,
    /// The profiles bound to the warehouse (D5 reads their hand-written
    /// policies).
    services: Vec<i64>,
    /// One entry per move.
    moves: Vec<PermMove>,
}

/// What one coordinate move carries.
struct PermMove {
    mv: follow::Move,
    follow: follow::PermFollow,
    handwritten: follow::HandwrittenCheck,
}

impl PermMove {
    /// Something to copy, move or report: the move needs the hold.
    fn has_work(&self) -> bool {
        !self.follow.copy_grants.is_empty()
            || !self.follow.copy_rules.is_empty()
            || !self.follow.move_grants.is_empty()
            || !self.follow.left.is_empty()
            || !self.handwritten.report.is_empty()
    }

    /// The 409's text when something refuses the move (D4, D5, D14).
    fn refusal(&self) -> Option<String> {
        follow::refusal_message(&self.follow.refused, &self.handwritten.refuse, &self.mv)
    }
}

impl PermPart {
    /// Read, on `conn`, what each of `moves` carries; `warehouse_id`'s
    /// registry catalog names the rows. No catalog: nothing (a warehouse no
    /// catalog is registered over has no grants).
    async fn read(
        conn: &mut sqlx::PgConnection,
        warehouse_id: Uuid,
        moves: &[(Coordinate, Coordinate)],
    ) -> Result<Self, String> {
        let catalog: Option<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM registry.catalogs WHERE warehouse_id = $1")
                .bind(warehouse_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| e.to_string())?;
        let Some((catalog_id, catalog_name)) = catalog.clone() else {
            return Ok(Self::default());
        };
        let services =
            egret_catalog_registry::services_bound_to_warehouse(&mut *conn, warehouse_id)
                .await
                .map_err(|e| e.to_string())?;
        let mut out = Self {
            catalog,
            services,
            moves: Vec::new(),
        };
        for (from, to) in moves {
            let mv = follow::Move {
                catalog_id,
                from: follow::Coord {
                    schema: from.schema.clone(),
                    table: from.table.clone(),
                    column: from.column.clone(),
                },
                to: follow::Coord {
                    schema: to.schema.clone(),
                    table: to.table.clone(),
                    column: to.column.clone(),
                },
            };
            out.moves
                .push(Self::read_move(conn, &out.services, &catalog_name, mv).await?);
        }
        Ok(out)
    }

    async fn read_move(
        conn: &mut sqlx::PgConnection,
        services: &[i64],
        catalog_name: &str,
        mv: follow::Move,
    ) -> Result<PermMove, String> {
        let follow = follow::plan_via(conn, &mv)
            .await
            .map_err(|e| e.to_string())?;
        let handwritten = follow::handwritten_via(conn, services, catalog_name, &mv)
            .await
            .map_err(|e| e.to_string())?;
        Ok(PermMove {
            mv,
            follow,
            handwritten,
        })
    }

    /// The same moves read again on `conn` (under the hold's locks).
    async fn reread(&self, conn: &mut sqlx::PgConnection) -> Result<Self, String> {
        let Some((_, catalog_name)) = &self.catalog else {
            return Ok(Self::default());
        };
        let mut moves = Vec::new();
        for m in &self.moves {
            moves.push(Self::read_move(conn, &self.services, catalog_name, m.mv.clone()).await?);
        }
        Ok(Self {
            catalog: self.catalog.clone(),
            services: self.services.clone(),
            moves,
        })
    }

    /// Every move's refusal, joined; `None` when nothing refuses.
    fn refusal(&self) -> Option<String> {
        let parts: Vec<String> = self.moves.iter().filter_map(PermMove::refusal).collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    fn has_work(&self) -> bool {
        self.moves.iter().any(PermMove::has_work)
    }
}

/// One sign that a held change committed.
enum Committed {
    /// The destination is an active table with the source's table id.
    Renamed {
        schema: String,
        table: String,
        tabular_id: Uuid,
    },
    /// The table's stored current schema is the commit's new one.
    Schema {
        schema: String,
        table: String,
        fields: Vec<(i64, String)>,
    },
}

impl MovePlan {
    /// The tables on either side of the moves, each once (folded).
    fn tables(&self) -> Vec<(String, String)> {
        let mut out = self.sources();
        for (schema, table) in self.destinations() {
            push_table(&mut out, schema, table);
        }
        out
    }

    fn sources(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (from, _) in &self.moves {
            push_table(&mut out, from.schema.clone(), from.table.clone());
        }
        out
    }

    fn destinations(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (_, to) in &self.moves {
            push_table(&mut out, to.schema.clone(), to.table.clone());
        }
        out
    }

    /// What the move carries, as its refusals name it.
    fn what(&self) -> &'static str {
        if self.classified {
            "classifications"
        } else {
            "permissions"
        }
    }

    fn destination_labels(&self) -> String {
        let labels: Vec<String> = self.moves.iter().map(|(_, to)| to.label()).collect();
        labels.join(", ")
    }
}

fn push_table(out: &mut Vec<(String, String)>, schema: String, table: String) {
    if !out
        .iter()
        .any(|(s, t)| fold(s) == fold(&schema) && fold(t) == fold(&table))
    {
        out.push((schema, table));
    }
}

/// The target state (rename spec §5.2): every row on a move's source
/// re-pointed to its destination, each copy keeping its column spelling (for a
/// table move) and its `from_lineage`; every other row as it is. One row per
/// exact key; a moved row that lands on a row already there is that row (C1).
fn target_of(before: &[Assignment], moves: &[(Coordinate, Coordinate)]) -> Vec<Assignment> {
    let mut out: Vec<Assignment> = before
        .iter()
        .filter(|row| !moves.iter().any(|(from, _)| from.holds(row)))
        .cloned()
        .collect();
    for row in before {
        let Some((_, to)) = moves.iter().find(|(from, _)| from.holds(row)) else {
            continue;
        };
        let moved = (
            row.0,
            to.schema.clone(),
            to.table.clone(),
            to.column.clone().unwrap_or_else(|| row.3.clone()),
            row.4,
        );
        if !out.iter().any(|r| same_key(r, &moved)) {
            out.push(moved);
        }
    }
    out
}

fn same_key(a: &Assignment, b: &Assignment) -> bool {
    a.0 == b.0 && a.1 == b.1 && a.2 == b.2 && a.3 == b.3
}

/// `rows` minus every row whose key is in `minus`.
fn without(rows: &[Assignment], minus: &[Assignment]) -> Vec<Assignment> {
    rows.iter()
        .filter(|r| !minus.iter().any(|m| same_key(r, m)))
        .cloned()
        .collect()
}

/// The classification rows of `tables` in `warehouse_id`, matched folded.
async fn rows_of(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    tables: &[(String, String)],
) -> Result<Vec<Assignment>, sqlx::Error> {
    let (schemas, names): (Vec<String>, Vec<String>) = tables.iter().cloned().unzip();
    sqlx::query_as(
        "SELECT a.tag_id, a.schema_name, a.table_name, a.column_name, a.from_lineage \
           FROM catalog_tag_assignments a \
          WHERE a.warehouse_id = $1 \
            AND EXISTS (SELECT 1 FROM unnest($2::text[], $3::text[]) AS t(s, n) \
                         WHERE lower(t.s) = lower(a.schema_name) \
                           AND lower(t.n) = lower(a.table_name)) \
          ORDER BY a.tag_id, a.schema_name, a.table_name, a.column_name",
    )
    .bind(warehouse_id)
    .bind(&schemas)
    .bind(&names)
    .fetch_all(&mut *conn)
    .await
}

/// What the copy transaction wrote and must be waited for.
struct Copied {
    /// The rows on the plan's tables before the copy, read under its locks.
    before: Vec<Assignment>,
    /// The rows the copy inserted (`Target \ Before`, less what existed).
    inserted: Vec<Assignment>,
    /// `Target`, which the after-commit step keeps.
    target: Vec<Assignment>,
    /// `(Trino profile, its tag version)` to wait for.
    tags: Vec<(i64, i64)>,
    /// `(profile, its policy version)` to wait for: StarRocks (C5), and Trino
    /// when a grant or rule copy was inserted (grants-follow D6).
    policies: Vec<(i64, i64)>,
    /// The permission-model part as read under the locks, and the rows each
    /// move's copy inserted (grants-follow §5.3).
    perm: PermPart,
    copies: Vec<follow::Copies>,
}

/// `(profile id, policy version)` of every StarRocks engine bound to the
/// warehouse: the StarRocks plugin reports its `lastKnownVersion` against
/// the profile's `policy_version` on each policy poll (rename spec C5).
async fn starrocks_policy_versions(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT s.id, s.policy_version \
           FROM registry.catalogs c \
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
           JOIN policy.authz_services s ON s.id = e.authz_service_id \
          WHERE c.warehouse_id = $1 AND e.type = 'starrocks' \
          ORDER BY s.id",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await
}

/// `(profile id, policy version)` of every Trino engine bound to the
/// warehouse: Trino's plugin reports its `lastKnownVersion` on each policy
/// poll as well (grants-follow spec D6, E26).
async fn trino_policy_versions(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT s.id, s.policy_version \
           FROM registry.catalogs c \
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
           JOIN policy.authz_services s ON s.id = e.authz_service_id \
          WHERE c.warehouse_id = $1 AND e.type = 'trino' \
          ORDER BY s.id",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await
}

/// The name locks of one move, on a connection detached from the pool;
/// closing it releases them.
struct NameLocks(sqlx::PgConnection);

impl NameLocks {
    async fn release(self) {
        let _ = self.0.close().await;
    }
}

impl TableHook {
    /// The move a table rename makes (rename spec §5.2), or `None` when
    /// nothing moves: the source is multi-level (it carries nothing), the
    /// rename changes case only (C10), the source carries no row, or it is
    /// not a live table (C9). A classified source going into a multi-level
    /// namespace is refused (C2).
    async fn move_plan_for_rename(
        &self,
        prefix: &str,
        body: &Value,
    ) -> Result<Option<MovePlan>, Refusal> {
        let check = |e: &dyn std::fmt::Display| {
            Refusal(format!(
                "egret could not check which classifications the table carries ({e}); retry"
            ))
        };
        let Some(((src_ns, src), (dst_ns, dst))) = rename_of(body) else {
            return Ok(None);
        };
        if is_multi_level(&src_ns) {
            return Ok(None);
        }
        // Every engine and resolver folds case: a case-only rename changes no
        // name any of them matches, so it is not worth a hold.
        if fold(&src_ns) == fold(&dst_ns) && fold(&src) == fold(&dst) {
            return Ok(None);
        }
        let mut conn = self.db.pool.acquire().await.map_err(|e| check(&e))?;
        let Some((warehouse_id, _)) = resolve(&mut conn, prefix).await.map_err(|e| check(&e))?
        else {
            return Ok(None);
        };
        let source = [(src_ns.clone(), src.clone())];
        let rows = rows_of(&mut conn, warehouse_id, &source)
            .await
            .map_err(|e| check(&e))?;
        let Some(current) = current_columns_via(&mut conn, warehouse_id, &src_ns, &src)
            .await
            .map_err(|e| check(&e))?
        else {
            return Ok(None);
        };
        let moves = vec![(
            Coordinate {
                schema: src_ns.clone(),
                table: src.clone(),
                column: None,
            },
            Coordinate {
                schema: dst_ns.clone(),
                table: dst.clone(),
                column: None,
            },
        )];
        let perm = PermPart::read(&mut conn, warehouse_id, &moves)
            .await
            .map_err(|e| check(&e))?;
        if rows.is_empty() && !perm.has_work() && perm.refusal().is_none() {
            return Ok(None);
        }
        let tabular_id = live_tabular_id_via(&mut conn, warehouse_id, &src_ns, &src)
            .await
            .map_err(|e| check(&e))?;
        // Lakekeeper refuses a rename onto a name a soft-deleted tabular holds
        // (with a 404 "Error getting tabular"): say so at once, instead of
        // after the whole hold (live check F1).
        let dst_key = TableKey {
            warehouse_id,
            schema: dst_ns.clone(),
            table: dst.clone(),
        };
        if !is_multi_level(&dst_ns)
            && table_states_via(&mut conn, std::slice::from_ref(&dst_key))
                .await
                .map_err(|e| check(&e))?
                == [TableState::Dropped]
        {
            return Err(Refusal(format!(
                "the new name {}.{dst} belongs to a dropped table in the recycle bin, and the \
                 catalog does not rename onto it; undrop that table, purge it, or choose another \
                 name. Nothing was renamed.",
                dst_ns.replace('\u{1f}', ".")
            )));
        }
        if !rows.is_empty() && is_multi_level(&dst_ns) {
            return Err(Refusal(
                "the table carries classifications and its new namespace has more than one \
                 level, where egret cannot keep them; use a single-level schema"
                    .to_string(),
            ));
        }
        if let Some(why) = perm.refusal() {
            return Err(Refusal(why));
        }
        let pending = |schema: &str, table: &str, leaving_table: bool| PendingColumns {
            warehouse_id,
            schema: schema.to_string(),
            table: table.to_string(),
            columns: Some(current.columns.clone()),
            leaving: Vec::new(),
            leaving_table,
        };
        let mut overlay = vec![pending(&dst_ns, &dst, false), pending(&src_ns, &src, true)];
        overlay.retain(|p| !is_multi_level(&p.schema));
        Ok(Some(MovePlan {
            warehouse_id,
            overlay,
            committed_if: tabular_id
                .map(|tabular_id| Committed::Renamed {
                    schema: dst_ns.clone(),
                    table: dst.clone(),
                    tabular_id,
                })
                .into_iter()
                .collect(),
            moves,
            classified: !rows.is_empty(),
            perm,
        }))
    }

    /// The moves a schema commit makes (rename spec §5.2, column plan, C12):
    /// for each table change that sets a new current schema, the field ids
    /// present in both the stored current schema and the new one whose folded
    /// name changed, kept when the old name carries a column row, or when the
    /// table carries a whole-table row and a StarRocks engine is bound
    /// (StarRocks masks per column name). `None` when nothing moves; a field
    /// id that vanished keeps its rows dormant.
    async fn move_plan_for_columns(
        &self,
        prefix: &str,
        target: &Target,
        body: &Value,
    ) -> Result<Option<MovePlan>, Refusal> {
        let check = |e: &dyn std::fmt::Display| {
            Refusal(format!(
                "egret could not check which classifications the table carries ({e}); retry"
            ))
        };
        let changes: Vec<(String, String, Option<&Value>)> = match target {
            Target::TableCommit { ns, table } => {
                vec![(ns.clone(), table.clone(), body.get("updates"))]
            }
            Target::Transaction => body
                .get("table-changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let (ns, table) = identifier(c.get("identifier")?)?;
                    Some((ns, table, c.get("updates")))
                })
                .collect(),
            _ => Vec::new(),
        };
        let changes: Vec<_> = changes
            .into_iter()
            .filter(|(ns, _, updates)| changes_schema(*updates) && !is_multi_level(ns))
            .collect();
        if changes.is_empty() {
            return Ok(None);
        }
        let mut conn = self.db.pool.acquire().await.map_err(|e| check(&e))?;
        let Some((warehouse_id, _)) = resolve(&mut conn, prefix).await.map_err(|e| check(&e))?
        else {
            return Ok(None);
        };
        let mut starrocks_bound: Option<bool> = None;
        let mut classified_any = false;
        let mut perm = PermPart::default();
        let mut moves = Vec::new();
        let mut overlay = Vec::new();
        // The request's other schema changes: when the commit is held, the
        // copy's compile is their dry run too (review finding 3), so a
        // ceiling they cross is a 409 before the commit, as #755's dry run
        // would answer.
        let mut others = Vec::new();
        let mut committed_if = Vec::new();
        for (ns, table, updates) in changes {
            let Some(new) = new_current_fields(&mut conn, warehouse_id, &ns, &table, updates)
                .await
                .map_err(|e| check(&e))?
            else {
                continue;
            };
            let Some(old) = current_fields_via(&mut conn, warehouse_id, &ns, &table)
                .await
                .map_err(|e| check(&e))?
            else {
                continue;
            };
            let renames: Vec<(String, String)> = old
                .fields
                .iter()
                .filter_map(|(id, old_name)| {
                    let (_, new_name) = new.iter().find(|(i, _)| i == id)?;
                    (fold(old_name) != fold(new_name)).then(|| (old_name.clone(), new_name.clone()))
                })
                .collect();
            let unmoved = PendingColumns {
                warehouse_id,
                schema: ns.clone(),
                table: table.clone(),
                columns: Some(column_names(&new)),
                leaving: Vec::new(),
                leaving_table: false,
            };
            if renames.is_empty() {
                others.push(unmoved);
                continue;
            }
            let rows = rows_of(&mut conn, warehouse_id, &[(ns.clone(), table.clone())])
                .await
                .map_err(|e| check(&e))?;
            let whole_table = rows.iter().any(|r| r.3.is_empty());
            let masks_per_column = whole_table && {
                if starrocks_bound.is_none() {
                    starrocks_bound = Some(
                        !starrocks_policy_versions(&mut conn, warehouse_id)
                            .await
                            .map_err(|e| check(&e))?
                            .is_empty(),
                    );
                }
                starrocks_bound == Some(true)
            };
            let at = |column: &str| Coordinate {
                schema: ns.clone(),
                table: table.clone(),
                column: Some(column.to_string()),
            };
            // A renamed column moves when it is classified (#757 C12) or when a
            // grant or path rule names it (grants-follow §5.4).
            let pairs: Vec<(Coordinate, Coordinate)> =
                renames.iter().map(|(o, n)| (at(o), at(n))).collect();
            let carried = PermPart::read(&mut conn, warehouse_id, &pairs)
                .await
                .map_err(|e| check(&e))?;
            let mut classified: Vec<&(String, String)> = Vec::new();
            for (pair, rename) in carried.moves.into_iter().zip(&renames) {
                let is_classified =
                    masks_per_column || rows.iter().any(|r| fold(&r.3) == fold(&rename.0));
                let carries = pair.has_work() || pair.refusal().is_some();
                if is_classified {
                    classified_any = true;
                }
                if is_classified || carries {
                    classified.push(rename);
                    perm.moves.push(pair);
                }
            }
            if perm.catalog.is_none() {
                perm.catalog = carried.catalog;
                perm.services = carried.services;
            }
            if classified.is_empty() {
                others.push(unmoved);
                continue;
            }
            committed_if.push(Committed::Schema {
                schema: ns.clone(),
                table: table.clone(),
                fields: new.clone(),
            });
            for (old_name, new_name) in classified {
                moves.push((at(old_name), at(new_name)));
            }
            // During the hold the table still has its old columns: both
            // names compile, the old ones no longer in the schema leaving.
            let old_names = column_names(&old.fields);
            let new_names = column_names(&new);
            let mut columns = old_names.clone();
            for name in &new_names {
                if !columns.iter().any(|c| fold(c) == fold(name)) {
                    columns.push(name.clone());
                }
            }
            overlay.push(PendingColumns {
                warehouse_id,
                schema: ns.clone(),
                table: table.clone(),
                columns: Some(columns),
                leaving: old_names
                    .into_iter()
                    .filter(|o| !new_names.iter().any(|n| fold(n) == fold(o)))
                    .collect(),
                leaving_table: false,
            });
        }
        if moves.is_empty() {
            return Ok(None);
        }
        if let Some(why) = perm.refusal() {
            return Err(Refusal(why));
        }
        overlay.extend(others);
        Ok(Some(MovePlan {
            warehouse_id,
            moves,
            overlay,
            committed_if,
            classified: classified_any,
            perm,
        }))
    }

    /// The hold (module doc, "A classified rename"): cap, name locks, profile
    /// locks, then the uncancellable copy, wait, forward and finish.
    async fn hold_move(
        self: Arc<Self>,
        deadline: Instant,
        prefix: &str,
        plan: MovePlan,
        caller: Option<CatalogCaller>,
        req: Request,
        next: Next,
    ) -> Response {
        let Ok(permit) = self.holds.clone().try_acquire_owned() else {
            return Refusal(format!(
                "egret is holding {DEFAULT_MAX_HOLDS} classified table changes already, its \
                 maximum; retry in a minute"
            ))
            .into_response();
        };
        let names = match self.lock_names(prefix, &plan.tables()).await {
            Ok(n) => n,
            Err(refusal) => return refusal.into_response(),
        };
        let profiles = match egret_catalog_registry::services_bound_to_warehouse(
            &self.db.pool,
            plan.warehouse_id,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                names.release().await;
                return Refusal(format!("egret could not lock the change ({e}); retry"))
                    .into_response();
            }
        };
        let locks = match self.lock_profiles(&profiles).await {
            Ok(l) => l,
            Err(refusal) => {
                names.release().await;
                return refusal.into_response();
            }
        };
        let this = self.clone();
        uncancellable(async move {
            let response = this
                .run_move(deadline, &plan, &profiles, caller.as_ref(), req, next)
                .await;
            locks.release().await;
            names.release().await;
            drop(permit);
            response
        })
        .await
    }

    /// `pg_try_advisory_lock` on every table name of the move, in the view
    /// hold's keyspace (C17), sorted. Busy refuses at once.
    async fn lock_names(
        &self,
        prefix: &str,
        tables: &[(String, String)],
    ) -> Result<NameLocks, Refusal> {
        let internal =
            |e: sqlx::Error| Refusal(format!("egret could not lock the change ({e}); retry"));
        let mut keys: Vec<String> = tables
            .iter()
            .map(|(ns, name)| lock_key(prefix, ns, name))
            .collect();
        keys.sort();
        keys.dedup();
        let mut conn = self.db.pool.acquire().await.map_err(internal)?.detach();
        for key in &keys {
            let got: bool =
                match sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                    .bind(key)
                    .fetch_one(&mut conn)
                    .await
                {
                    Ok(got) => got,
                    Err(e) => {
                        NameLocks(conn).release().await;
                        return Err(internal(e));
                    }
                };
            if !got {
                NameLocks(conn).release().await;
                return Err(Refusal(format!(
                    "another change to this table or its new name is being held; retry when it \
                     has finished (at most {} s)",
                    seconds(self.limit)
                )));
            }
        }
        Ok(NameLocks(conn))
    }

    /// Copy, wait, forward, then finish or compensate, under the locks.
    async fn run_move(
        &self,
        deadline: Instant,
        plan: &MovePlan,
        profiles: &[i64],
        caller: Option<&CatalogCaller>,
        req: Request,
        next: Next,
    ) -> Response {
        let copied = match self.copy(deadline, plan).await {
            Ok(c) => c,
            Err(refusal) => return refusal.into_response(),
        };
        if let Err(refusal) = self.wait(deadline, &copied).await {
            self.compensate(plan, &copied).await;
            return refusal.into_response();
        }
        let response = next.run(req).await;
        if !response.status().is_success() {
            // A non-2xx is not proof that nothing committed (review finding
            // 4): undo the copy only when the catalog shows the change absent.
            match self.committed(plan).await {
                Ok(false) => self.compensate(plan, &copied).await,
                Ok(true) => {
                    tracing::warn!(status = %response.status(),
                        destinations = %plan.destination_labels(),
                        "lakekeeper answered a held table change with an error, but the change \
                         committed: finishing the move");
                    self.finish(plan, &copied, profiles, caller).await;
                }
                Err(e) => tracing::error!(error = %e,
                    destinations = %plan.destination_labels(),
                    "lakekeeper answered a held table change with an error, and egret could not \
                     tell whether it committed: the classifications stay on both names"),
            }
            return response;
        }
        self.finish(plan, &copied, profiles, caller).await;
        response
    }

    /// Does the catalog show the held change committed (any of the plan's
    /// signs)? Read after lakekeeper answered non-2xx.
    async fn committed(&self, plan: &MovePlan) -> Result<bool, String> {
        let mut conn = self.db.pool.acquire().await.map_err(|e| e.to_string())?;
        for sign in &plan.committed_if {
            let seen = match sign {
                Committed::Renamed {
                    schema,
                    table,
                    tabular_id,
                } => {
                    live_tabular_id_via(&mut conn, plan.warehouse_id, schema, table)
                        .await
                        .map_err(|e| e.to_string())?
                        == Some(*tabular_id)
                }
                Committed::Schema {
                    schema,
                    table,
                    fields,
                } => current_fields_via(&mut conn, plan.warehouse_id, schema, table)
                    .await
                    .map_err(|e| e.to_string())?
                    .is_some_and(|current| {
                        current.fields.len() == fields.len()
                            && current
                                .fields
                                .iter()
                                .zip(fields)
                                .all(|((i, a), (j, b))| i == j && fold(a) == fold(b))
                    }),
            };
            if seen {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The copy transaction (rename spec §5.3 step 5): the pre-commit compile.
    async fn copy(&self, deadline: Instant, plan: &MovePlan) -> Result<Copied, Refusal> {
        let what = plan.what();
        let busy = || {
            Refusal(format!(
                "egret could not move the table's {what} in time, because other permission \
                 changes were being written; retry"
            ))
        };
        let db = |e: sqlx::Error| {
            if is_lock_timeout(&e) {
                busy()
            } else {
                Refusal(format!(
                    "egret could not move the table's {what} ({e}); retry"
                ))
            }
        };
        let refused = |e: &dyn std::fmt::Display| {
            Refusal(format!(
                "the table's {what} cannot move to {}: {e}. Nothing was renamed.",
                plan.destination_labels()
            ))
        };
        let reach_refusal = |e: ViewReachError| match e {
            ViewReachError::Db(e) => db(e),
            ViewReachError::Compile(e) if compile_is_lock_timeout(&e) => busy(),
            other => refused(&other),
        };
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(MIN_LOCK_WAIT);
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(format!("{}ms", wait.as_millis()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let reach = ViewReach::of_tables(&mut tx, plan.warehouse_id, &plan.destinations())
            .await
            .map_err(reach_refusal)?;
        let before = rows_of(&mut tx, plan.warehouse_id, &plan.tables())
            .await
            .map_err(db)?;
        let target = target_of(&before, &plan.moves);
        let overlaps_before =
            egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &reach.services)
                .await
                .map_err(|e| refused(&e))?;
        let mut inserted = Vec::new();
        for row in without(&target, &before) {
            let (tag, schema, table, column, from_lineage) = &row;
            let new: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO catalog_tag_assignments \
                     (tag_id, warehouse_id, schema_name, table_name, column_name, from_lineage) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 DO NOTHING RETURNING tag_id",
            )
            .bind(tag)
            .bind(plan.warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .bind(from_lineage)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| match e.as_database_error().and_then(|d| d.code()) {
                // 0177's CHECK: a destination name holding `*` or `?`.
                Some(c) if c == "23514" => refused(&e),
                _ => db(e),
            })?;
            if new.is_some() {
                inserted.push(row);
            }
        }
        // The permission-model part, read again under the locks: a pattern
        // or a hand-written policy written since the plan refuses here, and
        // nothing is renamed (grants-follow §5.3 step 5, D4).
        let perm = plan.perm.reread(&mut tx).await.map_err(|e| {
            Refusal(format!(
                "egret could not move the table's {what} ({e}); retry"
            ))
        })?;
        if let Some(why) = perm.refusal() {
            return Err(Refusal(why));
        }
        let mut copies = Vec::with_capacity(perm.moves.len());
        for m in &perm.moves {
            copies.push(
                follow::copy_in_tx(&mut tx, &m.follow, &m.mv)
                    .await
                    .map_err(|e| match e {
                        egret_perm::PermError::Db(e) => {
                            match e.as_database_error().and_then(|d| d.code()) {
                                Some(c) if c == "23514" => refused(&e),
                                _ => db(e),
                            }
                        }
                        other => refused(&other),
                    })?,
            );
        }
        let copied_restrictions = copies.iter().any(|c| !c.is_empty());
        let overlay = serde_json::to_string(&plan.overlay).map_err(|e| refused(&e))?;
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(PENDING_COLUMNS_SETTING)
            .bind(overlay)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        reach
            .retag_bump_and_recompile(&mut tx)
            .await
            .map_err(reach_refusal)?;
        egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &overlaps_before)
            .await
            .map_err(|e| refused(&e))?;
        // Read after the bump, in the same transaction: the versions the
        // plugins must report are the ones the copy is served under. Trino's
        // tags change only when rows were inserted; StarRocks' policies may
        // change through the overlay alone. A list with no engine bound is
        // empty, and is skipped by the wait (C4).
        //
        // A plan that copied nothing restrictive and moves no classification
        // (allows only, grants-follow D11) waits for nothing: the allows move
        // after the commit. A restriction copied changes the Trino profile's
        // policy version too (D6).
        let tags = if inserted.is_empty() {
            Vec::new()
        } else {
            trino_tag_versions(&mut tx, plan.warehouse_id)
                .await
                .map_err(db)?
        };
        let mut policies = if plan.classified || copied_restrictions {
            starrocks_policy_versions(&mut tx, plan.warehouse_id)
                .await
                .map_err(db)?
        } else {
            Vec::new()
        };
        if copied_restrictions {
            policies.extend(
                trino_policy_versions(&mut tx, plan.warehouse_id)
                    .await
                    .map_err(db)?,
            );
        }
        tx.commit().await.map_err(db)?;
        Ok(Copied {
            before,
            inserted,
            target,
            tags,
            policies,
            perm,
            copies,
        })
    }

    /// Wait until the plugins serve the copy (rename spec §5.3 step 6).
    async fn wait(&self, deadline: Instant, copied: &Copied) -> Result<(), Refusal> {
        let left = deadline.saturating_duration_since(Instant::now());
        match wait_for_versions(&self.policy, &copied.tags, &copied.policies, left).await {
            Ok(()) => Ok(()),
            Err(WaitError::Timeout { waiting_on }) => Err(Refusal(format!(
                "engines did not confirm the {} within {} s: {}; check that the engines are \
                 running and polling egret's Ranger endpoint, then retry. Nothing was renamed.",
                if copied.inserted.is_empty() && !copied.copies.iter().all(follow::Copies::is_empty)
                {
                    "copied restrictions"
                } else {
                    "moved classifications"
                },
                seconds(self.limit),
                waiting_on.join(", ")
            ))),
            Err(WaitError::Store(e)) => Err(Refusal(format!(
                "egret could not read the plugin download ledger ({e}); retry"
            ))),
        }
    }

    /// Undo the copy (rename spec §5.3, compensation): delete exactly the rows
    /// it inserted that still carry the provenance they were inserted with,
    /// and recompile with no overlay. A failure leaves them (over-tag) and is
    /// logged.
    async fn compensate(&self, plan: &MovePlan, copied: &Copied) {
        let result: Result<(), String> = async {
            let mut tx = self.db.pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                .bind(format!("{}ms", self.lock_wait.as_millis()))
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            let reach = ViewReach::of_tables(&mut tx, plan.warehouse_id, &plan.destinations())
                .await
                .map_err(|e| e.to_string())?;
            delete_inserted_rows(&mut tx, plan.warehouse_id, &copied.inserted)
                .await
                .map_err(|e| e.to_string())?;
            for copies in &copied.copies {
                follow::delete_copies_in_tx(&mut tx, copies)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            reach
                .retag_bump_and_recompile(&mut tx)
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        }
        .await;
        if let Err(e) = result {
            tracing::error!(error = %e, destinations = %plan.destination_labels(),
                "table hook could not undo a move; the copied classifications and restrictions \
                 remain on a name that did not get the table");
        }
    }

    /// After the 2xx (rename spec §5.3 step 8): delete `Before \ Target` (the
    /// sources' rows), re-tag the views that read the old names, recompile
    /// with no overlay, and record one `tag:move` event per moved coordinate
    /// by `caller` (C6, R7). A failure is logged and recorded as each
    /// profile's failed compile: the old rows stay, which over-tags and never
    /// under-tags.
    async fn finish(
        &self,
        plan: &MovePlan,
        copied: &Copied,
        profiles: &[i64],
        caller: Option<&CatalogCaller>,
    ) {
        let result: Result<(), String> = async {
            let mut tx = self.db.pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                .bind(format!("{}ms", self.lock_wait.as_millis()))
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            let reach = ViewReach::of_tables(&mut tx, plan.warehouse_id, &plan.sources())
                .await
                .map_err(|e| e.to_string())?;
            // A soft-deleted tabular under a source table's name still needs
            // its rows: an undrop brings it back under that name, classified
            // (review finding 1). Its source rows are copied, not moved.
            let sources = plan.sources();
            let keys: Vec<TableKey> = sources
                .iter()
                .map(|(schema, table)| TableKey {
                    warehouse_id: plan.warehouse_id,
                    schema: schema.clone(),
                    table: table.clone(),
                })
                .collect();
            let dropped = has_dropped_tabular_via(&mut tx, &keys)
                .await
                .map_err(|e| e.to_string())?;
            let kept: Vec<(String, String)> = sources
                .into_iter()
                .zip(dropped)
                .filter(|(_, d)| *d)
                .map(|(t, _)| t)
                .collect();
            let is_kept = |row: &Assignment| {
                kept.iter()
                    .any(|(s, t)| fold(s) == fold(&row.1) && fold(t) == fold(&row.2))
            };
            let leaving: Vec<Assignment> = without(&copied.before, &copied.target)
                .into_iter()
                .filter(|row| !is_kept(row))
                .collect();
            delete_rows(&mut tx, plan.warehouse_id, &leaving)
                .await
                .map_err(|e| e.to_string())?;
            // The allows follow the table now that it has its new name
            // (grants-follow G2, D2): in place, ids kept.
            let mut moved_allows = Vec::with_capacity(copied.perm.moves.len());
            for m in &copied.perm.moves {
                let ids: Vec<Uuid> = m.follow.move_grants.iter().map(|g| g.id).collect();
                moved_allows.push(
                    follow::move_allows_in_tx(&mut tx, &ids, &m.mv)
                        .await
                        .map_err(|e| e.to_string())?,
                );
            }
            reach
                .retag_bump_and_recompile(&mut tx)
                .await
                .map_err(|e| e.to_string())?;
            record_moves(&mut tx, plan, copied, &kept, caller)
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, destinations = %plan.destination_labels(),
                "a table change committed, but removing the classifications from the old name \
                 failed; they stay there until removed");
            self.record_failure(profiles, &egret_perm::CompileError::Db(e))
                .await;
        }
    }
}

impl TableHook {
    /// After a recursive namespace drop's 2xx (rename spec §5.6, C13), one
    /// transaction: the views in the namespace lose their lineage, every row
    /// whose folded schema is the namespace goes (any table, any column,
    /// manual or derived), the views elsewhere that read its tables are
    /// re-tagged, and the change is bumped, recompiled and recorded as
    /// `tag:purge` by `caller`. A failure is logged; the rows stay, and the
    /// stale-row report shows them.
    async fn purge_namespace(&self, prefix: &str, ns: &str, caller: Option<&CatalogCaller>) {
        let result: Result<(), String> = async {
            let err = |e: &dyn std::fmt::Display| e.to_string();
            let mut tx = self.db.pool.begin().await.map_err(|e| err(&e))?;
            sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                .bind(format!("{}ms", self.lock_wait.as_millis()))
                .execute(&mut *tx)
                .await
                .map_err(|e| err(&e))?;
            let Some((warehouse_id, _)) = resolve(&mut tx, prefix).await.map_err(|e| err(&e))?
            else {
                return Ok(());
            };
            // The namespace's tables any view may read: those with rows and
            // those a lineage records.
            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT table_name FROM catalog_tag_assignments \
                  WHERE warehouse_id = $1 AND lower(schema_name) = lower($2) \
                 UNION \
                 SELECT l.base_table FROM view_lineage l \
                   JOIN registry.catalogs c ON c.name = l.base_catalog \
                  WHERE c.warehouse_id = $1 AND lower(l.base_schema) = lower($2)",
            )
            .bind(warehouse_id)
            .bind(ns)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| err(&e))?;
            let tables: Vec<(String, String)> =
                tables.into_iter().map(|t| (ns.to_string(), t)).collect();
            let reach = ViewReach::of_tables(&mut tx, warehouse_id, &tables)
                .await
                .map_err(|e| err(&e))?;
            let views: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT s.view_catalog, s.view_schema, s.view_name FROM view_lineage_state s \
                   JOIN registry.catalogs c ON c.name = s.view_catalog \
                  WHERE c.warehouse_id = $1 AND lower(s.view_schema) = lower($2)",
            )
            .bind(warehouse_id)
            .bind(ns)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| err(&e))?;
            for (catalog, schema, name) in &views {
                let view = egret_perm_compile::lineage::ViewRef {
                    catalog: catalog.clone(),
                    schema: schema.clone(),
                    name: name.clone(),
                };
                egret_perm_compile::lineage::delete_lineage(&mut tx, &view)
                    .await
                    .map_err(|e| err(&e))?;
            }
            let rows = sqlx::query(
                "DELETE FROM catalog_tag_assignments \
                  WHERE warehouse_id = $1 AND lower(schema_name) = lower($2)",
            )
            .bind(warehouse_id)
            .bind(ns)
            .execute(&mut *tx)
            .await
            .map_err(|e| err(&e))?
            .rows_affected();
            reach
                .retag_bump_and_recompile(&mut tx)
                .await
                .map_err(|e| err(&e))?;
            if rows > 0 || !views.is_empty() {
                let caller = Caller::resolve(&mut tx, caller)
                    .await
                    .map_err(|e| err(&e))?;
                egret_audit::governance::record_change_by(
                    &mut tx,
                    caller.actor(),
                    egret_audit::governance::action::CATALOG_TAG_PURGE,
                    egret_audit::governance::resource::CATALOG,
                    format!("{warehouse_id}:{ns}"),
                    serde_json::json!({
                        "schema": ns, "rows": rows, "views": views.len(),
                        "trigger": "iceberg_drop_namespace",
                    }),
                )
                .await
                .map_err(|e| err(&e))?;
            }
            tx.commit().await.map_err(|e| err(&e))
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(namespace = %ns, error = %e,
                "a namespace was dropped, but removing its classifications failed; they stay \
                 until removed");
        }
    }
}

/// Who made a change, resolved from the Iceberg caller (rename spec §5.7, R7,
/// C6a): a catalog client by its `client_id` (one deleted since keeps it), a
/// user by id (no catalog token names one today), or egret when no guard ran.
enum Caller {
    User(egret_core::Principal),
    Client {
        id: Option<Uuid>,
        client_id: String,
        name: Option<String>,
        consumer_type: Option<String>,
    },
    Egret,
}

impl Caller {
    async fn resolve(
        conn: &mut sqlx::PgConnection,
        caller: Option<&CatalogCaller>,
    ) -> Result<Self, sqlx::Error> {
        let Some(c) = caller else {
            return Ok(Self::Egret);
        };
        if c.audience == "catalog" {
            let client: Option<(Uuid, String, String, String)> = sqlx::query_as(
                "SELECT id, client_id, name, consumer_type FROM catalog.catalog_clients \
                  WHERE client_id = $1",
            )
            .bind(&c.subject)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some((id, client_id, name, consumer_type)) = client {
                return Ok(Self::Client {
                    id: Some(id),
                    client_id,
                    name: Some(name),
                    consumer_type: Some(consumer_type),
                });
            }
        }
        if let Ok(user_id) = c.subject.parse::<Uuid>() {
            let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&mut *conn)
                .await?;
            if let Some(email) = email {
                return Ok(Self::User(egret_core::Principal {
                    user_id: egret_core::UserId(user_id),
                    email,
                    platform_roles: Vec::new(),
                    permissions: Vec::new(),
                    group_ids: Vec::new(),
                    impersonator: None,
                }));
            }
        }
        Ok(Self::Client {
            id: None,
            client_id: c.subject.clone(),
            name: None,
            consumer_type: None,
        })
    }

    fn actor(&self) -> egret_audit::governance::Actor<'_> {
        use egret_audit::governance::Actor;
        match self {
            Self::User(p) => Actor::User(p),
            Self::Client {
                id,
                client_id,
                name,
                consumer_type,
            } => Actor::CatalogClient {
                id: *id,
                client_id: client_id.clone(),
                name: name.clone(),
                consumer_type: consumer_type.clone(),
            },
            Self::Egret => Actor::Egret,
        }
    }
}

/// One `tag:move` event per coordinate whose rows moved (rename spec §5.7),
/// on the after-commit transaction: an event exists exactly when the move
/// completed.
async fn record_moves(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan: &MovePlan,
    copied: &Copied,
    kept: &[(String, String)],
    caller: Option<&CatalogCaller>,
) -> Result<(), sqlx::Error> {
    use egret_audit::governance::{action, record_change_by, resource};
    let caller = Caller::resolve(tx, caller).await?;
    let ids: Vec<Uuid> = copied.before.iter().map(|r| r.0).collect();
    let names: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM tags WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
    let tag = |id: Uuid| {
        let name = names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str());
        serde_json::json!({"id": id, "name": name})
    };
    let coordinate = |c: &Coordinate| serde_json::json!({"schema": c.schema, "table": c.table, "column": c.column});
    for (from, to) in &plan.moves {
        let moved: Vec<&Assignment> = copied.before.iter().filter(|r| from.holds(r)).collect();
        if moved.is_empty() {
            continue;
        }
        let mut tags: Vec<Uuid> = moved.iter().map(|r| r.0).collect();
        tags.sort();
        tags.dedup();
        let kept_rows: Vec<serde_json::Value> = copied
            .before
            .iter()
            .filter(|r| to.holds(r))
            .map(|r| {
                let column = (!r.3.is_empty()).then_some(r.3.as_str());
                serde_json::json!({"tag": tag(r.0), "column": column})
            })
            .collect();
        let source_kept = kept
            .iter()
            .any(|(s, t)| fold(s) == fold(&from.schema) && fold(t) == fold(&from.table));
        let trigger = if from.column.is_some() {
            "iceberg_column_rename"
        } else {
            "iceberg_rename"
        };
        record_change_by(
            tx,
            caller.actor(),
            action::CATALOG_TAG_MOVE,
            resource::CATALOG,
            format!("{}:{}.{}", plan.warehouse_id, from.schema, from.table),
            serde_json::json!({
                "before": coordinate(from),
                "after": coordinate(to),
                "tags": tags.into_iter().map(tag).collect::<Vec<_>>(),
                "rows": moved.len(),
                "kept": kept_rows,
                "source_kept": source_kept,
                "trigger": trigger,
            }),
        )
        .await?;
    }
    Ok(())
}

/// The axum middleware (module doc). Every request it does not handle passes
/// through unchanged.
pub async fn hook_table_commits(
    State((hook, loader)): State<(Arc<TableHook>, axum::Router)>,
    req: Request,
    next: Next,
) -> Response {
    let Some((prefix, target)) = classify(req.method(), req.uri().path(), req.uri().query()) else {
        return next.run(req).await;
    };
    let deadline = Instant::now() + hook.limit;
    // A recursive namespace drop cannot be undropped: after its 2xx the rows
    // under it go too (rename spec C13). A multi-level namespace carries none.
    if let Target::DropNamespace {
        ns,
        recursive: true,
    } = &target
    {
        if !is_multi_level(ns) {
            let ns = ns.clone();
            let caller = req.extensions().get::<CatalogCaller>().cloned();
            return uncancellable(async move {
                let response = handle_table_commit(
                    hook.clone(),
                    loader,
                    prefix.clone(),
                    target,
                    deadline,
                    req,
                    next,
                )
                .await;
                if response.status().is_success() {
                    hook.purge_namespace(&prefix, &ns, caller.as_ref()).await;
                }
                response
            })
            .await;
        }
    }
    handle_table_commit(hook, loader, prefix, target, deadline, req, next).await
}

/// The hook's work for one classified request (module doc).
async fn handle_table_commit(
    hook: Arc<TableHook>,
    loader: axum::Router,
    prefix: String,
    target: Target,
    deadline: Instant,
    req: Request,
    next: Next,
) -> Response {
    let (req, body) = if req.method() == Method::POST {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
            Ok(b) => b,
            Err(e) => {
                return Refusal(format!("the request body cannot be read: {e}")).into_response()
            }
        };
        // Not JSON: lakekeeper refuses it, and nothing changes.
        let json = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        (Request::from_parts(parts, Body::from(bytes)), json)
    } else {
        (req, Value::Null)
    };
    let refuse_check = |e: String| {
        Refusal(format!(
            "egret could not check which classifications the table carries ({e}); retry"
        ))
        .into_response()
    };
    let plan = match target {
        Target::Rename => hook.move_plan_for_rename(&prefix, &body).await,
        Target::TableCommit { .. } | Target::Transaction => {
            hook.move_plan_for_columns(&prefix, &target, &body).await
        }
        _ => Ok(None),
    };
    match plan {
        Ok(Some(plan)) => {
            let caller = req.extensions().get::<CatalogCaller>().cloned();
            return hook
                .clone()
                .hold_move(deadline, &prefix, plan, caller, req, next)
                .await;
        }
        Ok(None) => {}
        Err(refusal) => return refusal.into_response(),
    }
    let tables = tables_of(&target, &body);
    // The Ranger resource has one schema level: a multi-level namespace is
    // never addressed by a compiled policy.
    let tables: Vec<(String, Option<String>)> = tables
        .into_iter()
        .filter(|(ns, _)| !is_multi_level(ns))
        .collect();
    if tables.is_empty() {
        return next.run(req).await;
    }
    let (warehouse_id, profiles) = match hook.affected(&prefix, &tables).await {
        Ok(Some((w, p))) if !p.is_empty() => (w, p),
        Ok(_) => return next.run(req).await,
        Err(e) => return refuse_check(e),
    };
    // Held from the dry run to the recompile, so a concurrent schema change of
    // these profiles dry-runs against this one's columns (review finding 3).
    let locks = match hook.lock_profiles(&profiles).await {
        Ok(l) => l,
        Err(refusal) => return refusal.into_response(),
    };
    let pending = match hook.pending(warehouse_id, &target, &body).await {
        Ok(p) => p,
        Err(e) => {
            ProfileLocks::release(locks).await;
            return refuse_check(e);
        }
    };
    if !pending.is_empty() {
        if let Err(refusal) = hook.dry_run(&profiles, &pending).await {
            ProfileLocks::release(locks).await;
            return refusal.into_response();
        }
    }
    // A register's schema is in the metadata file it points to, which the
    // hook does not read: it is checked right after the commit instead, and a
    // refused one is dropped again (review finding 2, U5).
    let register_drop = match (&target, &tables[..]) {
        (Target::Register { .. }, [(_, Some(name))]) => Some((
            format!(
                "{}/tables/{}?purgeRequested=false",
                req.uri().path().trim_end_matches("/register"),
                encode_segment(name)
            ),
            request_metadata(&req),
        )),
        _ => None,
    };
    // Uncancellable: a commit lakekeeper made is recompiled even when the
    // client has gone (as the view hold's after-commit steps).
    let work = async move {
        let response = next.run(req).await;
        let response = if !response.status().is_success() {
            response
        } else if let Some((drop_path, metadata)) = register_drop {
            match hook.recompile_or_refuse(&profiles).await {
                Ok(()) => response,
                Err(e) => match drop_registered(&loader, drop_path, metadata).await {
                    Ok(()) => Refusal(format!("{e}; the table was not registered")).into_response(),
                    Err(why) => {
                        tracing::warn!(error = %why,
                            "a refused register could not be dropped again; the profile is \
                             wedged until the table or its classification is removed");
                        hook.record_failure(&profiles, &e).await;
                        response
                    }
                },
            }
        } else {
            hook.recompile(&profiles).await;
            response
        };
        ProfileLocks::release(locks).await;
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
        classify(&Method::POST, path, None)
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
                Target::TableCommit {
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
            classify(&Method::DELETE, "/wh/namespaces/sales/tables/orders", None),
            Some((
                p.into(),
                Target::Drop {
                    ns: "sales".into(),
                    table: "orders".into()
                }
            ))
        );
        assert_eq!(
            classify(&Method::DELETE, "/wh/namespaces/sales", None),
            Some((
                p.into(),
                Target::DropNamespace {
                    ns: "sales".into(),
                    recursive: false
                }
            ))
        );
        assert_eq!(
            classify(
                &Method::DELETE,
                "/wh/namespaces/sales",
                Some("force=true&recursive=true")
            ),
            Some((
                p.into(),
                Target::DropNamespace {
                    ns: "sales".into(),
                    recursive: true
                }
            ))
        );
    }

    #[test]
    fn reads_and_view_requests_are_not_this_hooks() {
        assert_eq!(
            classify(&Method::GET, "/wh/namespaces/sales/tables/orders", None),
            None
        );
        assert_eq!(
            classify(&Method::GET, "/wh/namespaces/sales/tables", None),
            None
        );
        assert_eq!(post("/wh/namespaces/sales/views"), None);
        assert_eq!(post("/wh/namespaces/sales/views/v"), None);
        assert_eq!(post("/wh/views/rename"), None);
        assert_eq!(
            classify(&Method::DELETE, "/wh/namespaces/sales/views/v", None),
            None
        );
        assert_eq!(post("/wh/namespaces/sales/tables/orders/metrics"), None);
    }

    #[test]
    fn an_update_counts_only_when_it_changes_the_schema() {
        let update = Target::TableCommit {
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
            tables_of(
                &Target::DropNamespace {
                    ns: "sales".into(),
                    recursive: false
                },
                &Value::Null
            ),
            vec![("sales".to_string(), None)]
        );
    }
}
