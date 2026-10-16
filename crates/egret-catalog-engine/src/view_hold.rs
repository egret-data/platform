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
//!   the hold replays the updates in order, as the view builder applies them
//!   ([`commit_change`]), and judges the version that is current at the end.
//!   A stored version made current by id is read from the view's metadata
//!   through lakekeeper's own router and judged like a new one; a version
//!   that cannot be resolved or loaded is refused on a classified profile;
//! - rename, `POST /{prefix}/views/rename`, which moves the view's lineage and
//!   tag assignments to the new name under the same hold;
//! - drop, `DELETE /{prefix}/namespaces/{ns}/views/{view}` (spec B3): after
//!   lakekeeper answers 2xx, the view's lineage and its lineage-derived tag
//!   rows are deleted. A tag an admin put on the view by hand stays.
//!
//! Every tag row the hold derives is marked `from_lineage` (migration 0188),
//! and it records the view's columns beside the lineage, so that B3
//! (`egret_perm_compile::view_tags`) can re-tag the view when a base changes.
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
//! Every held request has ONE deadline, [`ViewHold::limit`] (3 × the poll
//! interval, 90 s) from its arrival, covering the lock, the EXPLAIN and the
//! wait together. A confirmation takes one to two intervals: a plugin
//! downloads the new tags on its next poll and reports the version only on
//! the poll after (live result §2.6 I1, where a 2-interval deadline left a
//! 0.7 s margin). The third interval is that margin (user ruling 2026-10-15).
//! Trino 483's Iceberg REST client was measured to tolerate the full 90 s
//! hold with no client timeout and no retry (live result §3.5: held commits
//! of 54-60 s, and a refusal at 90.2 s server side, 91.2 s at the client, in
//! one request).
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
//!
//! The best-effort path (step 0, a profile with no classified column) has a
//! cap of its own, of the same size, taken before its lock connection (final
//! review I-1). Past it, or with the view's lock busy, the commit passes at
//! once with no EXPLAIN and no lock connection, and one bounded pooled
//! transaction records the lineage `failed` if the view's lock is free.
//!
//! Once a hold has its lock, the rest of the request runs in a task of its
//! own (final review I-2): a client that disconnects mid-wait drops only the
//! handler future, so the wait, the commit and any compensation still run.

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
/// The reason a view is refused when egret's classification writes stayed
/// busy past the hold's deadline (fix round 2, N4).
pub const BUSY_REFUSAL: &str = "egret could not write the view's classification tags";
/// The reason a view is refused when a plugin did not confirm the tags.
pub const UNCONFIRMED_REFUSAL: &str = "engines did not confirm the classification tags within";

/// The longest the lineage EXPLAIN may take. `EXPLAIN (TYPE IO)` plans and
/// reads no data; planning a view loads its tables' metadata from egret's own
/// catalog. 5 s is far above that, and leaves 85 s of the 90 s deadline for
/// the plugins to confirm. The production runner gets the same timeout.
pub const EXPLAIN_BUDGET: Duration = Duration::from_secs(5);

/// How many holds may be in flight at once.
pub const DEFAULT_MAX_HOLDS: usize = 8;

/// Largest view request body the hold reads.
const MAX_BODY: usize = 16 << 20;

/// The hold limit: 3 × the plugin poll interval (spec B2, user ruling
/// 2026-10-15), 90 s today.
pub fn default_hold_limit() -> Duration {
    Duration::from_millis(
        3 * u64::from(egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS),
    )
}

/// The hold (module doc). Install it with
/// [`crate::mount::mount_catalog_engine`].
///
/// `Clone` is cheap (every field is shared): a held request's work after its
/// lock runs on a clone, in a task of its own (final review I-2).
#[derive(Clone)]
pub struct ViewHold {
    db: Db,
    policy: PolicyStore,
    runner: Arc<dyn ExplainRunner>,
    limit: Duration,
    holds: Arc<Semaphore>,
    /// The best-effort path's own cap (final review I-1): a commit on a
    /// profile with no classified column takes one of these before its lock
    /// connection and its EXPLAIN, or passes through without either. A
    /// sibling of `holds`, so a burst of unclassified commits cannot refuse a
    /// classified one.
    best_effort: Arc<Semaphore>,
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
            best_effort: Arc::new(Semaphore::new(DEFAULT_MAX_HOLDS)),
            max_holds: DEFAULT_MAX_HOLDS,
        }
    }

    /// The same hold with another deadline (tests).
    #[must_use]
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }

    /// The same hold with another cap on holds in flight, and the same cap
    /// on best-effort records in flight.
    #[must_use]
    pub fn with_max_holds(mut self, n: usize) -> Self {
        self.holds = Arc::new(Semaphore::new(n));
        self.best_effort = Arc::new(Semaphore::new(n));
        self.max_holds = n;
        self
    }

    /// The deadline of one held request, from its arrival.
    pub fn limit(&self) -> Duration {
        self.limit
    }
}

/// One `catalog_tag_assignments` row: tag, schema, table, column, and
/// whether the lineage derived it (`from_lineage`, migration 0188). Every row
/// the hold derives is `true`; a rename carries each row's own flag.
type Assignment = (Uuid, String, String, String, bool);

