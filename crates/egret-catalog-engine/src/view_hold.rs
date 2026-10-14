//! View governance B2: hold a classified Trino view's Iceberg commit until
//! every Ranger plugin holds the view's classification tags.
//!
//! # Where it runs (live result §1 Q1)
//!
//! An axum layer in front of the mounted Iceberg router, inside egret's
//! [`crate::mount::CatalogGuard`]. lakekeeper's contract verification is not
//! the hook: it never runs on a view create, and on a commit it runs inside an
//! open Postgres write transaction. The layer sees three requests:
//!
//! - create, `POST /{prefix}/namespaces/{ns}/views`;
//! - commit, `POST /{prefix}/namespaces/{ns}/views/{view}`. Only a commit that
//!   carries an `add-view-version` is held. `COMMENT ON VIEW` and a replace
//!   whose updates are empty pass through untouched;
//! - rename, `POST /{prefix}/views/rename`, which moves the view's lineage and
//!   tag assignments to the new name under the same hold.
//!
//! # Order of operations (spec B2)
//!
//! No DB transaction stays open across the EXPLAIN or the wait.
//!
//! 0. If the profile that governs the view's catalog has no classified column
//!    at all, the commit passes through with no EXPLAIN. The lineage is left
//!    missing, and the B3 sweep computes it later.
//! 1. Compute the lineage ([`lineage::compute_lineage`]). On failure, refuse.
//! 2. If no lineage table is classified, let the commit proceed, and record the
//!    lineage (`computed`) once lakekeeper has answered 2xx.
//! 3. Otherwise, in one transaction: write one tag assignment per view column
//!    and tag, record the lineage, bump the tag version (and recompile the
//!    bound profiles, as every catalog-tag write does). Commit.
//! 4. Wait ([`wait_for_tag_version`]) for every Trino profile bound to the
//!    view's catalog to report the new tag version.
//! 5. Let the commit proceed. On timeout, or when lakekeeper answers non-2xx,
//!    compensate in one transaction: delete exactly the assignment rows step 3
//!    inserted, restore the lineage recorded before step 3, and bump again.
//!
//! # Refusal
//!
//! A `409` Iceberg REST error, which Trino prints verbatim. Its message names
//! the reason: [`LINEAGE_REFUSAL`] or [`UNCONFIRMED_REFUSAL`].
//!
//! # Concurrency
//!
//! Every hold takes a session-level advisory lock per view name it touches, on
//! a connection of its own that holds no transaction. Without it, a create
//! that lakekeeper refuses because a concurrent create of the same name won
//! would compensate, deleting assignment rows the winner relied on. The rows
//! were inserted by the loser, so the winner's insert was a no-op.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use egret_metastore::Db;
use egret_perm_compile::lineage::{self, ExplainRunner, LineageError, RecordedLineage, ViewRef};
use egret_policy::{wait_for_tag_version, PolicyStore, WaitError};
use serde_json::Value;
use sqlx::Connection as _;
use uuid::Uuid;

/// The reason a view is refused when egret cannot judge what it reads.
pub const LINEAGE_REFUSAL: &str = "lineage could not be computed";
/// The reason a view is refused when a plugin did not confirm the tags.
pub const UNCONFIRMED_REFUSAL: &str = "engines did not confirm the classification tags within";

/// Largest view request body the hold reads.
const MAX_BODY: usize = 16 << 20;

/// The hold limit: 2 × the plugin poll interval (spec B2), 60 s today.
pub fn default_hold_limit() -> Duration {
    Duration::from_millis(
        2 * u64::from(egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS),
    )
}

/// The hold (module doc). Install it with
/// [`crate::mount::mount_catalog_engine`].
pub struct ViewHold {
    db: Db,
    policy: PolicyStore,
    runner: Arc<dyn ExplainRunner>,
    limit: Duration,
}

impl ViewHold {
    /// A hold that waits up to [`default_hold_limit`].
    pub fn new(db: Db, runner: Arc<dyn ExplainRunner>) -> Self {
        Self {
            policy: PolicyStore::new(db.clone()),
            db,
            runner,
            limit: default_hold_limit(),
        }
    }

