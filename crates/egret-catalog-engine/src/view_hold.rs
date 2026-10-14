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
//! - commit, `POST /{prefix}/namespaces/{ns}/views/{view}`. A commit that
//!   neither adds a view version nor sets the current one (`COMMENT ON VIEW`,
//!   a replace whose updates are empty) passes through untouched. Otherwise
//!   the hold judges the version that is current after ALL the updates: the
//!   last added one when `set-current-view-version` is `-1`. A commit that
//!   makes current a version it does not add (an older stored one, or one
//!   named by id) is refused on a classified profile, because egret has not
//!   judged it;
//! - rename, `POST /{prefix}/views/rename`, which moves the view's lineage and
//!   tag assignments to the new name under the same hold.
//!
//! A multi-level namespace on any of these is refused on a classified profile
//! (the Ranger resource has one schema level), and passes otherwise.
//!
//! # Order of operations (spec B2)
//!
//! No DB transaction stays open across the EXPLAIN or the wait.
//!
//! 0. If the profile that governs the view's catalog has no classified column
//!    at all, the commit passes through with no EXPLAIN and no lock. The
//!    lineage is left missing, and the B3 sweep computes it later.
//! 1. Compute the lineage ([`lineage::compute_lineage`]), bounded by
//!    [`EXPLAIN_BUDGET`]. On failure, refuse.
//! 2. If no lineage table is classified, let the commit proceed, and record the
//!    lineage (`computed`) once lakekeeper has answered 2xx.
//! 3. Otherwise, in one transaction: write one tag assignment per view column
//!    and tag, record the lineage, bump the tag version (and recompile the
//!    bound profiles, as every catalog-tag write does). Commit.
//! 4. Wait ([`wait_for_tag_version`]) for every Trino profile bound to the
//!    view's catalog to report the new tag version. With none bound, refuse at
//!    once: no plugin could ever confirm.
//! 5. Let the commit proceed. On timeout, or when lakekeeper answers non-2xx,
//!    compensate in one transaction: delete exactly the assignment rows step 3
//!    inserted, restore the lineage recorded before step 3, and bump again.
//!
//! # One deadline
//!
//! Every held request has ONE deadline, [`ViewHold::limit`] (2 × the poll
//! interval, 60 s) from its arrival, covering the lock, the EXPLAIN and the
//! wait together. Trino was measured to tolerate a 65 s hold (live result §1
//! Q1), so a refusal arrives as a refusal rather than as a client timeout.
//!
//! # Refusal
//!
//! A `409` Iceberg REST error, which Trino prints verbatim. Its message names
//! the reason ([`LINEAGE_REFUSAL`], [`UNCONFIRMED_REFUSAL`], …) and what to
//! do about it.
//!
//! # Concurrency
//!
//! A hold that writes takes a session-level advisory lock per view name it
//! touches, with `pg_try_advisory_lock` on a connection of its own that holds
//! no transaction. A busy lock refuses at once: the other hold may run for the
//! whole deadline. Without the lock, a create that lakekeeper refuses because
//! a concurrent create of the same name won would compensate, deleting rows
//! the winner relied on. Holds are capped ([`DEFAULT_MAX_HOLDS`]); past the
//! cap a request is refused at once, so a burst cannot grow Postgres
//! connections without bound.

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
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use uuid::Uuid;

/// The reason a view is refused when egret cannot judge what it reads.
pub const LINEAGE_REFUSAL: &str = "lineage could not be computed";
/// The reason a view is refused when a plugin did not confirm the tags.
pub const UNCONFIRMED_REFUSAL: &str = "engines did not confirm the classification tags within";

/// The longest the lineage EXPLAIN may take. `EXPLAIN (TYPE IO)` plans and
/// reads no data; planning a view loads its tables' metadata from egret's own
/// catalog. 5 s is far above that, and leaves 55 s of the 60 s deadline for
/// the plugins to confirm. The production runner gets the same timeout.
pub const EXPLAIN_BUDGET: Duration = Duration::from_secs(5);

/// How many holds may be in flight at once.
pub const DEFAULT_MAX_HOLDS: usize = 8;

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
    holds: Arc<Semaphore>,
    max_holds: usize,
}

impl ViewHold {
    /// A hold with a deadline of [`default_hold_limit`] and at most
    /// [`DEFAULT_MAX_HOLDS`] in flight.
    pub fn new(db: Db, runner: Arc<dyn ExplainRunner>) -> Self {
        Self {
            policy: PolicyStore::new(db.clone()),
            db,
            runner,
            limit: default_hold_limit(),
            holds: Arc::new(Semaphore::new(DEFAULT_MAX_HOLDS)),
            max_holds: DEFAULT_MAX_HOLDS,
        }
    }