/// What a hold wrote in step 3, so that compensation removes exactly it.
struct Written {
    warehouse_id: Uuid,
    view: ViewRef,
    /// The rows this hold inserted. A row that already existed is not here.
    inserted: Vec<Assignment>,
    /// The view's lineage before this hold wrote its own.
    previous: RecordedLineage,
    /// The view's recorded columns before this hold wrote its own.
    previous_columns: Option<Vec<String>>,
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
    fn create_unjudgeable(why: impl std::fmt::Display) -> Self {
        Self(format!(
            "{why}: egret cannot judge this view definition on a catalog with classified \
             columns; create the view from Trino, with Trino SQL and a single-level default \
             schema"
        ))
    }
    fn unjudgeable(why: impl std::fmt::Display) -> Self {
        Self(format!(
            "{why}: egret cannot judge this view version on a catalog with classified columns; \
             change the view's definition, or DROP and CREATE it"
        ))
    }
    fn locked(limit: Duration) -> Self {
        Self(format!(
            "another change to this view is being held, or the lineage sweep is recomputing it; \
             retry when it has finished (at most {} s)",
            seconds(limit)
        ))
    }
    fn busy(limit: Duration) -> Self {
        Self(format!(
            "{BUSY_REFUSAL} within the view's deadline ({} s), because other classification \
             changes were being written; retry",
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
    Create {
        ns: String,
    },
    Commit {
        ns: String,
        view: String,
    },
    Rename,
    /// `DELETE …/namespaces/{ns}/views/{view}` (spec B3).
    Drop {
        ns: String,
        view: String,
    },
}

/// `(prefix, target)` for a request the hold handles, else `None`.
fn classify(method: &Method, path: &str) -> Option<(String, Target)> {
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if method == Method::DELETE {
        return match segs.as_slice() {
            [prefix, "namespaces", ns, "views", view] => Some((
                decode_segment(prefix)?,
                Target::Drop {
                    ns: decode_segment(ns)?,
                    view: decode_segment(view)?,
                },
            )),
            _ => None,
        };
    }
    if method != Method::POST {
        return None;
    }
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
pub(crate) struct Definition {
    pub(crate) sql: String,
    pub(crate) default_catalog: Option<String>,
    pub(crate) default_schema: Option<String>,
    pub(crate) columns: Vec<String>,
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

/// A create's view name and its version. The name is read first: without it
/// nothing can be held or recorded.
fn create_definition(body: &Value) -> Result<(String, Result<Pending, String>), String> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the create request names no view")?
        .to_string();
    let pending = match (body.get("view-version"), body.get("schema")) {
        (Some(version), Some(schema)) => Ok(Pending::Request {
            version: version.clone(),
            schema: Ok(SchemaSource::Request(schema.clone())),
        }),
        (None, _) => Err("the create request carries no view version".to_string()),
        (_, None) => Err("the create request carries no schema".to_string()),
    };
    Ok((name, pending))
}

/// Where the schema of an added version comes from.
#[derive(Debug, Clone, PartialEq)]
enum SchemaSource {
    /// An `add-schema` of this request, resolved positionally.
    Request(Value),
    /// A schema the view already stores, by id.
    Stored(i64),
}

/// The version a commit leaves current, before any stored metadata is read.
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    /// A version this request adds, with its schema.
    Request {
        version: Value,
        schema: Result<SchemaSource, String>,
    },
    /// A version the view already stores, by id.
    Stored(i64),
}

impl Pending {
    fn needs_stored_metadata(&self) -> bool {
        matches!(
            self,
            Pending::Stored(_)
                | Pending::Request {
                    schema: Ok(SchemaSource::Stored(_)),
                    ..
                }
        )
    }
}

/// What a commit does to the view's current version.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    /// Neither adds a version nor sets the current one: passes untouched.
    Untouched,
    /// The version to judge, or why the updates cannot be resolved to one.
    Judge(Result<Pending, String>),
}

/// What a `set-current-view-version` points at, at its position.
enum Current {
    /// The version added at this index of the request.
    Added(usize),
    /// A stored version, by id.
    Stored(i64),
    Invalid(&'static str),
}

/// The version that is current after the commit (N1, N2), by replaying the
/// updates IN ORDER with the view builder's "last added" bookkeeping
/// (iceberg-rust `ViewMetadataBuilder`, which lakekeeper applies):
///
/// - `add-schema` makes that schema the last added one;
/// - `add-view-version` with `schema-id: -1` uses the schema last added
///   BEFORE it, and becomes the last added version. A positive schema id
///   names a stored schema; alongside an `add-schema` it is refused, because
///   the builder may renumber the added schema;
/// - `set-current-view-version -1` binds to the version last added BEFORE
///   it; an explicit id names a stored version. The last one wins.
///
/// A commit with adds but no `set-current-view-version` leaves the current
/// version where it was, so it is [`Change::Untouched`]: what a reader sees
/// does not change, so neither its recorded lineage nor its tags may. Judging
/// the added version would record the lineage of SQL that is not current,
/// and B3 would then follow the wrong tables. An added version can become
/// current only through a later `set-current-view-version`, which is judged
/// (a stored version by id is loaded, N3).
fn commit_change(body: &Value) -> Change {
    let Some(updates) = body.get("updates").and_then(Value::as_array) else {
        // Not a commit body; lakekeeper refuses it.
        return Change::Untouched;
    };
    let action = |u: &Value, a: &str| u.get("action").and_then(Value::as_str) == Some(a);
    let mut schemas_added = false;
    let mut last_schema: Option<Value> = None;
    let mut added: Vec<Result<Pending, String>> = Vec::new();
    let mut last_added: Option<usize> = None;
    let mut current: Option<Current> = None;
    for u in updates {
        if action(u, "add-schema") {
            if let Some(schema) = u.get("schema") {
                schemas_added = true;
                last_schema = Some(schema.clone());
            }
        } else if action(u, "add-view-version") {
            let entry = match u.get("view-version") {
                None => Err("an add-view-version update carries no view version".to_string()),
                Some(version) => {
                    let schema = match version.get("schema-id").and_then(Value::as_i64) {
                        Some(-1) => last_schema.clone().map(SchemaSource::Request).ok_or_else(|| {
                            "an added view version uses schema -1, but no schema is added before it"
                                .to_string()
                        }),
                        Some(id) if !schemas_added => Ok(SchemaSource::Stored(id)),
                        Some(_) => Err(
                            "an added view version names its schema by id beside an add-schema, \
                             which the catalog may renumber"
                                .to_string(),
                        ),
                        None => Err("an added view version names no schema".to_string()),
                    };
                    Ok(Pending::Request {
                        version: version.clone(),
                        schema,
                    })
                }
            };
            added.push(entry);
            last_added = Some(added.len() - 1);
        } else if action(u, "set-current-view-version") {
            current = Some(match u.get("view-version-id").and_then(Value::as_i64) {
                Some(-1) => match last_added {
                    Some(i) => Current::Added(i),
                    None => Current::Invalid(
                        "the commit sets the current version to -1 before adding any version",
                    ),
                },
                Some(id) => Current::Stored(id),
                None => Current::Invalid("the commit sets the current version to a non-integer id"),
            });
        }
    }
    let index = match current {
        // Adds, or nothing, with the current version unchanged.
        None => return Change::Untouched,
        Some(Current::Added(i)) => i,
        Some(Current::Stored(id)) => return Change::Judge(Ok(Pending::Stored(id))),
        Some(Current::Invalid(why)) => return Change::Judge(Err(why.to_string())),
    };
    Change::Judge(added.swap_remove(index))
}

/// The definition `pending` names. `stored` is the view's current metadata
/// (`ViewMetadata` JSON), required when a part of it is stored; `None` makes
/// such a part fail.
fn resolve_pending(pending: Pending, stored: Option<&Value>) -> Result<Definition, String> {
    let stored_schema = |id: i64| -> Result<Value, String> {
        stored
            .and_then(|m| m.get("schemas"))
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|s| s.get("schema-id").and_then(Value::as_i64) == Some(id))
            })
            .cloned()
            .ok_or_else(|| format!("the view's stored schema {id} cannot be loaded"))
    };
    match pending {
        Pending::Request { version, schema } => {
            let schema = match schema? {
                SchemaSource::Request(s) => s,
                SchemaSource::Stored(id) => stored_schema(id)?,
            };
            definition(&version, Some(&schema))
        }
        Pending::Stored(id) => {
            let version = stored
                .and_then(|m| m.get("versions"))
                .and_then(Value::as_array)
                .and_then(|a| {
                    a.iter()
                        .find(|v| v.get("version-id").and_then(Value::as_i64) == Some(id))
                })
                .cloned()
                .ok_or_else(|| format!("the view's stored version {id} cannot be loaded"))?;
            let schema_id = version
                .get("schema-id")
                .and_then(Value::as_i64)
                .ok_or_else(|| format!("the view's stored version {id} names no schema"))?;
            definition(&version, Some(&stored_schema(schema_id)?))
        }
    }
}