    /// The same hold with another limit (tests).
    #[must_use]
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    pub fn limit(&self) -> Duration {
        self.limit
    }
}

/// One `catalog_tag_assignments` row: tag, schema, table, column.
type Assignment = (Uuid, String, String, String);

/// What a hold wrote in step 3, so that compensation removes exactly it.
struct Written {
    warehouse_id: Uuid,
    view: ViewRef,
    /// The rows this hold inserted. A row that already existed is not here.
    inserted: Vec<Assignment>,
    /// The view's lineage before this hold wrote its own.
    previous: RecordedLineage,
    /// `(Trino profile id, its tag version)` to wait for.
    required: Vec<(i64, i64)>,
}

/// Why a request is refused. Every refusal is the Iceberg `409`.
struct Refusal(String);

impl Refusal {
    fn lineage(detail: impl std::fmt::Display) -> Self {
        Self(format!("{LINEAGE_REFUSAL}: {detail}"))
    }
    fn internal(what: &str, e: impl std::fmt::Display) -> Self {
        Self(format!("egret could not hold the view ({what}): {e}"))
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let message = format!("egret refused the view: {}", self.0);
        tracing::warn!(%message, "view hold refused a view commit");
        (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({
                "error": {"message": message, "type": "CommitFailedException", "code": 409}
            })),
        )
            .into_response()
    }
}

/// The view request a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Create { ns: String },
    Commit { ns: String, view: String },
    Rename,
}

/// `(prefix, target)` for a `POST` the hold handles, else `None`. A
/// multi-level namespace is `None`: nothing is recorded under one (see
/// [`decode_segment`]).
fn classify(method: &Method, path: &str) -> Option<(String, Target)> {
    if method != Method::POST {
        return None;
    }
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    match segs.as_slice() {
        [prefix, "namespaces", ns, "views"] => Some((
            decode_segment(prefix)?,
            Target::Create {
                ns: decode_segment(ns)?,
            },
        )),
        [prefix, "namespaces", ns, "views", view] => Some((
            decode_segment(prefix)?,
            Target::Commit {
                ns: decode_segment(ns)?,
                view: decode_segment(view)?,
            },
        )),
        [prefix, "views", "rename"] => Some((decode_segment(prefix)?, Target::Rename)),
        _ => None,
    }
}

/// Percent-decode one path segment. `None` for a malformed escape, invalid
/// UTF-8, or a lakekeeper multi-level namespace (`\u{1f}`-separated): Trino
/// sends single-level namespaces, and the Ranger resource has one schema
/// level, so the hold neither records nor guesses one.
fn decode_segment(seg: &str) -> Option<String> {
    let bytes = seg.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    (!s.contains('\u{1f}')).then_some(s)
}

/// What the hold needs from a new view version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Definition {
    sql: String,
    default_catalog: Option<String>,
    default_schema: Option<String>,
    columns: Vec<String>,
}

/// The Trino SQL, defaults and columns of `version`, with `schema` the
/// Iceberg schema that version uses.
fn definition(version: &Value, schema: Option<&Value>) -> Result<Definition, String> {
    let sql = version
        .get("representations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|r| {
            r.get("type").and_then(Value::as_str) == Some("sql")
                && r.get("dialect")
                    .and_then(Value::as_str)
                    .is_some_and(|d| d.eq_ignore_ascii_case("trino"))
        })
        .and_then(|r| r.get("sql"))
        .and_then(Value::as_str)
        .ok_or("the view version carries no Trino SQL representation")?
        .to_string();
    let default_catalog = version
        .get("default-catalog")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let default_schema = match version.get("default-namespace") {
        None | Some(Value::Null) => None,
        Some(Value::Array(parts)) => match parts.as_slice() {
            [] => None,
            [Value::String(s)] if !s.is_empty() => Some(s.clone()),
            _ => return Err("the view's default namespace is not one level".to_string()),
        },
        Some(_) => return Err("the view's default namespace is not an array".to_string()),
    };
    let columns: Vec<String> = schema
        .and_then(|s| s.get("fields"))
        .and_then(Value::as_array)
        .ok_or("the request carries no schema for the new view version")?
        .iter()
        .map(|f| f.get("name").and_then(Value::as_str).map(str::to_string))
        .collect::<Option<_>>()
        .ok_or("a field of the view's schema has no name")?;
    if columns.is_empty() {
        return Err("the view's schema has no column".to_string());
    }
    Ok(Definition {
        sql,
        default_catalog,
        default_schema,
        columns,
    })
}