    /// The same hold with another deadline (tests).
    #[must_use]
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    /// The same hold with another cap on holds in flight.
    #[must_use]
    pub fn with_max_holds(mut self, n: usize) -> Self {
        self.holds = Arc::new(Semaphore::new(n));
        self.max_holds = n;
        self
    }

    /// The deadline of one held request, from its arrival.
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

/// Why a request is refused. Every refusal is the Iceberg `409`, and every
/// message says what to do.
struct Refusal(String);

impl Refusal {
    fn lineage(detail: impl std::fmt::Display) -> Self {
        Self(format!(
            "{LINEAGE_REFUSAL}: {detail}; check that a running Trino engine egret provisions \
             in JWT mode has the catalog attached and can plan this SQL, then retry"
        ))
    }
    fn internal(what: &str, e: impl std::fmt::Display) -> Self {
        Self(format!(
            "egret could not hold the view ({what}): {e}; retry"
        ))
    }
    fn multi_level() -> Self {
        Self(
            "the view's namespace has more than one level, which egret cannot govern on a \
             catalog with classified columns; use a single-level schema"
                .into(),
        )
    }
    fn older_version(why: &str) -> Self {
        Self(format!(
            "{why}: egret cannot judge an older view version on a catalog with classified \
             columns; replace the view with CREATE OR REPLACE VIEW instead"
        ))
    }
    fn locked(limit: Duration) -> Self {
        Self(format!(
            "another change to this view is being held; retry when it has finished (at most {} s)",
            seconds(limit)
        ))
    }
    fn full(max: usize) -> Self {
        Self(format!(
            "egret is holding {max} classified view changes already, its maximum; retry in a minute"
        ))
    }
    fn no_trino() -> Self {
        Self(
            "no Trino engine is bound to the view's catalog, so no plugin can confirm its \
             classification tags; attach a Trino engine to the catalog, then retry"
                .into(),
        )
    }
    fn overlap(e: impl std::fmt::Display) -> Self {
        Self(format!(
            "the classification tags cannot be assigned: {e}; resolve the overlapping mask or \
             row-filter policies, then retry"
        ))
    }
    fn unconfirmed(limit: Duration, waiting_on: &[String]) -> Self {
        Self(format!(
            "{UNCONFIRMED_REFUSAL} {} s: {}; check that the engines are running and polling \
             egret's Ranger endpoint, then retry",
            seconds(limit),
            waiting_on.join(", ")
        ))
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

/// The view request a path names. A namespace is decoded with lakekeeper's
/// `\u{1f}` level separator kept ([`is_multi_level`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Create { ns: String },
    Commit { ns: String, view: String },
    Rename,
}

/// `(prefix, target)` for a `POST` the hold handles, else `None`.
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

/// Percent-decode one path segment. `None` for a malformed escape or invalid
/// UTF-8, which lakekeeper refuses too.
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
    String::from_utf8(out).ok()
}

/// A lakekeeper multi-level namespace (`\u{1f}`-separated). Trino sends
/// single-level namespaces, and the Ranger resource has one schema level.
fn is_multi_level(ns: &str) -> bool {
    ns.contains('\u{1f}')
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

/// What a commit does to the view's current version.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    /// Neither adds a version nor sets the current one: passes untouched.
    Untouched,
    /// The version to judge.
    Judge(Result<Definition, String>),
    /// The current version after the commit is not one it adds.
    NotAdded(&'static str),
}

/// Resolve every update in order: the version that is current afterwards.
fn commit_change(body: &Value) -> Change {
    let Some(updates) = body.get("updates").and_then(Value::as_array) else {
        // Not a commit body; lakekeeper refuses it.
        return Change::Untouched;
    };
    let action = |u: &Value, a: &str| u.get("action").and_then(Value::as_str) == Some(a);
    let mut schemas: Vec<&Value> = Vec::new();
    let mut added: Vec<Option<&Value>> = Vec::new();
    // `Some(None)`: a set-current whose id is not an integer.
    let mut current: Option<Option<i64>> = None;
    for u in updates {
        if action(u, "add-schema") {
            if let Some(s) = u.get("schema") {
                schemas.push(s);
            }
        } else if action(u, "add-view-version") {
            added.push(u.get("view-version"));
        } else if action(u, "set-current-view-version") {
            current = Some(u.get("view-version-id").and_then(Value::as_i64));
        }
    }
    let last =
        match (added.last(), current) {
            (None, None) => return Change::Untouched,
            (Some(v), None | Some(Some(-1))) => *v,
            (None, Some(_)) => {
                return Change::NotAdded("the commit makes a stored view version current")
            }
            (Some(_), Some(_)) => return Change::NotAdded(
                "the commit makes a view version current by id rather than the one it adds last",
            ),
        };
    let Some(version) = last else {
        return Change::Judge(Err(
            "an add-view-version update carries no view version".into()
        ));
    };
    // `-1` is "the schema this commit last added"; any other id must be added
    // by this commit too, or the columns are not in the request.
    let schema = match version.get("schema-id").and_then(Value::as_i64) {
        Some(-1) => schemas.last().copied(),
        Some(id) => schemas
            .iter()
            .rev()
            .find(|s| s.get("schema-id").and_then(Value::as_i64) == Some(id))
            .copied(),
        None => None,
    };
    Change::Judge(definition(version, schema))
}

/// `{source, destination}` of a rename, each `(namespace, name)`, the
/// namespace's levels joined with `\u{1f}`. `None` for a body of another
/// shape, which lakekeeper refuses.
fn rename_of(body: &Value) -> Option<((String, String), (String, String))> {
    let side = |k: &str| -> Option<(String, String)> {
        let v = body.get(k)?;
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
    };
    Some((side("source")?, side("destination")?))
}

/// The egret warehouse a lakekeeper prefix belongs to, and its registry
/// catalog name (the name the engines and the lineage use).
async fn resolve(
    conn: &mut sqlx::PgConnection,
    prefix: &str,
) -> Result<Option<(Uuid, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT w.id, \
                (SELECT c.name FROM registry.catalogs c WHERE c.warehouse_id = w.id \
                  ORDER BY c.name LIMIT 1) \
           FROM catalog.warehouses w WHERE w.lakekeeper_warehouse_id = $1",
    )
    .bind(prefix)
    .fetch_optional(&mut *conn)
    .await
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
        .map_err(Refusal::overlap)?;
    Ok(())
}

/// The view request's state before any hold work: which egret warehouse and
/// catalog, and whether its profile is classified.
struct Resolved {
    warehouse_id: Uuid,
    catalog: Option<String>,
    classified: bool,
}

/// The lock connection and the semaphore permit of one hold; [`Self::release`]
/// frees both.
struct Held {
    conn: sqlx::PgConnection,
    _permit: OwnedSemaphorePermit,
}

impl Held {
    /// Close the lock connection, which releases its advisory locks.
    async fn release(self) {
        if let Err(e) = self.conn.close().await {
            tracing::warn!(error = %e, "view hold: closing the lock connection failed");
        }
    }
}

impl ViewHold {
    async fn resolve(&self, prefix: &str) -> Result<Option<Resolved>, Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut conn = self.db.pool.acquire().await.map_err(db)?;
        let Some((warehouse_id, catalog)) = resolve(&mut conn, prefix).await.map_err(db)? else {
            return Ok(None);
        };
        let classified = profile_has_classified_column(&mut conn, warehouse_id)
            .await
            .map_err(db)?;
        Ok(Some(Resolved {
            warehouse_id,
            catalog,
            classified,
        }))
    }