/// The definition of the version that is current in a view's stored
/// metadata (`ViewMetadata` JSON). The B3 sweep reads it.
pub(crate) fn current_definition(metadata: &Value) -> Result<Definition, String> {
    let id = metadata
        .get("current-version-id")
        .and_then(Value::as_i64)
        .ok_or("the view's metadata names no current version")?;
    resolve_pending(Pending::Stored(id), Some(metadata))
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
pub(crate) async fn profile_has_classified_column(
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
/// failed, or (for `Missing`) nothing at all. `columns` are the view's columns
/// (B3 re-tags every column of the view when a base changes); a missing
/// lineage has none.
async fn record(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    lineage: &RecordedLineage,
    columns: Option<&[String]>,
) -> Result<(), LineageError> {
    match lineage {
        RecordedLineage::Missing => return Ok(lineage::delete_lineage(conn, view).await?),
        RecordedLineage::Failed { detail } => {
            lineage::record_lineage_failure(conn, view, detail).await?
        }
        RecordedLineage::Computed(tables) => lineage::write_lineage(conn, view, tables).await?,
    }
    Ok(lineage::set_view_columns(conn, view, columns).await?)
}

/// The assignment rows `view` holds in `warehouse_id`.
async fn assignments_of(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    view: &ViewRef,
) -> Result<Vec<Assignment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT tag_id, schema_name, table_name, column_name, from_lineage \
           FROM catalog_tag_assignments \
          WHERE warehouse_id = $1 AND schema_name = $2 AND table_name = $3 \
          ORDER BY tag_id, column_name",
    )
    .bind(warehouse_id)
    .bind(&view.schema)
    .bind(&view.name)
    .fetch_all(&mut *conn)
    .await
}

/// The shortest lock wait a hold's write allows itself, even past its
/// deadline (fix round 2, N4).
const MIN_LOCK_WAIT: Duration = Duration::from_secs(1);

/// A database error in a hold's write. A lock wait that ran past the
/// deadline (`lock_timeout`, SQLSTATE 55P03) is the busy refusal; anything
/// else is internal.
fn db_refusal(limit: Duration, e: sqlx::Error) -> Refusal {
    if is_lock_timeout(&e) {
        Refusal::busy(limit)
    } else {
        Refusal::internal("database", e)
    }
}

fn is_lock_timeout(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "55P03")
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
        .map_err(|e| {
            if is_lock_timeout(&e) {
                Refusal(format!(
                    "{BUSY_REFUSAL} in time, because other classification changes were being \
                     written; retry"
                ))
            } else {
                Refusal::internal("tag version", e)
            }
        })?;
    egret_perm_compile::recompile_services_in_tx(tx, bound)
        .await
        .map_err(Refusal::overlap)?;
    Ok(())
}

/// The rows a hold's step-3 write puts on the view.
enum Rows<'a> {
    /// These rows, as they are (a rename carries the source's).
    Fixed(&'a [Assignment]),
    /// The judged lineage's tag set, computed inside the write, on every
    /// column.
    Derived,
}

/// What a view carries in egret: its lineage, its tag rows and its recorded
/// columns.
struct ViewState {
    lineage: RecordedLineage,
    rows: Vec<Assignment>,
    columns: Option<Vec<String>>,
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