/// A create's view name and definition. The name is read first: without it
/// nothing can be held or recorded.
fn create_definition(body: &Value) -> Result<(String, Result<Definition, String>), String> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the create request names no view")?
        .to_string();
    let def = match body.get("view-version") {
        Some(v) => definition(v, body.get("schema")),
        None => Err("the create request carries no view version".to_string()),
    };
    Ok((name, def))
}

/// A commit's new view version, or `None` when it carries none
/// (`COMMENT ON VIEW`, an empty replace): such a commit passes untouched.
fn commit_definition(body: &Value) -> Option<Result<Definition, String>> {
    let updates = body.get("updates").and_then(Value::as_array)?;
    let action = |u: &Value, a: &str| u.get("action").and_then(Value::as_str) == Some(a);
    let version = updates
        .iter()
        .rev()
        .find(|u| action(u, "add-view-version"))?
        .get("view-version");
    let Some(version) = version else {
        return Some(Err(
            "an add-view-version update carries no view version".into()
        ));
    };
    // `-1` is "the schema this commit last added"; any other id must be added
    // by this commit too, or the columns are not in the request.
    let schemas: Vec<&Value> = updates
        .iter()
        .filter(|u| action(u, "add-schema"))
        .filter_map(|u| u.get("schema"))
        .collect();
    let schema = match version.get("schema-id").and_then(Value::as_i64) {
        Some(-1) => schemas.last().copied(),
        Some(id) => schemas
            .iter()
            .rev()
            .find(|s| s.get("schema-id").and_then(Value::as_i64) == Some(id))
            .copied(),
        None => None,
    };
    Some(definition(version, schema))
}

/// `{source, destination}` of a rename, each `(namespace, name)`; `None` for
/// a multi-level namespace or a body of another shape.
fn rename_of(body: &Value) -> Option<((String, String), (String, String))> {
    let side = |k: &str| -> Option<(String, String)> {
        let v = body.get(k)?;
        let ns = match v.get("namespace")?.as_array()?.as_slice() {
            [Value::String(s)] => s.clone(),
            _ => return None,
        };
        Some((ns, v.get("name")?.as_str()?.to_string()))
    };
    Some((side("source")?, side("destination")?))
}

/// The egret warehouse a lakekeeper prefix belongs to, and its registry
/// catalog name (the name the engines and the lineage use).
async fn resolve(
    conn: &mut sqlx::PgConnection,
    prefix: &str,
) -> Result<Option<(Uuid, Option<String>)>, sqlx::Error> {
    let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT w.id, \
                (SELECT c.name FROM registry.catalogs c WHERE c.warehouse_id = w.id \
                  ORDER BY c.name LIMIT 1) \
           FROM catalog.warehouses w WHERE w.lakekeeper_warehouse_id = $1",
    )
    .bind(prefix)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row)
}

/// Does any warehouse reachable from `warehouse_id`'s profile hold a
/// classified column? Reachable: the warehouse itself, and every catalog
/// attached to an engine that attaches it or shares a profile with one that
/// does. "Classified" is [`lineage::view_tag_set`]'s rule: a tag with a
/// clearance-bearing `classification_policies` row.
async fn profile_has_classified_column(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "WITH engines AS ( \
             SELECT e2.id \
               FROM registry.catalogs c \
               JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
               JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
               JOIN engine_mgmt.engines e2 \
                 ON e2.id = e.id OR e2.authz_service_id = e.authz_service_id \
              WHERE c.warehouse_id = $1), \
         warehouses AS ( \
             SELECT $1::uuid AS id \
             UNION \
             SELECT c.warehouse_id FROM registry.engine_catalog_attachments ea \
               JOIN registry.catalogs c ON c.id = ea.catalog_id \
              WHERE ea.engine_id IN (SELECT id FROM engines) AND c.warehouse_id IS NOT NULL) \
         SELECT EXISTS ( \
             SELECT 1 FROM catalog_tag_assignments a \
               JOIN classification_policies cp ON cp.tag_id = a.tag_id \
              WHERE a.warehouse_id IN (SELECT id FROM warehouses) \
                AND (cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0))",
    )
    .bind(warehouse_id)
    .fetch_one(&mut *conn)
    .await
}