    /// A permit, then a session-level `pg_try_advisory_lock` per key on a
    /// connection detached from the pool. Full or busy refuses at once.
    async fn hold(&self, prefix: &str, views: &[(&str, &str)]) -> Result<Held, Refusal> {
        let permit = self
            .holds
            .clone()
            .try_acquire_owned()
            .map_err(|_| Refusal::full(self.max_holds))?;
        let mut keys: Vec<String> = views
            .iter()
            .map(|(ns, name)| lock_key(prefix, ns, name))
            .collect();
        keys.sort();
        keys.dedup();
        let db = |e: sqlx::Error| Refusal::internal("view lock", e);
        let mut conn = self.db.pool.acquire().await.map_err(db)?.detach();
        for key in &keys {
            let got: bool =
                sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                    .bind(key)
                    .fetch_one(&mut conn)
                    .await
                    .map_err(db)?;
            if !got {
                // Closing releases any key this loop already took.
                let _ = conn.close().await;
                return Err(Refusal::locked(self.limit));
            }
        }
        Ok(Held {
            conn,
            _permit: permit,
        })
    }

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
                .map_err(Refusal::overlap)?;
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
        delete_rows(&mut tx, w.warehouse_id, &w.inserted)
            .await
            .map_err(db)?;
        record(&mut tx, &w.view, &w.previous)
            .await
            .map_err(|e| Refusal::internal("lineage", e))?;
        if !w.inserted.is_empty() {
            bump_and_recompile(&mut tx, w.warehouse_id, &bound).await?;
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    /// Step 4, until `deadline`. No Trino engine bound refuses at once.
    async fn wait(&self, w: &Written, deadline: Instant) -> Result<(), Refusal> {
        if w.required.is_empty() {
            return Err(Refusal::no_trino());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match wait_for_tag_version(&self.policy, &w.required, left).await {
            Ok(()) => Ok(()),
            Err(WaitError::Timeout { waiting_on }) => {
                Err(Refusal::unconfirmed(self.limit, &waiting_on))
            }
            Err(WaitError::Store(e)) => Err(Refusal::internal("plugin download ledger", e)),
        }
    }

    /// Steps 4 and 5 after a successful step 3.
    async fn wait_then_commit(
        &self,
        w: Written,
        deadline: Instant,
        req: Request,
        next: Next,
    ) -> Response {
        if let Err(refusal) = self.wait(&w, deadline).await {
            self.compensate(&w).await;
            return refusal.into_response();
        }
        let response = next.run(req).await;
        if !response.status().is_success() {
            self.compensate(&w).await;
        }
        response
    }

    /// Create, replace, or a commit that sets the current version.
    #[allow(clippy::too_many_arguments)]
    async fn hold_definition(
        &self,
        deadline: Instant,
        prefix: &str,
        ns: &str,
        name: &str,
        change: Change,
        req: Request,
        next: Next,
    ) -> Response {
        let r = match self.resolve(prefix).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                tracing::warn!(
                    prefix,
                    "view hold: no egret warehouse has this lakekeeper id; not held"
                );
                return next.run(req).await;
            }
            Err(refusal) => return refusal.into_response(),
        };

        // Step 0: nothing classified anywhere this view could read. No lock.
        if !r.classified {
            let response = next.run(req).await;
            if response.status().is_success() && !is_multi_level(ns) {
                if let Some(catalog) = &r.catalog {
                    let view = view_ref(catalog, ns, name);
                    self.record_after_commit(&view, &RecordedLineage::Missing)
                        .await;
                }
            }
            return response;
        }
        if is_multi_level(ns) {
            return Refusal::multi_level().into_response();
        }
        let def = match change {
            Change::Untouched => return next.run(req).await,
            Change::NotAdded(why) => return Refusal::older_version(why).into_response(),
            Change::Judge(Err(e)) => return Refusal::lineage(e).into_response(),
            Change::Judge(Ok(d)) => d,
        };
        let Some(catalog) = r.catalog else {
            return Refusal::lineage("the view's warehouse has no catalog in egret's registry")
                .into_response();
        };
        let held = match self.hold(prefix, &[(ns, name)]).await {
            Ok(h) => h,
            Err(refusal) => return refusal.into_response(),
        };
        let response = self
            .judge(deadline, r.warehouse_id, &catalog, ns, name, def, req, next)
            .await;
        held.release().await;
        response
    }