    /// Step 3: one transaction that writes the rows, records `lineage` for
    /// `view`, and bumps the tag version when a row is new.
    ///
    /// Task 9 fix round 1 (I2): it takes the view-tag lock first, and for a
    /// judged view it computes the tag set HERE, inside the locked
    /// transaction, so a base classification either committed before (and is
    /// read) or waits for this commit (and then re-tags the view from this
    /// lineage).
    ///
    /// Fix round 2 (N4): every lock wait in it is bounded by the remaining
    /// deadline (`lock_timeout`, at least [`MIN_LOCK_WAIT`]). A wait past it
    /// fails the transaction, which rolls back, and the hold refuses cleanly
    /// instead of committing late.
    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        deadline: Instant,
        warehouse_id: Uuid,
        view: &ViewRef,
        rows: Rows<'_>,
        lineage: &RecordedLineage,
        columns: Option<&[String]>,
    ) -> Result<Written, Refusal> {
        let limit = self.limit;
        let db = move |e: sqlx::Error| db_refusal(limit, e);
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(MIN_LOCK_WAIT);
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(format!("{}ms", wait.as_millis()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
            .await
            .map_err(db)?;
        let mut rows: Vec<Assignment> = match rows {
            Rows::Fixed(rows) => rows.to_vec(),
            Rows::Derived => {
                let tables = lineage
                    .tables()
                    .map_err(|e| Refusal::internal("lineage", e))?
                    .clone();
                let tags = lineage::view_tag_set(&mut tx, &tables)
                    .await
                    .map_err(Refusal::lineage)?;
                tags.iter()
                    .flat_map(|tag| {
                        columns.unwrap_or_default().iter().map(move |c| {
                            (
                                *tag,
                                view.schema.clone(),
                                view.name.clone(),
                                c.clone(),
                                true,
                            )
                        })
                    })
                    .collect()
            }
        };
        // M2: one row order, (tag, column), as the B3 retag writes.
        rows.sort_by(|a, b| (a.0, &a.3).cmp(&(b.0, &b.3)));
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
            .await
            .map_err(db)?;
        let overlaps_before = egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &bound)
            .await
            .map_err(|e| Refusal::internal("overlap snapshot", e))?;
        let previous = lineage::recorded_lineage(&mut tx, view).await.map_err(db)?;
        let previous_columns = lineage::recorded_view_columns(&mut tx, view)
            .await
            .map_err(db)?;
        let mut inserted = Vec::new();
        for row in &rows {
            let (tag, schema, table, column, from_lineage) = row;
            // A row that exists already keeps its provenance: an admin's
            // manual tag stays manual.
            let new: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO catalog_tag_assignments \
                     (tag_id, warehouse_id, schema_name, table_name, column_name, from_lineage) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \
                 DO NOTHING RETURNING tag_id",
            )
            .bind(tag)
            .bind(warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .bind(from_lineage)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if new.is_some() {
                inserted.push(row.clone());
            }
        }
        record(&mut tx, view, lineage, columns)
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
            previous_columns,
            required,
        })
    }

    /// Step 5's compensation: delete the rows `w` inserted that still carry
    /// the provenance they were inserted with (never a row an admin has since
    /// made manual, I1(a); a rename's own manual copies go, N1), restore the
    /// lineage recorded before it, re-derive that lineage's tags (I1(b): a
    /// base of the previous lineage may have been classified meanwhile, with
    /// a tag whose rows this hold had inserted), and bump again.
    ///
    /// If the re-derivation is refused (it would complete an overlapping mask
    /// pair), compensation runs again without it, so the hold's own rows
    /// still go; the next base write or sweep re-derives. A failure is
    /// logged; what it leaves behind over-tags a view that was not committed.
    async fn compensate(&self, w: &Written) {
        let result = match self.try_compensate(w, true).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    view = ?w.view,
                    error = %e.0,
                    "view hold: compensation could not re-derive the previous lineage's tags; \
                     compensating without"
                );
                self.try_compensate(w, false).await
            }
        };
        if let Err(e) = result {
            tracing::error!(
                view = ?w.view,
                error = %e.0,
                "view hold could not compensate; the uncommitted view's tags and lineage remain"
            );
        }
    }

    async fn try_compensate(&self, w: &Written, rederive: bool) -> Result<(), Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut tx = self.db.pool.begin().await.map_err(db)?;
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
            .await
            .map_err(db)?;
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, w.warehouse_id)
            .await
            .map_err(db)?;
        let overlaps_before = egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &bound)
            .await
            .map_err(|e| Refusal::internal("overlap snapshot", e))?;
        let removed = delete_inserted_rows(&mut tx, w.warehouse_id, &w.inserted)
            .await
            .map_err(db)?;
        record(&mut tx, &w.view, &w.previous, w.previous_columns.as_deref())
            .await
            .map_err(|e| Refusal::internal("lineage", e))?;
        let rederived = if rederive && matches!(w.previous, RecordedLineage::Computed(_)) {
            matches!(
                egret_perm_compile::retag_view_in_tx(&mut tx, &w.view)
                    .await
                    .map_err(|e| Refusal::internal("re-tag", e))?,
                egret_perm_compile::ViewRetag::Changed { .. }
            )
        } else {
            false
        };
        if removed > 0 || rederived {
            bump_and_recompile(&mut tx, w.warehouse_id, &bound).await?;
            egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &overlaps_before)
                .await
                .map_err(Refusal::overlap)?;
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

    /// The view's stored metadata (`ViewMetadata` JSON), by a `GET` of the
    /// commit's own path on lakekeeper's router, with the commit's request
    /// metadata, bounded by the deadline.
    async fn load_stored(
        &self,
        loader: &axum::Router,
        path: String,
        metadata: lakekeeper::api::RequestMetadata,
        deadline: Instant,
    ) -> Result<Value, String> {
        use tower::ServiceExt as _;
        let mut get = Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .map_err(|e| format!("the stored view cannot be loaded: {e}"))?;
        get.extensions_mut().insert(metadata);
        let left = deadline.saturating_duration_since(Instant::now());
        let response = tokio::time::timeout(left, loader.clone().oneshot(get))
            .await
            .map_err(|_| "the stored view cannot be loaded before the deadline".to_string())?
            .map_err(|e| format!("the stored view cannot be loaded: {e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "the stored view cannot be loaded: the catalog answered {}",
                response.status()
            ));
        }
        let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY)
            .await
            .map_err(|e| format!("the stored view cannot be loaded: {e}"))?;
        let mut body: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("the stored view cannot be loaded: {e}"))?;
        body.get_mut("metadata")
            .map(Value::take)
            .ok_or_else(|| "the stored view cannot be loaded: no metadata".to_string())
    }

    /// Create, replace, or a commit that sets the current version.
    #[allow(clippy::too_many_arguments)]
    async fn hold_definition(
        &self,
        loader: &axum::Router,
        create: bool,
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

        // Step 0: nothing classified anywhere this view could read. No lock,
        // never a refusal; the lineage is recorded best-effort.
        if !r.classified {
            return self
                .best_effort(loader, deadline, prefix, &r, ns, name, change, req, next)
                .await;
        }
        if is_multi_level(ns) {
            return Refusal::multi_level().into_response();
        }
        // A create's definition errors get advice that fits a create.
        let unjudgeable = |e: String| {
            if create {
                Refusal::create_unjudgeable(e)
            } else {
                Refusal::unjudgeable(e)
            }
        };
        let pending = match change {
            Change::Untouched => return next.run(req).await,
            Change::Judge(Err(e)) => return unjudgeable(e).into_response(),
            Change::Judge(Ok(p)) => p,
        };
        let Some(catalog) = r.catalog else {
            return Refusal::lineage("the view's warehouse has no catalog in egret's registry")
                .into_response();
        };
        let held = match self.hold(prefix, &[(ns, name)]).await {
            Ok(h) => h,
            Err(refusal) => return refusal.into_response(),
        };
        // I-2: everything under the lock runs in its own task, so a client
        // that disconnects mid-wait cannot skip the compensation.
        let this = self.clone();
        let loader = loader.clone();
        let (ns, name) = (ns.to_string(), name.to_string());
        let warehouse_id = r.warehouse_id;
        uncancellable(async move {
            // NM2: the stored metadata is read under the view's lock. The
            // commit's own path and request metadata (the guard put it there)
            // are taken out first, since a `Body` is not `Sync`.
            let path = req.uri().path().to_string();
            let metadata = request_metadata(&req);
            let response = match this
                .definition_of(&loader, pending, path, metadata, deadline)
                .await
            {
                Ok(def) => {
                    this.judge(deadline, warehouse_id, &catalog, &ns, &name, def, req, next)
                        .await
                }
                Err(e) if create => Refusal::create_unjudgeable(e).into_response(),
                Err(e) => Refusal::unjudgeable(e).into_response(),
            };
            held.release().await;
            response
        })
        .await
    }

    /// The definition `pending` names. N3: a stored version (or schema) is
    /// read from the view's metadata, through lakekeeper's own load, within
    /// the deadline.
    async fn definition_of(
        &self,
        loader: &axum::Router,
        pending: Pending,
        path: String,
        metadata: lakekeeper::api::RequestMetadata,
        deadline: Instant,
    ) -> Result<Definition, String> {
        let stored = if pending.needs_stored_metadata() {
            Some(self.load_stored(loader, path, metadata, deadline).await?)
        } else {
            None
        };
        resolve_pending(pending, stored.as_ref())
    }

    /// The lineage EXPLAIN of `def`, bounded by [`EXPLAIN_BUDGET`] and the
    /// deadline, with no connection held.
    async fn explain(
        &self,
        def: &Definition,
        catalog: &str,
        ns: &str,
        deadline: Instant,
    ) -> Result<std::collections::BTreeSet<lineage::LineageTable>, String> {
        let budget = EXPLAIN_BUDGET.min(deadline.saturating_duration_since(Instant::now()));
        let default_catalog = def.default_catalog.as_deref().unwrap_or(catalog);
        let default_schema = def.default_schema.as_deref().unwrap_or(ns);
        tokio::time::timeout(
            budget,
            lineage::compute_lineage(
                self.runner.as_ref(),
                default_catalog,
                default_schema,
                &def.sql,
            ),
        )
        .await
        .map_err(|_| format!("the EXPLAIN did not answer within {} s", seconds(budget)))?
    }

    /// Step 0 (Task 9 fix round 1, amended ruling): on a profile with no
    /// classified column, the lineage is computed best-effort and recorded
    /// after lakekeeper answers 2xx: computed (with the columns) on success,
    /// `failed` with the reason on any failure. It NEVER refuses. What it
    /// records only lets a later classification find the view.
    ///
    /// It takes the view's lock (Task 9 fix round 2, N2), as the hold and the
    /// sweep do, so two writers cannot record out of order; the lock is a
    /// session-level advisory lock on a connection detached from the pool, so
    /// it is taken only under a permit of its own cap (final review I-1).
    /// With no permit free, or the lock busy, it skips the EXPLAIN, lets the
    /// commit through at once, and records `failed` without waiting
    /// ([`record_failed_if_free`]).
    #[allow(clippy::too_many_arguments)]
    async fn best_effort(
        &self,
        loader: &axum::Router,
        deadline: Instant,
        prefix: &str,
        r: &Resolved,
        ns: &str,
        name: &str,
        change: Change,
        req: Request,
        next: Next,
    ) -> Response {
        let Some(catalog) = r.catalog.clone().filter(|_| !is_multi_level(ns)) else {
            return next.run(req).await;
        };
        let view = view_ref(&catalog, ns, name);
        let key = lock_key(prefix, ns, name);
        // I-1: the permit before the lock connection. The permit and the
        // connection both live until this function returns.
        let lock = match self.best_effort.clone().try_acquire_owned() {
            Ok(permit) => match try_view_lock(&self.db, &key).await {
                Ok(Some(conn)) => Some((permit, conn)),
                Ok(None) | Err(_) => None,
            },
            Err(_) => None,
        };
        let Some((_permit, lock)) = lock else {
            let response = next.run(req).await;
            if response.status().is_success() {
                // Bounded (pooled connection, `RECORD_LOCK_WAIT`), so no
                // task outlives a few seconds.
                let db = self.db.clone();
                tokio::spawn(async move {
                    match record_failed_if_free(db, key, view.clone()).await {
                        Ok(true) => {}
                        Ok(false) => tracing::info!(
                            ?view,
                            "view hold: the view's lock is busy; its lineage is left to the sweep"
                        ),
                        Err(e) => tracing::warn!(
                            ?view,
                            error = %e,
                            "view hold: the failed lineage was not recorded; the sweep repairs it"
                        ),
                    }
                });
            }
            return response;
        };
        let outcome: Option<(RecordedLineage, Option<Vec<String>>)> = match change {
            Change::Untouched => None,
            Change::Judge(Err(detail)) => Some((RecordedLineage::Failed { detail }, None)),
            Change::Judge(Ok(pending)) => {
                let path = req.uri().path().to_string();
                let metadata = request_metadata(&req);
                match self
                    .definition_of(loader, pending, path, metadata, deadline)
                    .await
                {
                    Err(detail) => Some((RecordedLineage::Failed { detail }, None)),
                    Ok(def) => match self.explain(&def, &catalog, ns, deadline).await {
                        Ok(tables) => Some((RecordedLineage::Computed(tables), Some(def.columns))),
                        Err(detail) => {
                            Some((RecordedLineage::Failed { detail }, Some(def.columns)))
                        }
                    },
                }
            }
        };
        let response = next.run(req).await;
        if response.status().is_success() {
            if let Some((lineage, columns)) = outcome {
                self.record_after_commit(r.warehouse_id, &view, &lineage, columns.as_deref())
                    .await;
            }
        }
        if let Err(e) = lock.close().await {
            tracing::warn!(error = %e, "view hold: closing the view lock failed");
        }
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
        let tables = match self.explain(&def, catalog, ns, deadline).await {
            Ok(t) => t,
            Err(e) => return Refusal::lineage(e).into_response(),
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
                self.record_after_commit(
                    warehouse_id,
                    &view,
                    &RecordedLineage::Computed(tables),
                    Some(&def.columns),
                )
                .await;
            }
            return response;
        }

        // M1: with no Trino engine bound, no plugin could confirm; refuse
        // before writing anything. (`wait` checks again, after the write.)
        match self.db.pool.acquire().await {
            Ok(mut conn) => match trino_tag_versions(&mut conn, warehouse_id).await {
                Ok(pairs) if pairs.is_empty() => return Refusal::no_trino().into_response(),
                Ok(_) => {}
                Err(e) => return Refusal::internal("database", e).into_response(),
            },
            Err(e) => return Refusal::internal("database", e).into_response(),
        }

        // Step 3. The tag set is computed again inside the write's locked
        // transaction (I2); the one read above only chose step 2 or 3.
        let written = match self
            .write(
                deadline,
                warehouse_id,
                &view,
                Rows::Derived,
                &RecordedLineage::Computed(tables),
                Some(&def.columns),
            )
            .await
        {
            Ok(w) => w,
            Err(r) => return r.into_response(),
        };
        self.wait_then_commit(written, deadline, req, next).await
    }

    /// Record a lineage for a view lakekeeper has committed, under the
    /// view-tag lock, and re-tag the view from a computed one (Task 9 fix
    /// round 1, I2): a base classification that committed after the hold's
    /// first read is seen here, and one that commits later sees this lineage.
    /// When the re-tag cannot be written (it would complete an overlapping
    /// mask pair), the lineage is still recorded, so the next base write or
    /// sweep tries again and reports it. A failure leaves the lineage missing
    /// or stale, which the B3 sweep repairs; it never removes a tag.
    async fn record_after_commit(
        &self,
        warehouse_id: Uuid,
        view: &ViewRef,
        lineage: &RecordedLineage,
        columns: Option<&[String]>,
    ) {
        let retag = matches!(lineage, RecordedLineage::Computed(_));
        let mut result = self
            .try_record_after_commit(warehouse_id, view, lineage, columns, retag)
            .await;
        if retag {
            if let Err(e) = &result {
                tracing::error!(
                    ?view,
                    error = %e,
                    "view hold: the committed view could not be re-tagged; recording its lineage only"
                );
                result = self
                    .try_record_after_commit(warehouse_id, view, lineage, columns, false)
                    .await;
            }
        }
        if let Err(e) = result {
            tracing::warn!(?view, error = %e, "view hold: lineage not recorded after commit");
        }
    }

    async fn try_record_after_commit(
        &self,
        warehouse_id: Uuid,
        view: &ViewRef,
        lineage: &RecordedLineage,
        columns: Option<&[String]>,
        retag: bool,
    ) -> Result<(), String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let mut tx = self.db.pool.begin().await.map_err(|e| s(&e))?;
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
            .await
            .map_err(|e| s(&e))?;
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
            .await
            .map_err(|e| s(&e))?;
        let before = if retag {
            Some(
                egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &bound)
                    .await
                    .map_err(|e| s(&e))?,
            )
        } else {
            None
        };
        record(&mut tx, view, lineage, columns)
            .await
            .map_err(|e| s(&e))?;
        if let Some(before) = before {
            if let egret_perm_compile::ViewRetag::Changed { .. } =
                egret_perm_compile::retag_view_in_tx(&mut tx, view)
                    .await
                    .map_err(|e| s(&e))?
            {
                bump_and_recompile(&mut tx, warehouse_id, &bound)
                    .await
                    .map_err(|r| r.0)?;
                egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &before)
                    .await
                    .map_err(|e| s(&e))?;
            }
        }
        tx.commit().await.map_err(|e| s(&e))
    }

    /// What `view` carries now: its lineage, its assignment rows and its
    /// recorded columns.
    async fn state_of(&self, warehouse_id: Uuid, view: &ViewRef) -> Result<ViewState, Refusal> {
        let db = |e: sqlx::Error| Refusal::internal("database", e);
        let mut conn = self.db.pool.acquire().await.map_err(db)?;
        let lineage = lineage::recorded_lineage(&mut conn, view)
            .await
            .map_err(db)?;
        let rows = assignments_of(&mut conn, warehouse_id, view)
            .await
            .map_err(db)?;
        let columns = lineage::recorded_view_columns(&mut conn, view)
            .await
            .map_err(db)?;
        Ok(ViewState {
            lineage,
            rows,
            columns,
        })
    }

    /// Drop (spec B3): once lakekeeper has answered 2xx, delete the view's
    /// lineage and the tag rows its lineage derived, and bump when a row went.
    /// An admin's manual tag on the view stays (migration 0188). A view that
    /// carries nothing passes with no lock; otherwise the view's lock is held
    /// across the drop, so a create of the same name cannot write rows this
    /// cleanup would then remove.
    async fn hold_drop(
        &self,
        prefix: &str,
        ns: &str,
        name: &str,
        req: Request,
        next: Next,
    ) -> Response {
        let r = match self.resolve(prefix).await {
            Ok(Some(r)) => r,
            Ok(None) => return next.run(req).await,
            Err(refusal) => return refusal.into_response(),
        };
        // Nothing is recorded under a multi-level name or without a catalog.
        let Some(catalog) = r.catalog.as_deref() else {
            return next.run(req).await;
        };
        if is_multi_level(ns) {
            return next.run(req).await;
        }
        let view = view_ref(catalog, ns, name);
        let carries = match self.state_of(r.warehouse_id, &view).await {
            Ok(st) => st.lineage != RecordedLineage::Missing || st.rows.iter().any(|row| row.4),
            Err(refusal) => return refusal.into_response(),
        };
        if !carries {
            return next.run(req).await;
        }
        let held = match self.hold(prefix, &[(ns, name)]).await {
            Ok(h) => h,
            Err(refusal) => return refusal.into_response(),
        };
        // I-2: a drop lakekeeper committed is cleaned up even when the client
        // has gone.
        let this = self.clone();
        let warehouse_id = r.warehouse_id;
        uncancellable(async move {
            let response = next.run(req).await;
            if response.status().is_success() {
                this.forget_dropped(warehouse_id, &view).await;
            }
            held.release().await;
            response
        })
        .await
    }

    /// The drop's cleanup, in one transaction. A failure is logged: what it
    /// leaves behind is a lineage and derived tags of a view that no longer
    /// exists, which over-tag a later view of the same name until it is
    /// judged or swept, and never under-tag one.
    async fn forget_dropped(&self, warehouse_id: Uuid, view: &ViewRef) {
        let result = async {
            let mut tx = self.db.pool.begin().await.map_err(|e| e.to_string())?;
            egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
                .await
                .map_err(|e| e.to_string())?;
            let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
                .await
                .map_err(|e| e.to_string())?;
            let removed = sqlx::query(
                "DELETE FROM catalog_tag_assignments \
                  WHERE warehouse_id = $1 AND schema_name = $2 AND table_name = $3 \
                    AND from_lineage",
            )
            .bind(warehouse_id)
            .bind(&view.schema)
            .bind(&view.name)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
            lineage::delete_lineage(&mut tx, view)
                .await
                .map_err(|e| e.to_string())?;
            if removed > 0 {
                bump_and_recompile(&mut tx, warehouse_id, &bound)
                    .await
                    .map_err(|r| r.0)?;
            }
            tx.commit().await.map_err(|e| e.to_string())
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(?view, error = %e, "view hold: the dropped view's lineage and tags remain");
        }
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
            Ok(st) => st.lineage != RecordedLineage::Missing || !st.rows.is_empty(),
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
        // I-2: the rename's wait, commit and compensation survive a client
        // that disconnects.
        let this = self.clone();
        uncancellable(async move {
            let response = this
                .move_view(deadline, warehouse_id, &src, &dst, req, next)
                .await;
            held.release().await;
            response
        })
        .await
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
        let ViewState {
            lineage: src_lineage,
            rows: src_rows,
            columns: src_columns,
        } = match self.state_of(warehouse_id, src).await {
            Ok(s) => s,
            Err(refusal) => return refusal.into_response(),
        };
        let rows: Vec<Assignment> = src_rows
            .iter()
            .map(|(tag, _, _, column, from_lineage)| {
                (
                    *tag,
                    dst.schema.clone(),
                    dst.name.clone(),
                    column.clone(),
                    *from_lineage,
                )
            })
            .collect();
        let written = match self
            .write(
                deadline,
                warehouse_id,
                dst,
                Rows::Fixed(&rows),
                &src_lineage,
                src_columns.as_deref(),
            )
            .await
        {
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
            egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
                .await
                .map_err(|e| e.to_string())?;
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

/// Delete those of `rows` (the rows a hold inserted) in `warehouse_id` that
/// still carry the provenance they were inserted with; how many went.
///
/// - A derived row an admin has since assigned by hand is now manual, so it
///   stays (I1(a)).
/// - A manual row the hold inserted itself (a rename copies the source's
///   manual tags) cannot have been flipped by an admin, since `assign_catalog`
///   leaves a manual row as it is, so it is still the hold's own and goes
///   (fix round 2, N1).
async fn delete_inserted_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    warehouse_id: Uuid,
    rows: &[Assignment],
) -> Result<u64, sqlx::Error> {
    let mut removed = 0;
    for (tag, schema, table, column, from_lineage) in rows {
        removed += sqlx::query(
            "DELETE FROM catalog_tag_assignments \
              WHERE tag_id = $1 AND warehouse_id = $2 AND schema_name = $3 \
                AND table_name = $4 AND column_name = $5 AND from_lineage = $6",
        )
        .bind(tag)
        .bind(warehouse_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .bind(from_lineage)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }
    Ok(removed)
}

/// Delete exactly `rows` from `warehouse_id`.
async fn delete_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    warehouse_id: Uuid,
    rows: &[Assignment],
) -> Result<(), sqlx::Error> {
    for (tag, schema, table, column, _) in rows {
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

/// `pg_try_advisory_lock` on `key`, on a connection detached from the pool:
/// the connection holding the lock, or `None` when another writer has it.
async fn try_view_lock(db: &Db, key: &str) -> Result<Option<sqlx::PgConnection>, sqlx::Error> {
    let mut conn = db.pool.acquire().await?.detach();
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(key)
        .fetch_one(&mut conn)
        .await?;
    if got {
        Ok(Some(conn))
    } else {
        let _ = conn.close().await;
        Ok(None)
    }
}

/// How long [`record_failed_if_free`] waits for the view-tag lock.
const RECORD_LOCK_WAIT: Duration = EXPLAIN_BUDGET;

/// A best-effort commit that skipped its EXPLAIN (no permit, or the view's
/// lock busy) records the view's lineage `failed`, so the sweep a later
/// classification triggers recomputes it. Its own EXPLAIN was skipped, so a
/// lineage another writer records may describe a version older than this
/// commit: `failed` is the honest state.
///
/// Final review I-1 (and Task 9 N8): one pooled transaction, never a
/// detached connection or a wait for the view's lock. It tries the view's
/// lock as a transaction-level advisory lock (the same key space as the
/// session-level one the hold and the sweep take); busy answers `Ok(false)`
/// and leaves the lineage to the sweep, at most 15 minutes. The view-tag lock
/// wait is bounded by `lock_timeout` ([`RECORD_LOCK_WAIT`]) set on this same
/// transaction.
async fn record_failed_if_free(db: Db, key: String, view: ViewRef) -> Result<bool, sqlx::Error> {
    let detail = "another change to this view was being recorded at the same time; \
                  the next sweep recomputes its lineage";
    let mut tx = db.pool.begin().await?;
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(format!("{}ms", RECORD_LOCK_WAIT.as_millis()))
        .execute(&mut *tx)
        .await?;
    let free: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&key)
            .fetch_one(&mut *tx)
            .await?;
    if !free {
        tx.rollback().await?;
        return Ok(false);
    }
    egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx).await?;
    lineage::record_lineage_failure(&mut tx, &view, detail).await?;
    tx.commit().await?;
    Ok(true)
}

/// Final review I-2: run a held request's work after its lock in a task of
/// its own. When the client disconnects, hyper drops the handler future, but
/// dropping a `JoinHandle` does not cancel its task, so the wait, the commit
/// and any compensation still run, under the lock and the permit the task
/// owns.
async fn uncancellable<F>(work: F) -> Response
where
    F: std::future::Future<Output = Response> + Send + 'static,
{
    match tokio::spawn(work).await {
        Ok(response) => response,
        Err(e) => Refusal::internal("hold task", e).into_response(),
    }
}

/// The commit's request metadata (the guard put it there), or an internal one.
fn request_metadata(req: &Request) -> lakekeeper::api::RequestMetadata {
    req.extensions()
        .get::<lakekeeper::api::RequestMetadata>()
        .cloned()
        .unwrap_or_else(
            || lakekeeper::api::RequestMetadata::new_lakekeeper_internal(Uuid::now_v7()),
        )
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
///
/// The state's router is lakekeeper's own, with no hold in front: the hold
/// reads a stored view through it (N3).
pub async fn hold_view_commits(
    State((hold, loader)): State<(Arc<ViewHold>, axum::Router)>,
    req: Request,
    next: Next,
) -> Response {
    let Some((prefix, target)) = classify(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };
    // A drop carries no body.
    if let Target::Drop { ns, view } = &target {
        return hold.hold_drop(&prefix, ns, view, req, next).await;
    }
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
                hold.hold_definition(
                    &loader, false, deadline, &prefix, &ns, &view, change, req, next,
                )
                .await
            }
        },
        Target::Create { ns } => match create_definition(&json) {
            Ok((name, def)) => {
                hold.hold_definition(
                    &loader,
                    true,
                    deadline,
                    &prefix,
                    &ns,
                    &name,
                    Change::Judge(def),
                    req,
                    next,
                )
                .await
            }
            // No name: lakekeeper refuses it, and nothing can be recorded.
            Err(_) => next.run(req).await,
        },
        Target::Drop { .. } => unreachable!("a drop is handled before its body is read"),
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

pub(crate) fn lock_key(prefix: &str, ns: &str, name: &str) -> String {
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
        // A view drop is classified (spec B3); a table drop is not.
        assert_eq!(
            classify(&Method::DELETE, "/wh1/namespaces/s/views/v1"),
            Some((
                "wh1".into(),
                Target::Drop {
                    ns: "s".into(),
                    view: "v1".into()
                }
            ))
        );
        assert_eq!(
            classify(&Method::DELETE, "/wh1/namespaces/s/tables/t1"),
            None
        );
        assert_eq!(classify(&Method::DELETE, "/wh1/namespaces/s/views"), None);
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
        let Some(Ok(def)) = judged(&body) else {
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
        let Some(Ok(def)) = judged(&body) else {
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
            assert!(judged(&body).is_some_and(|d| d.is_err()), "{body}");
        }
    }

    /// The definition a commit body resolves to with no stored metadata, or
    /// `Err` when it cannot; `None` when it is not judged at all.
    fn judged(body: &Value) -> Option<Result<Definition, String>> {
        match commit_change(body) {
            Change::Untouched => None,
            Change::Judge(p) => Some(p.and_then(|p| resolve_pending(p, None))),
        }
    }

    fn add(sql: &str, schema_id: i64) -> Value {
        json!({"action": "add-view-version", "view-version": {"schema-id": schema_id,
            "representations": [{"type": "sql", "sql": sql, "dialect": "trino"}]}})
    }

    fn add_schema(id: i64, cols: &[&str]) -> Value {
        let fields: Vec<Value> = cols.iter().map(|c| json!({"name": c})).collect();
        json!({"action": "add-schema", "schema": {"schema-id": id, "fields": fields}})
    }

    fn set(id: i64) -> Value {
        json!({"action": "set-current-view-version", "view-version-id": id})
    }

    /// N1 and N2: the updates replay in order, with the builder's "last
    /// added" bookkeeping for both `-1`s.
    #[test]
    fn the_updates_replay_in_order_like_the_view_builder() {
        let s1 = add_schema(1, &["id", "ssn"]);
        let s2 = add_schema(2, &["x", "y"]);
        let sql = |body: Value| judged(&body).unwrap().unwrap().sql;
        let cols = |body: Value| judged(&body).unwrap().unwrap().columns;

        // [add A, set -1, add B]: A is current.
        assert_eq!(
            sql(json!({"updates": [s1, add("A", -1), set(-1), add("B", -1)]})),
            "A"
        );
        // [add A, add B, set -1]: B is current.
        assert_eq!(
            sql(json!({"updates": [s1, add("A", -1), add("B", -1), set(-1)]})),
            "B"
        );
        // [add-schema S1, add A(-1), add-schema S2, set -1]: A with S1.
        assert_eq!(
            cols(json!({"updates": [s1, add("A", -1), s2, set(-1)]})),
            vec!["id", "ssn"]
        );
        // [S1, add A, set -1, S2, add B(-1)]: A, with S1, stays current.
        let body = json!({"updates": [s1, add("A", -1), set(-1), s2, add("B", -1)]});
        assert_eq!(sql(body.clone()), "A");
        assert_eq!(cols(body), vec!["id", "ssn"]);
        // [S1, add A, S2, add B(-1), set -1]: B, with S2.
        let body = json!({"updates": [s1, add("A", -1), s2, add("B", -1), set(-1)]});
        assert_eq!(sql(body.clone()), "B");
        assert_eq!(cols(body), vec!["x", "y"]);
        // The last set-current wins: [add A, set -1, add B, set -1] is B.
        assert_eq!(
            sql(json!({"updates": [s1, add("A", -1), set(-1), add("B", -1), set(-1)]})),
            "B"
        );
        // An add with no set-current leaves the current version: untouched.
        assert!(judged(&json!({"updates": [s1, add("A", -1)]})).is_none());
        // `-1` before any version, and a `-1` schema before any schema, fail.
        assert!(judged(&json!({"updates": [set(-1), s1, add("A", -1)]}))
            .unwrap()
            .is_err());
        assert!(judged(&json!({"updates": [add("A", -1), s1, set(-1)]}))
            .unwrap()
            .is_err());
        // An explicit schema id alongside an add-schema is ambiguous (the
        // builder may renumber the added schema).
        assert!(judged(&json!({"updates": [s1, add("A", 1), set(-1)]}))
            .unwrap()
            .is_err());
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
            "schema-id": 4, "representations": [{"type": "sql", "sql": "x", "dialect": "trino"}]}},
            {"action": "set-current-view-version", "view-version-id": -1}]});
        assert!(judged(&body).is_some_and(|d| d.is_err()));
    }

    #[test]
    fn a_rename_keeps_every_namespace_level() {
        let body = json!({"source": {"namespace": ["a", "b"], "name": "v"},
                          "destination": {"namespace": ["s"], "name": "v"}});
        let (src, dst) = rename_of(&body).unwrap();
        assert!(is_multi_level(&src.0));
        assert_eq!(dst, ("s".to_string(), "v".to_string()));
    }

    /// Task 12 fix round 1 (review I-1): egret's own chart must not cut a hold.
    /// The Ingress object that routes `/catalog` must read for longer than the
    /// hold limit, or nginx answers 504 while egret goes on to commit or refuse
    /// the view. Reads the template, so a chart edit cannot drift silently.
    #[test]
    fn the_chart_reads_catalog_requests_for_longer_than_a_hold() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/helm/egret/templates/ingress.yaml"
        );
        let chart = std::fs::read_to_string(path).expect("ingress.yaml missing");
        let objects: Vec<&str> = chart
            .split("\n---")
            .filter(|o| o.contains("- path: /catalog\n"))
            .collect();
        assert_eq!(objects.len(), 1, "exactly one Ingress routes /catalog");
        let key = "nginx.ingress.kubernetes.io/proxy-read-timeout: \"";
        let at = objects[0]
            .find(key)
            .expect("the /catalog Ingress pins its read timeout")
            + key.len();
        let secs: u64 = objects[0][at..]
            .split('"')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("a whole number of seconds");
        assert!(
            secs > default_hold_limit().as_secs(),
            "/catalog is read for {secs} s, not longer than the {} s hold limit",
            default_hold_limit().as_secs()
        );
    }

    #[test]
    fn the_refusal_messages_name_their_reason_and_what_to_do() {
        assert_eq!(seconds(default_hold_limit()), "90");
        assert_eq!(seconds(Duration::from_millis(1500)), "1.5");
        assert!(Refusal::lineage("boom")
            .0
            .starts_with("lineage could not be computed: boom; "));
        for r in [
            Refusal::lineage("x"),
            Refusal::internal("db", "x"),
            Refusal::multi_level(),
            Refusal::unjudgeable("x"),
            Refusal::create_unjudgeable("x"),
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
                    "DROP and CREATE",
                    "create the view from Trino",
                    "attach a Trino"
                ]
                .iter()
                .any(|w| r.0.contains(w)),
                "says what to do: {}",
                r.0
            );
        }
    }

    /// Final review I-1 (Task 9 N8): the failed-lineage record of a busy
    /// best-effort commit waits for the view-tag lock no longer than its
    /// bound. Before the fix its transaction had no `lock_timeout` (that was
    /// set on the other, detached connection), so it waited as long as the
    /// view-tag lock stayed taken.
    #[tokio::test]
    async fn the_failed_record_waits_for_the_view_tag_lock_no_longer_than_its_bound() {
        let Some(dsn) = egret_test_db::url().await else {
            return;
        };
        let db = Db::connect(&dsn, 4).await.expect("connect");
        let view = view_ref(
            "vh_bound_cat",
            "s",
            &format!("v_{}", Uuid::new_v4().simple()),
        );
        let key = lock_key("vh-bound", "s", &view.name);
        let mut blocker = db.pool.begin().await.expect("blocker");
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut blocker)
            .await
            .expect("take the view-tag lock");

        let start = Instant::now();
        let outcome = tokio::time::timeout(
            RECORD_LOCK_WAIT + Duration::from_secs(5),
            record_failed_if_free(db.clone(), key, view.clone()),
        )
        .await;
        let took = start.elapsed();
        blocker.rollback().await.expect("release");

        let outcome = outcome.expect("the record waited past its bound");
        assert!(outcome.is_err(), "the lock wait timed out: {outcome:?}");
        assert!(took < RECORD_LOCK_WAIT + Duration::from_secs(2), "{took:?}");
        let mut conn = db.pool.acquire().await.expect("conn");
        assert_eq!(
            lineage::recorded_lineage(&mut conn, &view)
                .await
                .expect("read"),
            RecordedLineage::Missing,
        );
    }
}