/// `(profile id, tag version)` of every Trino engine bound to the view's
/// warehouse. Trino polls tags under its RESOURCE service, and
/// `download_tags` serves that service's own counter (`plugin_downloads`).
async fn trino_tag_versions(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT s.id, s.tag_version \
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

/// Record `lineage` as `view`'s, through the lineage API only: computed,
/// failed, or (for `Missing`) nothing at all.
async fn record(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    lineage: &RecordedLineage,
) -> Result<(), LineageError> {
    match lineage {
        RecordedLineage::Missing => Ok(lineage::delete_lineage(conn, view).await?),
        RecordedLineage::Failed { detail } => {
            Ok(lineage::record_lineage_failure(conn, view, detail).await?)
        }
        RecordedLineage::Computed(tables) => lineage::write_lineage(conn, view, tables).await,
    }
}

/// The assignment rows `view` holds in `warehouse_id`.
async fn assignments_of(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    view: &ViewRef,
) -> Result<Vec<Assignment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT tag_id, schema_name, table_name, column_name FROM catalog_tag_assignments \
          WHERE warehouse_id = $1 AND schema_name = $2 AND table_name = $3 \
          ORDER BY tag_id, column_name",
    )
    .bind(warehouse_id)
    .bind(&view.schema)
    .bind(&view.name)
    .fetch_all(&mut *conn)
    .await
}

/// Bump the bound services' tag versions and recompile their profiles, as
/// every catalog-tag write does (`egret_bi::CatalogTagRepo`).
async fn bump_and_recompile(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    warehouse_id: Uuid,
    bound: &[i64],
) -> Result<(), Refusal> {
    egret_catalog_registry::bump_bound_tag_versions(tx, warehouse_id)
        .await
        .map_err(|e| Refusal::internal("tag version", e))?;
    egret_perm_compile::recompile_services_in_tx(tx, bound)
        .await
        .map_err(|e| Refusal(format!("the classification tags cannot be assigned: {e}")))?;
    Ok(())
}