    /// Steps 1 to 5 for one definition, under the hold's lock.
    #[allow(clippy::too_many_arguments)]
    async fn judge(
        &self,
        deadline: Instant,
        warehouse_id: Uuid,
        catalog: &str,
        ns: &str,
        name: &str,
        def: Definition,
        req: Request,
        next: Next,
    ) -> Response {
        let view = view_ref(catalog, ns, name);

        // Step 1, bounded, with no connection held across the EXPLAIN.
        let budget = EXPLAIN_BUDGET.min(deadline.saturating_duration_since(Instant::now()));
        let default_catalog = def.default_catalog.as_deref().unwrap_or(catalog);
        let default_schema = def.default_schema.as_deref().unwrap_or(ns);
        let explained = tokio::time::timeout(
            budget,
            lineage::compute_lineage(
                self.runner.as_ref(),
                default_catalog,
                default_schema,
                &def.sql,
            ),
        )
        .await;
        let tables = match explained {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return Refusal::lineage(e).into_response(),
            Err(_) => {
                return Refusal::lineage(format!(
                    "the EXPLAIN did not answer within {} s",
                    seconds(budget)
                ))
                .into_response()
            }
        };
        let tags = {
            let mut conn = match self.db.pool.acquire().await {
                Ok(c) => c,
                Err(e) => return Refusal::internal("database", e).into_response(),
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
        self.wait_then_commit(written, deadline, req, next).await
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

    /// What `view` carries now: its lineage and its assignment rows.
    async fn state_of(
        &self,
        warehouse_id: Uuid,
        view: &ViewRef,
    ) -> Result<(RecordedLineage, Vec<Assignment>), Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut conn = self.db.pool.acquire().await.map_err(db)?;
        let lineage = lineage::recorded_lineage(&mut conn, view)
            .await
            .map_err(db)?;
        let rows = assignments_of(&mut conn, warehouse_id, view)
            .await
            .map_err(db)?;
        Ok((lineage, rows))
    }

    /// Rename: copy the source's lineage and tag assignments to the
    /// destination, hold until the plugins report the version, let lakekeeper
    /// rename, then delete the source's.
    async fn hold_rename(
        &self,
        deadline: Instant,
        prefix: &str,
        source: (String, String),
        destination: (String, String),
        req: Request,
        next: Next,
    ) -> Response {
        let r = match self.resolve(prefix).await {
            Ok(Some(r)) => r,
            Ok(None) => return next.run(req).await,
            Err(refusal) => return refusal.into_response(),
        };
        let Some(catalog) = r.catalog.as_deref() else {
            return next.run(req).await;
        };
        let warehouse_id = r.warehouse_id;
        let src = view_ref(catalog, &source.0, &source.1);
        let dst = view_ref(catalog, &destination.0, &destination.1);
        let multi = is_multi_level(&src.schema) || is_multi_level(&dst.schema);
        if multi && r.classified {
            return Refusal::multi_level().into_response();
        }
        // Nothing can be recorded under a multi-level name, so a multi-level
        // source carries nothing. A single-level source that does carry
        // something may not move where it cannot follow.
        if is_multi_level(&src.schema) {
            return next.run(req).await;
        }
        let carries = match self.state_of(warehouse_id, &src).await {
            Ok((l, rows)) => l != RecordedLineage::Missing || !rows.is_empty(),
            Err(refusal) => return refusal.into_response(),
        };
        if !carries {
            return next.run(req).await;
        }
        if multi {
            return Refusal::multi_level().into_response();
        }

        let held = match self
            .hold(
                prefix,
                &[(&src.schema, &src.name), (&dst.schema, &dst.name)],
            )
            .await
        {
            Ok(h) => h,
            Err(refusal) => return refusal.into_response(),
        };
        let response = self
            .move_view(deadline, warehouse_id, &src, &dst, req, next)
            .await;
        held.release().await;
        response
    }

    /// The rename's hold, under its lock.
    async fn move_view(
        &self,
        deadline: Instant,
        warehouse_id: Uuid,
        src: &ViewRef,
        dst: &ViewRef,
        req: Request,
        next: Next,
    ) -> Response {
        // Re-read under the lock.
        let (src_lineage, src_rows) = match self.state_of(warehouse_id, src).await {
            Ok(s) => s,
            Err(refusal) => return refusal.into_response(),
        };
        let rows: Vec<Assignment> = src_rows
            .iter()
            .map(|(tag, _, _, column)| (*tag, dst.schema.clone(), dst.name.clone(), column.clone()))
            .collect();
        let written = match self.write(warehouse_id, dst, &rows, &src_lineage).await {
            Ok(w) => w,
            Err(r) => return r.into_response(),
        };
        if !rows.is_empty() {
            if let Err(refusal) = self.wait(&written, deadline).await {
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
            delete_rows(&mut tx, warehouse_id, &src_rows)
                .await
                .map_err(|e| e.to_string())?;
            lineage::delete_lineage(&mut tx, src)
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

/// Delete exactly `rows` from `warehouse_id`.
async fn delete_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    warehouse_id: Uuid,
    rows: &[Assignment],
) -> Result<(), sqlx::Error> {
    for (tag, schema, table, column) in rows {
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
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
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
    // The one deadline of this request (module doc), from its arrival.
    let deadline = Instant::now() + hold.limit;
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
        Target::Commit { ns, view } => match commit_change(&json) {
            Change::Untouched => next.run(req).await,
            change => {
                hold.hold_definition(deadline, &prefix, &ns, &view, change, req, next)
                    .await
            }
        },
        Target::Create { ns } => match create_definition(&json) {
            Ok((name, def)) => {
                hold.hold_definition(deadline, &prefix, &ns, &name, Change::Judge(def), req, next)
                    .await
            }
            // No name: lakekeeper refuses it, and nothing can be recorded.
            Err(_) => next.run(req).await,
        },
        Target::Rename => match rename_of(&json) {
            Some((source, destination)) => {
                hold.hold_rename(deadline, &prefix, source, destination, req, next)
                    .await
            }
            None => next.run(req).await,
        },
    }
}

/// The advisory-lock key of one view (exposed for tests).
pub fn view_lock_key(prefix: &str, ns: &str, name: &str) -> String {
    lock_key(prefix, ns, name)
}

fn lock_key(prefix: &str, ns: &str, name: &str) -> String {
    // A NUL cannot be sent as Postgres text; `\u{1e}` occurs in no name Trino
    // sends.
    format!("egret-view-hold\u{1e}{prefix}\u{1e}{ns}\u{1e}{name}")
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
        // A multi-level namespace is classified, and recognised as one.
        let Some((_, Target::Create { ns })) = classify(&post, "/wh1/namespaces/a%1Fb/views")
        else {
            panic!("multi-level create not classified");
        };
        assert!(is_multi_level(&ns));
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
        let Change::Judge(Ok(def)) = commit_change(&body) else {
            panic!("not judged");
        };
        assert_eq!(def.sql, "SELECT id, ssn FROM ice.s.pii");
        assert_eq!(def.default_schema.as_deref(), Some("s"));
        assert_eq!(def.default_catalog, None);
        assert_eq!(def.columns, vec!["id", "ssn"]);
    }

    #[test]
    fn a_commit_without_a_new_version_is_not_held() {
        let comment =
            json!({"updates": [{"action": "set-properties", "updates": {"comment": "hello"}}]});
        assert_eq!(commit_change(&comment), Change::Untouched);
        assert_eq!(commit_change(&json!({"updates": []})), Change::Untouched);
    }

    #[test]
    fn the_current_version_after_all_updates_is_the_one_judged() {
        let v = |sql: &str| {
            json!({"action": "add-view-version", "view-version": {"schema-id": -1,
                "representations": [{"type": "sql", "sql": sql, "dialect": "trino"}]}})
        };
        let schema =
            json!({"action": "add-schema", "schema": {"schema-id": 1, "fields": [{"name": "a"}]}});
        let current =
            |id: i64| json!({"action": "set-current-view-version", "view-version-id": id});
        // Two versions, `-1`: the last added is judged.
        let body = json!({"updates": [schema, v("first"), v("second"), current(-1)]});
        let Change::Judge(Ok(def)) = commit_change(&body) else {
            panic!("not judged");
        };
        assert_eq!(def.sql, "second");
        // An explicit id, or a set-current with nothing added, is not judged.
        for body in [
            json!({"updates": [schema, v("first"), v("second"), current(1)]}),
            json!({"updates": [current(1)]}),
            json!({"updates": [current(-1)]}),
            json!({"updates": [{"action": "set-current-view-version", "view-version-id": "x"}]}),
        ] {
            assert!(
                matches!(commit_change(&body), Change::NotAdded(_)),
                "{body}"
            );
        }
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
        assert!(matches!(commit_change(&body), Change::Judge(Err(_))));
    }

    #[test]
    fn a_rename_keeps_every_namespace_level() {
        let body = json!({"source": {"namespace": ["a", "b"], "name": "v"},
                          "destination": {"namespace": ["s"], "name": "v"}});
        let (src, dst) = rename_of(&body).unwrap();
        assert!(is_multi_level(&src.0));
        assert_eq!(dst, ("s".to_string(), "v".to_string()));
    }

    #[test]
    fn the_refusal_messages_name_their_reason_and_what_to_do() {
        assert_eq!(seconds(default_hold_limit()), "60");
        assert_eq!(seconds(Duration::from_millis(1500)), "1.5");
        assert!(Refusal::lineage("boom")
            .0
            .starts_with("lineage could not be computed: boom; "));
        for r in [
            Refusal::lineage("x"),
            Refusal::internal("db", "x"),
            Refusal::multi_level(),
            Refusal::older_version("x"),
            Refusal::locked(default_hold_limit()),
            Refusal::full(8),
            Refusal::no_trino(),
            Refusal::overlap("x"),
            Refusal::unconfirmed(default_hold_limit(), &["p".into()]),
        ] {
            assert!(
                [
                    "retry",
                    "use a single-level",
                    "CREATE OR REPLACE",
                    "attach a Trino"
                ]
                .iter()
                .any(|w| r.0.contains(w)),
                "says what to do: {}",
                r.0
            );
        }
    }
}