impl ViewHold {
    /// Step 3: one transaction that writes `rows`, records `lineage` for
    /// `view`, and bumps the tag version when a row is new.
    async fn write(
        &self,
        warehouse_id: Uuid,
        view: &ViewRef,
        rows: &[Assignment],
        lineage: &RecordedLineage,
    ) -> Result<Written, Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
            .await
            .map_err(db)?;
        let overlaps_before = egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &bound)
            .await
            .map_err(|e| Refusal::internal("overlap snapshot", e))?;
        let previous = lineage::recorded_lineage(&mut tx, view).await.map_err(db)?;
        let mut inserted = Vec::new();
        for row in rows {
            let (tag, schema, table, column) = row;
            let new: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO catalog_tag_assignments \
                     (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 DO NOTHING RETURNING tag_id",
            )
            .bind(tag)
            .bind(warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if new.is_some() {
                inserted.push(row.clone());
            }
        }
        record(&mut tx, view, lineage)
            .await
            .map_err(|e| Refusal::internal("lineage", e))?;
        if !inserted.is_empty() {
            bump_and_recompile(&mut tx, warehouse_id, &bound).await?;
            egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &overlaps_before)
                .await
                .map_err(|e| Refusal(format!("the classification tags cannot be assigned: {e}")))?;
        }
        // Read after the bump, in the same transaction: the version the
        // plugins must report is the one these rows are served under.
        let required = trino_tag_versions(&mut tx, warehouse_id)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Written {
            warehouse_id,
            view: view.clone(),
            inserted,
            previous,
            required,
        })
    }

    /// Step 5's compensation: delete exactly the rows `w` inserted, restore
    /// the lineage recorded before it, and bump again. A failure is logged;
    /// what it leaves behind over-tags a view that was not committed.
    async fn compensate(&self, w: &Written) {
        if let Err(e) = self.try_compensate(w).await {
            tracing::error!(
                view = ?w.view,
                error = %e.0,
                "view hold could not compensate; the uncommitted view's tags and lineage remain"
            );
        }
    }

    async fn try_compensate(&self, w: &Written) -> Result<(), Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, w.warehouse_id)
            .await
            .map_err(db)?;
        for (tag, schema, table, column) in &w.inserted {
            sqlx::query(
                "DELETE FROM catalog_tag_assignments \
                  WHERE tag_id = $1 AND warehouse_id = $2 AND schema_name = $3 \
                    AND table_name = $4 AND column_name = $5",
            )
            .bind(tag)
            .bind(w.warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        record(&mut tx, &w.view, &w.previous)
            .await
            .map_err(|e| Refusal::internal("lineage", e))?;
        if !w.inserted.is_empty() {
            bump_and_recompile(&mut tx, w.warehouse_id, &bound).await?;
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    /// Step 4.
    async fn wait(&self, w: &Written) -> Result<(), Refusal> {
        match wait_for_tag_version(&self.policy, &w.required, self.limit).await {
            Ok(()) => Ok(()),
            Err(WaitError::Timeout { waiting_on }) => Err(Refusal(format!(
                "{UNCONFIRMED_REFUSAL} {} s: {}",
                seconds(self.limit),
                waiting_on.join(", ")
            ))),
            Err(WaitError::Store(e)) => Err(Refusal::internal("plugin download ledger", e)),
        }
    }

    /// Steps 4 and 5 after a successful step 3.
    async fn wait_then_commit(&self, w: Written, req: Request, next: Next) -> Response {
        if let Err(refusal) = self.wait(&w).await {
            self.compensate(&w).await;
            return refusal.into_response();
        }
        let response = next.run(req).await;
        if !response.status().is_success() {
            self.compensate(&w).await;
        }
        response
    }

    /// A session-level advisory lock per key, on a connection detached from
    /// the pool and holding no transaction. Dropping or closing the
    /// connection releases them.
    async fn lock(&self, keys: &mut [String]) -> Result<sqlx::PgConnection, sqlx::Error> {
        keys.sort();
        let mut conn = self.db.pool.acquire().await?.detach();
        let mut last: Option<&String> = None;
        for key in keys.iter() {
            if last == Some(key) {
                continue;
            }
            sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
                .bind(key)
                .execute(&mut conn)
                .await?;
            last = Some(key);
        }
        Ok(conn)
    }

    /// Create or replace (steps 0 to 5).
    async fn hold_definition(
        &self,
        prefix: &str,
        ns: &str,
        name: &str,
        def: Result<Definition, String>,
        req: Request,
        next: Next,
    ) -> Response {
        let db = |e: sqlx::Error| Refusal::internal("database", e).into_response();
        let (warehouse_id, catalog, classified) = {
            let mut conn = match self.db.pool.acquire().await {
                Ok(c) => c,
                Err(e) => return db(e),
            };
            let Some((warehouse_id, catalog)) = (match resolve(&mut conn, prefix).await {
                Ok(r) => r,
                Err(e) => return db(e),
            }) else {
                tracing::warn!(
                    prefix,
                    "view hold: no egret warehouse has this lakekeeper id; not held"
                );
                return next.run(req).await;
            };
            let classified = match profile_has_classified_column(&mut conn, warehouse_id).await {
                Ok(c) => c,
                Err(e) => return db(e),
            };
            (warehouse_id, catalog, classified)
        };

        // Step 0: nothing classified anywhere this view could read.
        if !classified {
            let response = next.run(req).await;
            if response.status().is_success() {
                if let Some(catalog) = catalog {
                    let view = view_ref(&catalog, ns, name);
                    self.record_after_commit(&view, &RecordedLineage::Missing)
                        .await;
                }
            }
            return response;
        }

        let Some(catalog) = catalog else {
            return Refusal::lineage("the view's warehouse has no catalog in egret's registry")
                .into_response();
        };
        let def = match def {
            Ok(d) => d,
            Err(e) => return Refusal::lineage(e).into_response(),
        };
        let view = view_ref(&catalog, ns, name);

        // Step 1, with no connection held across the EXPLAIN.
        let default_catalog = def.default_catalog.as_deref().unwrap_or(&catalog);
        let default_schema = def.default_schema.as_deref().unwrap_or(ns);
        let tables = match lineage::compute_lineage(
            self.runner.as_ref(),
            default_catalog,
            default_schema,
            &def.sql,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => return Refusal::lineage(e).into_response(),
        };
        let tags = {
            let mut conn = match self.db.pool.acquire().await {
                Ok(c) => c,
                Err(e) => return db(e),
            };
            match lineage::view_tag_set(&mut conn, &tables).await {
                Ok(t) => t,
                Err(e) => return Refusal::lineage(e).into_response(),
            }
        };

        // Step 2.
        if tags.is_empty() {
            let response = next.run(req).await;
            if response.status().is_success() {
                self.record_after_commit(&view, &RecordedLineage::Computed(tables))
                    .await;
            }
            return response;
        }

        // Step 3.
        let rows: Vec<Assignment> = tags
            .iter()
            .flat_map(|tag| {
                def.columns
                    .iter()
                    .map(move |c| (*tag, ns.to_string(), name.to_string(), c.clone()))
            })
            .collect();
        let written = match self
            .write(
                warehouse_id,
                &view,
                &rows,
                &RecordedLineage::Computed(tables),
            )
            .await
        {
            Ok(w) => w,
            Err(r) => return r.into_response(),
        };
        self.wait_then_commit(written, req, next).await
    }

    /// Record a lineage for a view lakekeeper has committed. A failure leaves
    /// the lineage missing or stale, which the B3 sweep repairs; it never
    /// removes a tag.
    async fn record_after_commit(&self, view: &ViewRef, lineage: &RecordedLineage) {
        let result = async {
            let mut tx = self.db.pool.begin().await?;
            record(&mut tx, view, lineage).await?;
            tx.commit().await?;
            Ok::<(), LineageError>(())
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(?view, error = %e, "view hold: lineage not recorded after commit");
        }
    }

    /// Rename: copy the source's lineage and tag assignments to the
    /// destination, hold until the plugins report the version, let lakekeeper
    /// rename, then delete the source's.
    async fn hold_rename(
        &self,
        prefix: &str,
        source: (String, String),
        destination: (String, String),
        req: Request,
        next: Next,
    ) -> Response {
        let db = |e: sqlx::Error| Refusal::internal("database", e).into_response();
        let (warehouse_id, src, dst, src_lineage, src_rows) = {
            let mut conn = match self.db.pool.acquire().await {
                Ok(c) => c,
                Err(e) => return db(e),
            };
            let Some((warehouse_id, Some(catalog))) = (match resolve(&mut conn, prefix).await {
                Ok(r) => r,
                Err(e) => return db(e),
            }) else {
                return next.run(req).await;
            };
            let src = view_ref(&catalog, &source.0, &source.1);
            let dst = view_ref(&catalog, &destination.0, &destination.1);
            let src_lineage = match lineage::recorded_lineage(&mut conn, &src).await {
                Ok(l) => l,
                Err(e) => return db(e),
            };
            let src_rows = match assignments_of(&mut conn, warehouse_id, &src).await {
                Ok(r) => r,
                Err(e) => return db(e),
            };
            (warehouse_id, src, dst, src_lineage, src_rows)
        };
        if src_rows.is_empty() && src_lineage == RecordedLineage::Missing {
            return next.run(req).await;
        }
        let rows: Vec<Assignment> = src_rows
            .iter()
            .map(|(tag, _, _, column)| (*tag, dst.schema.clone(), dst.name.clone(), column.clone()))
            .collect();
        let written = match self.write(warehouse_id, &dst, &rows, &src_lineage).await {
            Ok(w) => w,
            Err(r) => return r.into_response(),
        };
        if !rows.is_empty() {
            if let Err(refusal) = self.wait(&written).await {
                self.compensate(&written).await;
                return refusal.into_response();
            }
        }
        let response = next.run(req).await;
        if !response.status().is_success() {
            self.compensate(&written).await;
            return response;
        }
        // The source no longer exists: drop what it held.
        let result = async {
            let mut tx = self.db.pool.begin().await.map_err(|e| e.to_string())?;
            let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
                .await
                .map_err(|e| e.to_string())?;
            for (tag, schema, table, column) in &src_rows {
                sqlx::query(
                    "DELETE FROM catalog_tag_assignments \
                      WHERE tag_id = $1 AND warehouse_id = $2 AND schema_name = $3 \
                        AND table_name = $4 AND column_name = $5",
                )
                .bind(tag)
                .bind(warehouse_id)
                .bind(schema)
                .bind(table)
                .bind(column)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            }
            lineage::delete_lineage(&mut tx, &src)
                .await
                .map_err(|e| e.to_string())?;
            if !src_rows.is_empty() {
                bump_and_recompile(&mut tx, warehouse_id, &bound)
                    .await
                    .map_err(|r| r.0)?;
            }
            tx.commit().await.map_err(|e| e.to_string())
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(?src, error = %e, "view hold: the renamed view's old rows remain");
        }
        response
    }
}

fn view_ref(catalog: &str, schema: &str, name: &str) -> ViewRef {
    ViewRef {
        catalog: catalog.to_string(),
        schema: schema.to_string(),
        name: name.to_string(),
    }
}

/// `60` for 60 s; one decimal below a whole second.
fn seconds(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        d.as_secs().to_string()
    } else {
        format!("{:.1}", d.as_secs_f64())
    }
}

/// The axum middleware (module doc). Every request it does not hold passes
/// through unchanged.
pub async fn hold_view_commits(
    State(hold): State<Arc<ViewHold>>,
    req: Request,
    next: Next,
) -> Response {
    let Some((prefix, target)) = classify(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return Refusal::internal("request body", e).into_response(),
    };
    let json: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Refusal::internal("request body is not JSON", e).into_response(),
    };
    let req = Request::from_parts(parts, Body::from(bytes));

    match target {
        Target::Commit { ns, view } => {
            let Some(def) = commit_definition(&json) else {
                return next.run(req).await;
            };
            let mut keys = vec![lock_key(&prefix, &ns, &view)];
            let lock = match hold.lock(&mut keys).await {
                Ok(c) => c,
                Err(e) => return Refusal::internal("view lock", e).into_response(),
            };
            let response = hold
                .hold_definition(&prefix, &ns, &view, def, req, next)
                .await;
            release(lock).await;
            response
        }
        Target::Create { ns } => {
            let (name, def) = match create_definition(&json) {
                Ok(x) => x,
                // No name: lakekeeper refuses it, and nothing can be recorded.
                Err(_) => return next.run(req).await,
            };
            let mut keys = vec![lock_key(&prefix, &ns, &name)];
            let lock = match hold.lock(&mut keys).await {
                Ok(c) => c,
                Err(e) => return Refusal::internal("view lock", e).into_response(),
            };
            let response = hold
                .hold_definition(&prefix, &ns, &name, def, req, next)
                .await;
            release(lock).await;
            response
        }
        Target::Rename => {
            let Some((source, destination)) = rename_of(&json) else {
                return next.run(req).await;
            };
            let mut keys = vec![
                lock_key(&prefix, &source.0, &source.1),
                lock_key(&prefix, &destination.0, &destination.1),
            ];
            let lock = match hold.lock(&mut keys).await {
                Ok(c) => c,
                Err(e) => return Refusal::internal("view lock", e).into_response(),
            };
            let response = hold
                .hold_rename(&prefix, source, destination, req, next)
                .await;
            release(lock).await;
            response
        }
    }
}

fn lock_key(prefix: &str, ns: &str, name: &str) -> String {
    // `\u{1f}` cannot occur in a decoded single-level name (`decode_segment`);
    // a NUL cannot be sent as Postgres text.
    format!("egret-view-hold\u{1f}{prefix}\u{1f}{ns}\u{1f}{name}")
}

/// Close the lock connection, which releases its advisory locks.
async fn release(conn: sqlx::PgConnection) {
    if let Err(e) = conn.close().await {
        tracing::warn!(error = %e, "view hold: closing the lock connection failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_paths_trino_sends_are_classified() {
        let post = Method::POST;
        assert_eq!(
            classify(&post, "/wh1/namespaces/s/views"),
            Some(("wh1".into(), Target::Create { ns: "s".into() }))
        );
        assert_eq!(
            classify(&post, "/wh1/namespaces/s/views/v1"),
            Some((
                "wh1".into(),
                Target::Commit {
                    ns: "s".into(),
                    view: "v1".into()
                }
            ))
        );
        assert_eq!(
            classify(&post, "/wh1/views/rename"),
            Some(("wh1".into(), Target::Rename))
        );
        assert_eq!(classify(&Method::GET, "/wh1/namespaces/s/views/v1"), None);
        assert_eq!(
            classify(&Method::DELETE, "/wh1/namespaces/s/views/v1"),
            None
        );
        assert_eq!(classify(&post, "/wh1/namespaces/s/tables"), None);
        assert_eq!(classify(&post, "/wh1/namespaces/a%1Fb/views"), None);
        assert_eq!(
            classify(&post, "/wh1/namespaces/my%20s/views"),
            Some(("wh1".into(), Target::Create { ns: "my s".into() }))
        );
    }

    /// Live result §1 Q1's replace body, verbatim in its shape.
    #[test]
    fn a_replace_carries_its_sql_and_the_schema_it_added() {
        let body = json!({
            "requirements": [{"type": "assert-view-uuid", "uuid": "x"}],
            "updates": [
                {"action": "set-properties", "updates": {"trino.run-as-owner": "vgadmin"}},
                {"action": "add-schema", "schema": {"type": "struct", "schema-id": 1,
                    "fields": [{"id": 1, "name": "id", "type": "int", "required": false},
                               {"id": 2, "name": "ssn", "type": "string", "required": false}]}},
                {"action": "add-view-version", "view-version": {"version-id": 2, "schema-id": -1,
                    "default-namespace": ["s"],
                    "representations": [{"type": "sql", "sql": "SELECT id, ssn FROM ice.s.pii", "dialect": "trino"}]}},
                {"action": "set-current-view-version", "view-version-id": -1}
            ]
        });
        let def = commit_definition(&body).unwrap().unwrap();
        assert_eq!(def.sql, "SELECT id, ssn FROM ice.s.pii");
        assert_eq!(def.default_schema.as_deref(), Some("s"));
        assert_eq!(def.default_catalog, None);
        assert_eq!(def.columns, vec!["id", "ssn"]);
    }

    #[test]
    fn a_commit_without_a_new_version_is_not_held() {
        let comment =
            json!({"updates": [{"action": "set-properties", "updates": {"comment": "hello"}}]});
        assert!(commit_definition(&comment).is_none());
        assert!(commit_definition(&json!({"updates": []})).is_none());
    }

    #[test]
    fn a_version_without_trino_sql_or_columns_cannot_be_judged() {
        let v = json!({"representations": [{"type": "sql", "sql": "x", "dialect": "spark"}]});
        assert!(definition(&v, Some(&json!({"fields": [{"name": "a"}]}))).is_err());
        let v = json!({"representations": [{"type": "sql", "sql": "x", "dialect": "trino"}],
                       "default-namespace": ["a", "b"]});
        assert!(definition(&v, Some(&json!({"fields": [{"name": "a"}]}))).is_err());
        let v = json!({"representations": [{"type": "sql", "sql": "x", "dialect": "trino"}]});
        assert!(definition(&v, None).is_err());
        // An add-view-version naming a schema the commit did not add.
        let body = json!({"updates": [{"action": "add-view-version", "view-version": {
            "schema-id": 4, "representations": [{"type": "sql", "sql": "x", "dialect": "trino"}]}}]});
        assert!(commit_definition(&body).unwrap().is_err());
    }

    #[test]
    fn the_refusal_messages_name_their_reason() {
        assert_eq!(seconds(default_hold_limit()), "60");
        assert_eq!(seconds(Duration::from_millis(1500)), "1.5");
        assert!(Refusal::lineage("boom")
            .0
            .starts_with("lineage could not be computed: "));
    }
}
