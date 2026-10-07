//! `workflow.kv_entries` (migration 0175) and the objects behind it: the
//! reads (W9c spec §4.1, §4.4-§4.6) and the writes (§4.3, §4.4, §8.5, §8.6;
//! see "Writes" below).
//!
//! Every statement is scoped `WHERE workspace_id = $1 AND namespace = …`,
//! both taken from the [`Grant`]: the workspace is the caller's, never an
//! argument's. Every function checks its grant with
//! `require(grant, Resource::Kv, access)`, so a namespace-file grant cannot
//! reach the KV store.
//!
//! - A key's state is its HIGHEST revision's row. The latest row is taken
//!   first and judged live after it -- filtering `deleted` or expiry first
//!   would resurrect an older revision of a deleted or expired key.
//! - **Expired means missing** (§4.5): `expired` is computed in SQL, in the
//!   statement that reads the row, as `expires_at <= now()` -- the
//!   database's clock (its transaction's `now()`), never this process's.
//! - **Reads never write**: nothing here inserts, updates or deletes.
//! - A key prefix is matched with `left(key, length($n)) = $n`, never `LIKE`
//!   (`_` and `.` are legal in keys).
//! - A listing is in the order the latest revisions were written (the latest
//!   row's `id` ascending), as Kestra lists them (w9c `get_keys._order`).
//!
//! # The index
//!
//! The unique constraint `workflow_kv_entries_revision_key` is
//! `(workspace_id, namespace, key, revision)`: it leads with
//! `(workspace_id, namespace, key)`, so it IS spec §4.1's latest-row index
//! (`ORDER BY revision DESC LIMIT 1` scans it backwards).
//!
//! # Connections
//!
//! Every read takes a `&mut PgConnection` it does not keep; [`read_value`]
//! takes none -- call it after the row was read and the connection let go,
//! so no connection is held across the object's download.
//!
//! # Writes
//!
//! Every writer of a key takes the key's advisory lock,
//! `hashtextextended(kv_lock_text(grant, key), 0)` (the text built here and
//! bound as a value, the hash computed by SQL): [`set`], [`put_merge`],
//! [`delete`] and [`delete_many`] as a TRANSACTION lock, [`purge_key`] and
//! the expiry sweep (`super::sweep`) as a SESSION lock, which outlives the
//! purge's commit until its objects are deleted. The two kinds conflict, so
//! every writer of one key is serialised. A grant with a
//! [`Grant::lock_wait`] (the API's) waits at most that long, then answers
//! `Conflict` "the key is busy"; a flow's grant waits as long as it takes.
//!
//! What each writer holds, and across which I/O (the hold audit, spec §4.3
//! and the user's ruling of 2026-10-07 on server-paced calls under a lock):
//! - [`set`]: one statement (`staging::record`); then the staging PUT,
//!   holding NO connection and NO lock; then one transaction holding the
//!   key's lock across one server-side `copy` and one `head`, each of at
//!   most `max_read_bytes`.
//! - [`put_merge`]: one transaction holding the key's lock across one GET of
//!   the latest value (at most `max_read_bytes`), the merge and
//!   `check_readable` (CPU, on the blocking pool), and one PUT of the merged
//!   value (at most `max_read_bytes`).
//! - [`delete`], [`delete_many`]: the lock across SQL only.
//! - [`purge_key`]: the session lock across the re-read, the delete and the
//!   object deletes of the purged revisions.
//!
//! None of them reads a caller's body under a lock or a connection: a value
//! reaches them in memory, at most `max_read_bytes`.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use super::key::KvKey;
use super::kv_lock_text;
use super::merge;
use super::value::{
    check_readable, decode, max_ttl, KvData, KvType, KvValue, Unstorable, MAX_TTL_DAYS,
};
use crate::storage::lock::{
    is_lock_timeout, is_lock_timeout_error, lock_timeout_value, LockedSession,
};
use crate::storage::nsfiles::access::{require, Access, Grant, Resource};
use crate::storage::nsfiles::staging;
use crate::storage::nsfiles::store::{writer, Want};
use crate::storage::nsfiles::{CreatedBy, NsError};
use crate::storage::FileStore;

#[derive(Clone, Debug, PartialEq)]
pub struct KvEntry {
    pub id: i64,
    pub workspace: Uuid,
    pub namespace: String,
    pub key: KvKey,
    pub revision: u32,
    pub value_type: KvType,
    pub size: u64,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
    /// `expires_at <= now()` by THE DATABASE's clock, computed in the same
    /// statement (§4.5).
    pub expired: bool,
    /// The task run that wrote it, and the one that deleted it (re-claims, §11).
    pub created_by_task_run: Option<Uuid>,
    pub deleted_by_task_run: Option<Uuid>,
}

impl KvEntry {
    /// Neither soft-deleted nor expired.
    pub fn live(&self) -> bool {
        !self.deleted && !self.expired
    }
}

/// The columns of one [`KvEntry`] read from the table, in [`Row`]'s order,
/// `expired` computed by the database in the reading statement.
macro_rules! cols {
    () => {
        "id, workspace_id, namespace, key, revision, value_type, size, description, \
         expires_at, deleted, created_at, \
         (expires_at IS NOT NULL AND expires_at <= now()) AS expired, \
         created_by_task_run, deleted_by_task_run"
    };
}

/// [`cols`] for a WRITER, under the key's lock (Task 3 review I4): `expired`
/// is judged at `clock_timestamp()`, the moment of the statement, not the
/// transaction's start (`now()`), which is before the lock wait.
macro_rules! wcols {
    () => {
        "id, workspace_id, namespace, key, revision, value_type, size, description, \
         expires_at, deleted, created_at, \
         (expires_at IS NOT NULL AND expires_at <= clock_timestamp()) AS expired, \
         created_by_task_run, deleted_by_task_run"
    };
}

/// [`cols`] as an outer query names them over a subquery that selected [`cols`].
macro_rules! out_cols {
    () => {
        "id, workspace_id, namespace, key, revision, value_type, size, description, \
         expires_at, deleted, created_at, expired, created_by_task_run, deleted_by_task_run"
    };
}

type Row = (
    i64,
    Uuid,
    String,
    String,
    i32,
    String,
    i64,
    Option<String>,
    Option<DateTime<Utc>>,
    bool,
    DateTime<Utc>,
    bool,
    Option<Uuid>,
    Option<Uuid>,
);

/// egret's bug, not the author's and not an outage: a 500.
fn internal(message: String) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::anyhow!(message)))
}

/// A KV function handed a grant of the other resource, or a Read grant for a
/// write: egret's own bug. NOT `NsError::Db`, which a handler treats as an
/// outage and re-claims forever; an `Author` error fails the task, loudly and
/// once (W9b's `wrong_grant`).
fn wrong_grant(e: egret_core::Error) -> NsError {
    NsError::Author(format!("internal error (an egret bug, not retried): {e}"))
}

fn need(grant: &Grant, access: Access) -> Result<(), NsError> {
    require(grant, Resource::Kv, access).map_err(wrong_grant)
}

fn db_err(e: sqlx::Error) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::Error::new(e)))
}

fn entry(row: Row) -> Result<KvEntry, NsError> {
    let (
        id,
        workspace,
        namespace,
        key,
        revision,
        value_type,
        size,
        description,
        expires_at,
        deleted,
        created_at,
        expired,
        created_by_task_run,
        deleted_by_task_run,
    ) = row;
    let bad = |why: String| {
        internal(format!(
            "workflow.kv_entries row {id} holds what egret would not have written: {why}"
        ))
    };
    Ok(KvEntry {
        id,
        workspace,
        namespace,
        key: KvKey::parse(&key).map_err(|e| bad(format!("key {key:?}: {e}")))?,
        revision: u32::try_from(revision).map_err(|_| bad(format!("revision {revision}")))?,
        value_type: KvType::parse(&value_type)
            .ok_or_else(|| bad(format!("value_type {value_type:?}")))?,
        size: u64::try_from(size).map_err(|_| bad(format!("size {size}")))?,
        description,
        expires_at,
        deleted,
        created_at,
        expired,
        created_by_task_run,
        deleted_by_task_run,
    })
}

fn entries(rows: Vec<Row>) -> Result<Vec<KvEntry>, NsError> {
    rows.into_iter().map(entry).collect()
}

/// `LIMIT cap + 1`, as Postgres takes it.
fn limit(cap: u64) -> i64 {
    i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX)
}

/// `rows` holds at most `cap + 1` rows; the `+1` is the refusal.
fn capped(rows: Vec<KvEntry>, cap: u64, what: &str) -> Result<Vec<KvEntry>, NsError> {
    if rows.len() as u64 > cap {
        return Err(NsError::TooLarge(format!(
            "{what} holds more than {cap} keys, the most one answer may hold \
             (workflow.storage.max_namespace_files); {} were seen",
            rows.len()
        )));
    }
    Ok(rows)
}

/// The same message for a key never written, soft-deleted or expired.
fn not_found(grant: &Grant, key: &KvKey) -> NsError {
    NsError::NotFound(format!(
        "key '{key}' does not exist in namespace '{}'",
        grant.namespace()
    ))
}

fn revision_not_found(grant: &Grant, key: &KvKey, n: u32) -> NsError {
    NsError::NotFound(format!(
        "revision {n} of key '{key}' does not exist in namespace '{}'",
        grant.namespace()
    ))
}

/// Latest: the highest revision, which must be live (deleted or expired ->
/// NotFound, the same message as a key never written). Revision(n): that
/// row, served when soft-deleted, NotFound when expired or absent (§4.4,
/// §4.5). The message names the key and namespace only.
pub async fn resolve(
    conn: &mut PgConnection,
    grant: &Grant,
    key: &KvKey,
    want: Want,
) -> Result<KvEntry, NsError> {
    need(grant, Access::Read)?;
    let row: Option<Row> = match want {
        Want::Latest => sqlx::query_as(concat!(
            "SELECT ",
            cols!(),
            " FROM workflow.kv_entries \
              WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
              ORDER BY revision DESC LIMIT 1"
        ))
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(key.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?,
        Want::Revision(n) => {
            let Ok(n) = i32::try_from(n) else {
                return Err(revision_not_found(grant, key, n));
            };
            sqlx::query_as(concat!(
                "SELECT ",
                cols!(),
                " FROM workflow.kv_entries \
                  WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND revision = $4"
            ))
            .bind(grant.workspace())
            .bind(grant.namespace())
            .bind(key.as_str())
            .bind(n)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?
        }
    };
    let missing = || match want {
        Want::Latest => not_found(grant, key),
        Want::Revision(n) => revision_not_found(grant, key, n),
    };
    let Some(row) = row else {
        return Err(missing());
    };
    let e = entry(row)?;
    // After taking the latest row, never before (this module's doc).
    let served = match want {
        Want::Latest => e.live(),
        Want::Revision(_) => !e.expired,
    };
    if !served {
        return Err(missing());
    }
    Ok(e)
}

/// The grants of a multi-namespace read: all of one workspace, all Kv.
fn one_workspace(grants: &[Grant]) -> Result<Option<Uuid>, NsError> {
    let Some(first) = grants.first() else {
        return Ok(None);
    };
    for g in grants {
        need(g, Access::Read)?;
        if g.workspace() != first.workspace() {
            return Err(wrong_grant(egret_core::Error::Internal(anyhow::anyhow!(
                "egret read the KV store of two workspaces in one walk"
            ))));
        }
    }
    Ok(Some(first.workspace()))
}

/// The inheritance walk (§4.6): `grants` deepest first, all of one
/// workspace, all Read on Kv. ONE statement reads every level's latest row;
/// answers the first LIVE one, or None. An expired or soft-deleted level is
/// missing there, and the walk goes on.
pub async fn resolve_first_live(
    conn: &mut PgConnection,
    grants: &[Grant],
    key: &KvKey,
) -> Result<Option<KvEntry>, NsError> {
    let Some(workspace) = one_workspace(grants)? else {
        return Ok(None);
    };
    let namespaces: Vec<&str> = grants.iter().map(Grant::namespace).collect();
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT DISTINCT ON (namespace) ",
        cols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = ANY($2) AND key = $3 \
          ORDER BY namespace, revision DESC"
    ))
    .bind(workspace)
    .bind(&namespaces)
    .bind(key.as_str())
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let latest = entries(rows)?;
    // Deepest first, in the grants' order; each level judged on its latest row.
    Ok(namespaces.iter().find_map(|ns| {
        latest
            .iter()
            .find(|e| e.namespace == *ns)
            .filter(|e| e.live())
            .cloned()
    }))
}

/// `ns` and its parents, deepest first, cut at dots: "a.b.c" -> ["a.b.c",
/// "a.b", "a"].
pub fn ancestors(ns: &str) -> Vec<&str> {
    let mut out = vec![ns];
    let mut rest = ns;
    while let Some((parent, _)) = rest.rsplit_once('.') {
        out.push(parent);
        rest = parent;
    }
    out
}

/// Live latest rows of the namespace, in the order w9c `get_keys` measured:
/// the order the latest revisions were written (the latest row's id
/// ascending, NOT byte order), whose key starts with `prefix`. At most
/// cap + 1 are fetched; the +1 is TooLarge naming
/// workflow.storage.max_namespace_files and the count seen. The live filter
/// runs BEFORE the limit, so the cap counts live keys.
pub async fn list_live(
    conn: &mut PgConnection,
    grant: &Grant,
    prefix: Option<&str>,
    cap: u64,
) -> Result<Vec<KvEntry>, NsError> {
    need(grant, Access::Read)?;
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        out_cols!(),
        " FROM (SELECT DISTINCT ON (key) ",
        cols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 \
            AND ($3::text IS NULL OR left(key, length($3)) = $3) \
          ORDER BY key, revision DESC) latest \
          WHERE NOT deleted AND NOT expired \
          ORDER BY id LIMIT $4"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(prefix)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let what = format!("namespace '{}'", grant.namespace());
    capped(entries(rows)?, cap, &what)
}

/// The API's inheritance listing of `namespace`: `grants` are its
/// ANCESTORS (not the namespace itself), deepest first; the live keys of
/// each, the first occurrence of a key winning (KVController.java:73-100).
/// Bounded by `cap` over the union (before a shadowed key is dropped); the
/// refusal names `namespace`, the one the caller asked about.
pub async fn list_inherited(
    conn: &mut PgConnection,
    namespace: &str,
    grants: &[Grant],
    cap: u64,
) -> Result<Vec<KvEntry>, NsError> {
    let Some(workspace) = one_workspace(grants)? else {
        return Ok(vec![]);
    };
    let namespaces: Vec<&str> = grants.iter().map(Grant::namespace).collect();
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        out_cols!(),
        " FROM (SELECT DISTINCT ON (namespace, key) ",
        cols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = ANY($2) \
          ORDER BY namespace, key, revision DESC) latest \
          WHERE NOT deleted AND NOT expired \
          ORDER BY array_position($2, namespace), id LIMIT $3"
    ))
    .bind(workspace)
    .bind(&namespaces)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let what = format!("the parents of namespace '{namespace}'");
    let all = capped(entries(rows)?, cap, &what)?;
    let mut seen = std::collections::HashSet::new();
    Ok(all
        .into_iter()
        .filter(|e| seen.insert(e.key.clone()))
        .collect())
}

/// Every row of the key, ascending, deleted and expired flagged. No row:
/// NotFound.
pub async fn revisions(
    conn: &mut PgConnection,
    grant: &Grant,
    key: &KvKey,
) -> Result<Vec<KvEntry>, NsError> {
    need(grant, Access::Read)?;
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
          ORDER BY revision"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(key.as_str())
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    if rows.is_empty() {
        return Err(not_found(grant, key));
    }
    entries(rows)
}

/// Every row of the namespace by (key in byte order, revision), strictly
/// after `after`; at most cap + 1 (Task 3's purge pages with it).
pub async fn rows_page(
    conn: &mut PgConnection,
    grant: &Grant,
    after: Option<(&KvKey, u32)>,
    cap: u64,
) -> Result<Vec<KvEntry>, NsError> {
    need(grant, Access::Read)?;
    let (after_key, after_rev) = match after {
        Some((k, r)) => (Some(k.as_str()), i32::try_from(r).unwrap_or(i32::MAX)),
        None => (None, 0),
    };
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        cols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 \
            AND ($3::text IS NULL \
                 OR (key COLLATE \"C\", revision) > ($3::text COLLATE \"C\", $4)) \
          ORDER BY key COLLATE \"C\", revision LIMIT $5"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(after_key)
    .bind(after_rev)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    entries(rows)
}

/// The namespaces of `workspace` holding at least one row, in byte order
/// (PurgeKV's candidates, Task 5).
pub async fn namespaces_with_kv(
    conn: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<String>, NsError> {
    sqlx::query_scalar(
        "SELECT DISTINCT namespace COLLATE \"C\" FROM workflow.kv_entries \
         WHERE workspace_id = $1 ORDER BY 1",
    )
    .bind(workspace)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// The entry's object, decoded into the merge-ready [`KvData`]
/// (`KvData::to_json` is what a reader sees). size > max_read_bytes:
/// TooLarge naming the cap (no download). A missing object: Author naming
/// the key and the revision (no fallback, §4.5). Any other store error:
/// Store. Takes NO connection: call it after the row was read and the
/// connection let go. The decode runs on the blocking pool, never on the
/// async worker (review I1): a value of `max_read_bytes` takes the Ion
/// reader hundreds of milliseconds.
pub async fn read_value(fs: &FileStore, grant: &Grant, e: &KvEntry) -> Result<KvData, NsError> {
    need(grant, Access::Read)?;
    // An entry another grant resolved names another namespace's object: egret's
    // bug, refused before the store is asked.
    if e.workspace != grant.workspace() || e.namespace != grant.namespace() {
        return Err(wrong_grant(egret_core::Error::Internal(anyhow::anyhow!(
            "egret read key '{}' of namespace '{}' holding a grant for namespace '{}' (or \
             another workspace)",
            e.key,
            e.namespace,
            grant.namespace()
        ))));
    }
    let what = format!(
        "revision {} of key '{}' in namespace '{}'",
        e.revision,
        e.key,
        grant.namespace()
    );
    let cap = fs.caps.max_read_bytes;
    let too_large = |size: u64| {
        NsError::TooLarge(format!(
            "{what} is {size} bytes, more than workflow.storage.max_read_bytes ({cap} bytes) \
             lets egret read"
        ))
    };
    if e.size > cap {
        return Err(too_large(e.size));
    }
    let object = fs.namespace_kv_uri(grant, &e.key, e.revision);
    let store_failed = |err: &dyn std::fmt::Display| {
        tracing::error!(value = %what, key = %object, error = %err, "internal storage failed to read a KV value");
        NsError::Store(format!(
            "key '{}' in namespace '{}': internal storage is unavailable; retry later",
            e.key,
            grant.namespace()
        ))
    };
    let reader = match fs.store.get_stream(&object).await {
        Ok(r) => r,
        Err(egret_storage::StorageError::NotFound(_)) => {
            tracing::warn!(value = %what, "a KV row has no object");
            return Err(NsError::Author(format!(
                "{what} is recorded but its value is gone from internal storage"
            )));
        }
        Err(err) => return Err(store_failed(&err)),
    };
    let mut bytes = Vec::new();
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|err| store_failed(&err))?;
    if bytes.len() as u64 > cap {
        return Err(too_large(bytes.len() as u64));
    }
    if bytes.len() as u64 != e.size {
        // Review M3: the row was read, then its key's revisions were purged
        // (or swept) and the number reused, so the object is another
        // value's. Never decoded under this row's type: a retry reads the
        // new row.
        tracing::warn!(value = %what, read = bytes.len(), recorded = e.size, "a KV object is not the size its row records");
        return Err(NsError::Store(format!(
            "key '{}' in namespace '{}' changed while it was read; retry later",
            e.key,
            grant.namespace()
        )));
    }
    let kind = e.value_type;
    let decoded = tokio::task::spawn_blocking(move || decode(kind, &bytes, cap))
        .await
        .map_err(|join| {
            if join.is_cancelled() {
                // The runtime is shutting down: nothing was read; retry.
                NsError::Store(format!(
                    "key '{}' in namespace '{}': the read was cancelled; retry later",
                    e.key,
                    grant.namespace()
                ))
            } else {
                wrong_grant(egret_core::Error::Internal(anyhow::anyhow!(
                    "decoding {what} panicked: {join}"
                )))
            }
        })?;
    decoded.map_err(|why| NsError::Author(format!("{what}: {why}")))
}

// ---------------------------------------------------------------------------
// Writes (§4.3, §4.4, §8.5, §8.6)
// ---------------------------------------------------------------------------

/// THE mapping of a value the KV store refuses (review P2): the variant,
/// never the message. `TooLarge` (a cap, the dense-value limit, the digit
/// limit) is `NsError::TooLarge`, a 413 on the API; `Invalid` is
/// `NsError::Author`, a 400. Tasks 3, 5 and 6 all map through it.
pub fn unstorable(e: Unstorable) -> NsError {
    match e {
        Unstorable::TooLarge(m) => NsError::TooLarge(m),
        Unstorable::Invalid(m) => NsError::Author(m),
    }
}

/// A value to [`set`].
#[derive(Clone, Debug, PartialEq)]
pub struct NewValue {
    pub value: KvValue,
    pub description: Option<String>,
    /// `expires_at = now() + ttl` at the commit, by the database's clock;
    /// `None` never expires. Must be positive.
    pub ttl: Option<chrono::TimeDelta>,
    /// `false`: a LIVE latest revision is a `Conflict` (§4.3).
    pub overwrite: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum KvWritten {
    Written(KvEntry),
    /// A CreatedBy::TaskRun write whose task run already wrote the latest
    /// revision (a re-claim, judged under the key's lock): nothing is
    /// written (§11).
    Already(KvEntry),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deleted {
    Deleted,
    Missing,
    Already,
}

/// The most keys one [`delete_many`] takes (the user's ruling, 2026-10-07):
/// one advisory lock per key in one transaction, and Postgres's shared lock
/// table is about 6 400 slots on a stock server.
pub const MAX_BULK_DELETE_KEYS: usize = 1_000;

/// How many [`delete_many`] calls of one process hold their locks at once
/// (review M2, spec §15).
pub const BULK_DELETES_AT_ONCE: usize = 2;

static BULK_DELETES: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(BULK_DELETES_AT_ONCE);

/// One row of [`purge_key`]'s re-read, in revision order. `superseded_at`:
/// when this revision stopped being the latest, i.e. the created_at of the
/// key's next revision; None for the latest row.
#[derive(Clone, Debug, PartialEq)]
pub struct PurgeRow {
    pub entry: KvEntry,
    pub superseded_at: Option<DateTime<Utc>>,
}

/// Runs `work` on a detached task and awaits it, as W9b's `detached`: a
/// dropped caller's future no longer stops a write part-way. Its own
/// mapping (Task 3 review M5; W9b's helper and its `Db` mapping are left as
/// they are): a panic inside is egret's bug, an `Author` error that fails
/// the task once instead of a `Db` outage re-claimed forever; a cancelled
/// task (the runtime shutting down) is `Store`, retried.
async fn detached<T, F>(what: &'static str, work: F) -> Result<T, NsError>
where
    F: std::future::Future<Output = Result<T, NsError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(work).await {
        Ok(result) => result,
        Err(e) if e.is_cancelled() => Err(NsError::Store(format!(
            "a KV {what} was cancelled; retry later"
        ))),
        Err(e) => Err(wrong_grant(egret_core::Error::Internal(anyhow::anyhow!(
            "a KV {what} stopped: {e}"
        )))),
    }
}

/// A bounded wait for a key's lock ran out (the API's 5 s): the caller's to
/// retry, a 409. Names the key, never what holds it.
fn busy(grant: &Grant, key: &KvKey) -> NsError {
    NsError::Conflict(format!(
        "key '{key}' in namespace '{}': the key is busy (another write holds it); try again",
        grant.namespace()
    ))
}

/// A store failure: logged with the `s3://` key, answered naming the key and
/// namespace only (§11).
fn store_failed(
    grant: &Grant,
    key: &KvKey,
    doing: &str,
    object: &str,
    err: &dyn std::fmt::Display,
) -> NsError {
    tracing::error!(object = %object, error = %err, namespace = %grant.namespace(), key = %key, "a KV value could not be {doing}");
    NsError::Store(format!(
        "key '{key}' in namespace '{}' could not be {doing}: internal storage is unavailable; \
         retry later",
        grant.namespace()
    ))
}

/// `pg_advisory_xact_lock` on the key, its wait bounded by the grant's
/// [`Grant::lock_wait`] (`SET LOCAL lock_timeout`): past it, [`busy`].
async fn lock_key(conn: &mut PgConnection, grant: &Grant, key: &KvKey) -> Result<(), NsError> {
    bound_lock_wait(conn, grant).await?;
    lock_one(conn, grant, key).await
}

/// `SET LOCAL lock_timeout` to the grant's [`Grant::lock_wait`], once per
/// transaction.
async fn bound_lock_wait(conn: &mut PgConnection, grant: &Grant) -> Result<(), NsError> {
    if let Some(wait) = grant.lock_wait() {
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(lock_timeout_value(wait))
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

/// The key's xact lock, under whatever `lock_timeout` the transaction has.
async fn lock_one(conn: &mut PgConnection, grant: &Grant, key: &KvKey) -> Result<(), NsError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(kv_lock_text(grant, key))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            if is_lock_timeout(&e) {
                busy(grant, key)
            } else {
                db_err(e)
            }
        })?;
    Ok(())
}

/// The key's latest row, live or not (taken first, judged after).
async fn latest_row(
    conn: &mut PgConnection,
    grant: &Grant,
    key: &KvKey,
) -> Result<Option<KvEntry>, NsError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "SELECT ",
        wcols!(),
        " FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
          ORDER BY revision DESC LIMIT 1"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(key.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    row.map(entry).transpose()
}

/// The re-claim skip (§11): a task run whose earlier attempt wrote the
/// latest revision (not since deleted) writes nothing more.
fn reclaimed(latest: Option<&KvEntry>, by: CreatedBy) -> Option<KvEntry> {
    let CreatedBy::TaskRun { task_run, .. } = by else {
        return None;
    };
    latest
        .filter(|e| e.created_by_task_run == Some(task_run) && !e.deleted)
        .cloned()
}

/// When the new row expires: `now() + ttl` (seconds) at the commit, or an
/// instant kept from the latest revision (`Put`), or never.
enum Expiry {
    Ttl(Option<f64>),
    Keep(Option<DateTime<Utc>>),
}

/// The row of a revision whose object is in place, under the key's lock.
#[allow(clippy::too_many_arguments)]
async fn insert_row(
    conn: &mut PgConnection,
    grant: &Grant,
    key: &KvKey,
    revision: u32,
    kind: KvType,
    size: u64,
    description: Option<&str>,
    expiry: Expiry,
    by: CreatedBy,
) -> Result<KvEntry, NsError> {
    let (user, execution, task_run) = writer(by);
    let (ttl, keep) = match expiry {
        Expiry::Ttl(secs) => (secs, None),
        Expiry::Keep(at) => (None, at),
    };
    // `created_at` and `now + ttl` are one `clock_timestamp()` read under
    // the key's lock (Task 3 review I4): a writer that waited for the lock
    // is not born expired, and `created_at` rises with the revision, which
    // `superseded_at` (PurgeKV's `before`) relies on.
    let row: Row = sqlx::query_as(concat!(
        "WITH t AS (SELECT clock_timestamp() AS at) \
         INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, description, created_at, \
            expires_at, created_by_user, created_by_execution, created_by_task_run) \
         SELECT $1, $2, $3, $4, $5, $6, $7, t.at, \
                CASE WHEN $8::float8 IS NOT NULL THEN t.at + make_interval(secs => $8::float8) \
                     ELSE $9::timestamptz END, \
                $10, $11, $12 FROM t \
         RETURNING ",
        wcols!()
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(key.as_str())
    .bind(
        i32::try_from(revision)
            .map_err(|_| internal(format!("key '{key}' would reach revision {revision}")))?,
    )
    .bind(kind.as_str())
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(description)
    .bind(ttl)
    .bind(keep)
    .bind(user)
    .bind(execution)
    .bind(task_run)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    entry(row)
}

/// `head` of the object just written: its size must be `size`.
async fn check_head(
    fs: &FileStore,
    grant: &Grant,
    key: &KvKey,
    object: &str,
    size: u64,
) -> Result<(), NsError> {
    match fs.store.head(object).await {
        Ok(Some(n)) if n == size => Ok(()),
        Ok(Some(n)) => Err(store_failed(
            grant,
            key,
            "stored",
            object,
            &format!("wrote {n} bytes of a {size}-byte value"),
        )),
        Ok(None) => Err(store_failed(
            grant,
            key,
            "stored",
            object,
            &"written, but head finds no object",
        )),
        Err(e) => Err(store_failed(grant, key, "stored", object, &e)),
    }
}

/// A blocking-pool job that did not answer: cancelled (shutdown; retry) or
/// panicked (egret's bug).
fn join_failed(grant: &Grant, key: &KvKey, join: tokio::task::JoinError) -> NsError {
    if join.is_cancelled() {
        NsError::Store(format!(
            "key '{key}' in namespace '{}': the write was cancelled; retry later",
            grant.namespace()
        ))
    } else {
        wrong_grant(egret_core::Error::Internal(anyhow::anyhow!(
            "checking the value of key '{key}' panicked: {join}"
        )))
    }
}

/// [`check_readable`] on the blocking pool (it runs the Ion reader): the
/// exact bytes to store, or the refusal mapped by [`unstorable`].
async fn readable(
    grant: &Grant,
    key: &KvKey,
    value: KvValue,
    cap: u64,
) -> Result<Vec<u8>, NsError> {
    tokio::task::spawn_blocking(move || check_readable(&value, cap))
        .await
        .map_err(|join| join_failed(grant, key, join))?
        .map_err(|e| refused_value(grant, key, e))
}

/// [`unstorable`], the message prefixed with the key and namespace.
fn refused_value(grant: &Grant, key: &KvKey, e: Unstorable) -> NsError {
    let about = |m: String| format!("key '{key}' in namespace '{}': {m}", grant.namespace());
    unstorable(match e {
        Unstorable::TooLarge(m) => Unstorable::TooLarge(about(m)),
        Unstorable::Invalid(m) => Unstorable::Invalid(about(m)),
    })
}

/// `ttl` as `make_interval`'s seconds. The caller validated it with
/// `parse_ttl`; a negative one or one past [`MAX_TTL_DAYS`] is refused.
fn ttl_secs(
    grant: &Grant,
    key: &KvKey,
    ttl: Option<chrono::TimeDelta>,
) -> Result<Option<f64>, NsError> {
    match ttl {
        None => Ok(None),
        // Zero is the API header's "expired at once" (w9c
        // `api.headers.h_ttl_zero`); zero and the sign are each caller's
        // `parse_ttl` mode to judge (Task 3 review I1), not this function's.
        Some(t) if t >= chrono::TimeDelta::zero() && t <= max_ttl() => {
            Ok(Some(t.num_milliseconds() as f64 / 1000.0))
        }
        // Only a caller that skipped `parse_ttl` gets here: the author's,
        // never a database error (Task 3 review I3).
        Some(t) => Err(NsError::Author(format!(
            "key '{key}' in namespace '{}': ttl: {}s is outside 0 to {MAX_TTL_DAYS} days \
             (1,000 years)",
            grant.namespace(),
            t.num_seconds()
        ))),
    }
}

/// §4.3.
///
/// 1. `require(Kv, Write)`; the bytes are `check_readable(&new.value,
///    max_read_bytes)` on the blocking pool (refusals through
///    [`unstorable`]), before anything is staged.
/// 2. `staging::record`, then `fs.store.put(staged, bytes)`, holding NO
///    connection and NO lock.
/// 3. On a detached task, one transaction: the key's lock (a `lock_wait`
///    past: `Conflict` "busy") -> the latest row -> the re-claim skip ->
///    the overwrite rule (a LIVE latest with `overwrite: false`: `Conflict`
///    with Kestra's message) -> revision = max + 1 -> server-side `copy` of
///    the staging object -> `head` == size -> INSERT (`expires_at = now() +
///    ttl`) -> commit.
/// 4. `staging::forget` on every way out.
pub async fn set(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    key: &KvKey,
    new: NewValue,
    by: CreatedBy,
) -> Result<KvWritten, NsError> {
    need(grant, Access::Write)?;
    let ttl = ttl_secs(grant, key, new.ttl)?;
    let NewValue {
        value,
        description,
        overwrite,
        ..
    } = new;
    let kind = value.kind;
    let bytes = readable(grant, key, value, fs.caps.max_read_bytes).await?;
    let size = bytes.len() as u64;
    let staged = staging::record(pool, fs, grant.workspace()).await?;
    if let Err(e) = fs.store.put(&staged.uri, bytes).await {
        let err = store_failed(grant, key, "stored", &staged.uri, &e);
        staging::forget(pool, fs, &staged).await;
        return Err(err);
    }
    let (pool, fs, grant, key) = (pool.clone(), fs.clone(), grant.clone(), key.clone());
    detached("KV write", async move {
        let result = async {
            let mut tx = pool.begin().await.map_err(db_err)?;
            lock_key(&mut tx, &grant, &key).await?;
            let latest = latest_row(&mut tx, &grant, &key).await?;
            if let Some(e) = reclaimed(latest.as_ref(), by) {
                return Ok(KvWritten::Already(e));
            }
            if !overwrite && latest.as_ref().is_some_and(KvEntry::live) {
                return Err(NsError::Conflict(format!(
                    "Cannot set value for key '{key}'. Key already exists and `overwrite` is \
                     set to `false`."
                )));
            }
            let revision = latest.map_or(1, |l| l.revision + 1);
            let object = fs.namespace_kv_uri(&grant, &key, revision);
            fs.store
                .copy(&staged.uri, &object)
                .await
                .map_err(|e| store_failed(&grant, &key, "stored", &object, &e))?;
            check_head(&fs, &grant, &key, &object, size).await?;
            let e = insert_row(
                &mut tx,
                &grant,
                &key,
                revision,
                kind,
                size,
                description.as_deref(),
                Expiry::Ttl(ttl),
                by,
            )
            .await?;
            tx.commit().await.map_err(db_err)?;
            Ok(KvWritten::Written(e))
        }
        .await;
        staging::forget(&pool, &fs, &staged).await;
        result
    })
    .await
}

/// §8.5. `patch` is the rendered value (a flow's: `KvData::from_json`, so a
/// number is an int or a float, never a decimal); nothing is staged. On a
/// detached task, ONE transaction holding the key's lock: the latest row ->
/// the re-claim skip -> a live latest: `read_value` (bounded, decimals
/// kept) -> `merge::merge` and `check_readable` (on the blocking pool;
/// server-paced, under the lock) -> `fs.store.put(revision key, those
/// bytes)` -> `head` -> INSERT keeping the latest's description and
/// `expires_at`. Missing (never written, soft-deleted, or expired: the
/// user's ruling, 2026-10-07): `error_on_missing` -> NotFound with Kestra's
/// message; else `merge::merge(None, patch)`, with no description and no
/// expiry. A shape the merge refuses is `Author`.
pub async fn put_merge(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    key: &KvKey,
    patch: KvData,
    error_on_missing: bool,
    by: CreatedBy,
) -> Result<KvWritten, NsError> {
    need(grant, Access::Write)?;
    let (pool, fs, grant, key) = (pool.clone(), fs.clone(), grant.clone(), key.clone());
    detached("KV put", async move {
        let mut tx = pool.begin().await.map_err(db_err)?;
        lock_key(&mut tx, &grant, &key).await?;
        let latest = latest_row(&mut tx, &grant, &key).await?;
        if let Some(e) = reclaimed(latest.as_ref(), by) {
            return Ok(KvWritten::Already(e));
        }
        let live = latest.as_ref().filter(|e| e.live());
        let current = match live {
            Some(e) => Some(read_value(&fs, &grant, e).await?),
            None if error_on_missing => {
                return Err(NsError::NotFound(format!(
                    "No value found for key '{key}' in namespace '{}' and `errorOnMissing` is \
                     set to true",
                    grant.namespace()
                )))
            }
            None => None,
        };
        let cap = fs.caps.max_read_bytes;
        let (g, k) = (grant.clone(), key.clone());
        let checked = tokio::task::spawn_blocking(move || {
            // The bare message, exact as the fixture's `put_merge` cells (review
            // M1): any key or namespace prefix belongs to the task layer.
            let merged = merge::merge(current.as_ref(), &patch).map_err(NsError::Author)?;
            let value = KvValue {
                kind: KvType::of_data(&merged),
                data: merged,
            };
            let kind = value.kind;
            check_readable(&value, cap)
                .map(|bytes| (kind, bytes))
                .map_err(|e| refused_value(&g, &k, e))
        })
        .await
        .map_err(|join| join_failed(&grant, &key, join))?;
        let (kind, bytes) = checked?;
        let size = bytes.len() as u64;
        let revision = latest.as_ref().map_or(1, |l| l.revision + 1);
        let object = fs.namespace_kv_uri(&grant, &key, revision);
        fs.store
            .put(&object, bytes)
            .await
            .map_err(|e| store_failed(&grant, &key, "stored", &object, &e))?;
        check_head(&fs, &grant, &key, &object, size).await?;
        let (description, expires_at) = match live {
            Some(e) => (e.description.as_deref(), e.expires_at),
            None => (None, None),
        };
        let e = insert_row(
            &mut tx,
            &grant,
            &key,
            revision,
            kind,
            size,
            description,
            Expiry::Keep(expires_at),
            by,
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(KvWritten::Written(e))
    })
    .await
}

/// `deleted_at = now()` and `deleted_by_*` from `by` on the rows `ids`.
async fn flag_deleted(
    conn: &mut PgConnection,
    grant: &Grant,
    ids: &[i64],
    by: CreatedBy,
) -> Result<(), NsError> {
    let (user, execution, task_run) = writer(by);
    sqlx::query(
        "UPDATE workflow.kv_entries \
            SET deleted = true, deleted_at = clock_timestamp(), deleted_by_user = $3, \
                deleted_by_execution = $4, deleted_by_task_run = $5 \
          WHERE workspace_id = $1 AND namespace = $2 AND id = ANY($6)",
    )
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(user)
    .bind(execution)
    .bind(task_run)
    .bind(ids)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// §4.4/§8.4: under the key's lock, a LIVE latest row is flagged deleted
/// (`deleted_at = now()`, `deleted_by_*` from `by`); Missing for
/// none/deleted/expired; Already when the latest row was deleted by this
/// task run (`CreatedBy::TaskRun`). Adds no revision.
pub async fn delete(
    pool: &PgPool,
    grant: &Grant,
    key: &KvKey,
    by: CreatedBy,
) -> Result<Deleted, NsError> {
    need(grant, Access::Write)?;
    let (pool, grant, key) = (pool.clone(), grant.clone(), key.clone());
    detached("KV delete", async move {
        let mut tx = pool.begin().await.map_err(db_err)?;
        lock_key(&mut tx, &grant, &key).await?;
        let latest = latest_row(&mut tx, &grant, &key).await?;
        if let (CreatedBy::TaskRun { task_run, .. }, Some(e)) = (by, &latest) {
            if e.deleted && e.deleted_by_task_run == Some(task_run) {
                return Ok(Deleted::Already);
            }
        }
        match latest {
            Some(e) if e.live() => {
                flag_deleted(&mut tx, &grant, &[e.id], by).await?;
                tx.commit().await.map_err(db_err)?;
                Ok(Deleted::Deleted)
            }
            _ => Ok(Deleted::Missing),
        }
    })
    .await
}

/// §9's bulk delete: keys sorted and de-duplicated, ONE transaction taking
/// each key's lock in ascending order (each wait bounded by the grant's
/// lock_wait; past it, `Conflict` naming that key and nothing deleted), all
/// or nothing. Answers the keys that were live and are now deleted, in
/// ascending order. More than [`MAX_BULK_DELETE_KEYS`] keys given:
/// `TooLarge` naming the cap, nothing taken (the user's ruling, 2026-10-07).
/// Touches no object.
pub async fn delete_many(
    pool: &PgPool,
    _fs: &FileStore,
    grant: &Grant,
    keys: &[KvKey],
    by: CreatedBy,
) -> Result<Vec<KvKey>, NsError> {
    need(grant, Access::Write)?;
    if keys.len() > MAX_BULK_DELETE_KEYS {
        return Err(NsError::TooLarge(format!(
            "a bulk delete takes at most {MAX_BULK_DELETE_KEYS} keys; {} were given",
            keys.len()
        )));
    }
    let mut keys = keys.to_vec();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return Ok(vec![]);
    }
    let (pool, grant) = (pool.clone(), grant.clone());
    detached("KV delete", async move {
        // Review M2: at most BULK_DELETES_AT_ONCE bulk deletes hold their
        // (up to 1,000) advisory locks at once in this process, so they
        // cannot fill Postgres's shared lock table (spec §15). Waiting for a
        // permit holds no connection.
        let _permit = BULK_DELETES
            .acquire()
            .await
            .map_err(|e| internal(format!("the bulk delete semaphore is closed: {e}")))?;
        let mut tx = pool.begin().await.map_err(db_err)?;
        bound_lock_wait(&mut tx, &grant).await?;
        for key in &keys {
            lock_one(&mut tx, &grant, key).await?;
        }
        let names: Vec<&str> = keys.iter().map(KvKey::as_str).collect();
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT DISTINCT ON (key) ",
            wcols!(),
            " FROM workflow.kv_entries \
              WHERE workspace_id = $1 AND namespace = $2 AND key = ANY($3) \
              ORDER BY key, revision DESC"
        ))
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(&names)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        let live: Vec<KvEntry> = entries(rows)?.into_iter().filter(KvEntry::live).collect();
        let ids: Vec<i64> = live.iter().map(|e| e.id).collect();
        flag_deleted(&mut tx, &grant, &ids, by).await?;
        tx.commit().await.map_err(db_err)?;
        let mut gone: Vec<KvKey> = live.into_iter().map(|e| e.key).collect();
        gone.sort();
        Ok(gone)
    })
    .await
}

/// The session-lock key of `(grant, key)`: `SELECT hashtextextended($1, 0)`,
/// the hash [`lock_key`] takes, computed by SQL.
pub(crate) async fn session_key(pool: &PgPool, grant: &Grant, key: &KvKey) -> Result<i64, NsError> {
    sqlx::query_scalar("SELECT hashtextextended($1, 0)")
        .bind(kv_lock_text(grant, key))
        .fetch_one(pool)
        .await
        .map_err(db_err)
}

/// Unlocks `session`; a failure is logged, not returned: the session's
/// connection is then closed, which releases its lock.
pub(crate) async fn release(session: LockedSession, what: &str) {
    if let Err(e) = session.unlock().await {
        tracing::warn!(operation = %what, error = %e, "a KV session lock was released by closing its connection");
    }
}

/// Deletes the objects of the purged `revisions`; a failure is a WARN
/// orphan, not an error: the row is gone, so the value is gone for every
/// reader (§11).
pub(crate) async fn delete_objects(fs: &FileStore, grant: &Grant, key: &KvKey, revisions: &[i32]) {
    for revision in revisions {
        let Ok(revision) = u32::try_from(*revision) else {
            continue;
        };
        let object = fs.namespace_kv_uri(grant, key, revision);
        if let Err(e) = fs.store.delete(&object).await {
            tracing::warn!(object = %object, error = %e, "a purged KV value's object could not be deleted: it is an orphan");
        }
    }
}

type PurgeRowCols = (
    i64,
    Uuid,
    String,
    String,
    i32,
    String,
    i64,
    Option<String>,
    Option<DateTime<Utc>>,
    bool,
    DateTime<Utc>,
    bool,
    Option<Uuid>,
    Option<Uuid>,
    Option<DateTime<Utc>>,
);

/// §8.6, per key: `require(Kv, Purge)`; on a detached task, take the key's
/// SESSION lock (the same hash as the transaction lock; the grant's
/// `lock_wait` bounds the wait), re-read every row of the key with each
/// row's `superseded_at` (`lead(created_at) OVER (ORDER BY revision)`: the
/// next revision's `created_at`; more than `cap` rows: TooLarge naming the
/// key and the cap), let `judge` pick the row ids to purge FROM THOSE FRESH
/// ROWS (plan item 13), `DELETE … WHERE id = ANY($ids) RETURNING revision`
/// (one statement: its own commit), delete each removed revision's object
/// (a failure is a WARN orphan), unlock. Answers the rows removed. An id
/// the judge names that is not one of the key's rows removes nothing.
pub async fn purge_key<F>(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    key: &KvKey,
    cap: u64,
    judge: F,
) -> Result<u64, NsError>
where
    F: FnOnce(&[PurgeRow]) -> Vec<i64> + Send + 'static,
{
    require(grant, Resource::Kv, Access::Purge).map_err(wrong_grant)?;
    let (pool, fs, grant, key) = (pool.clone(), fs.clone(), grant.clone(), key.clone());
    detached("KV purge", async move {
        let lock = session_key(&pool, &grant, &key).await?;
        let mut session = LockedSession::acquire(&pool, &[lock], grant.lock_wait())
            .await
            .map_err(|e| {
                if is_lock_timeout_error(&e) {
                    busy(&grant, &key)
                } else {
                    NsError::Db(e)
                }
            })?;
        let result = async {
            let rows: Vec<PurgeRowCols> = sqlx::query_as(concat!(
                "SELECT ",
                cols!(),
                ", lead(created_at) OVER (ORDER BY revision) AS superseded_at \
                  FROM workflow.kv_entries \
                  WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
                  ORDER BY revision LIMIT $4"
            ))
            .bind(grant.workspace())
            .bind(grant.namespace())
            .bind(key.as_str())
            .bind(limit(cap))
            .fetch_all(session.conn())
            .await
            .map_err(db_err)?;
            if rows.len() as u64 > cap {
                return Err(NsError::TooLarge(format!(
                    "key '{key}' in namespace '{}' holds more than {cap} revisions, the most one \
                     purge reads (workflow.storage.max_namespace_files)",
                    grant.namespace()
                )));
            }
            let rows: Vec<PurgeRow> = rows
                .into_iter()
                .map(|r| {
                    let superseded_at = r.14;
                    Ok(PurgeRow {
                        entry: entry((
                            r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9, r.10, r.11, r.12,
                            r.13,
                        ))?,
                        superseded_at,
                    })
                })
                .collect::<Result<_, NsError>>()?;
            let ids = judge(&rows);
            if ids.is_empty() {
                return Ok(0);
            }
            let gone: Vec<i32> = sqlx::query_scalar(
                "DELETE FROM workflow.kv_entries \
                  WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND id = ANY($4) \
                  RETURNING revision",
            )
            .bind(grant.workspace())
            .bind(grant.namespace())
            .bind(key.as_str())
            .bind(&ids)
            .fetch_all(session.conn())
            .await
            .map_err(db_err)?;
            delete_objects(&fs, &grant, &key, &gone).await;
            Ok(gone.len() as u64)
        }
        .await;
        release(session, "KV purge").await;
        result
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_cut_at_dots_deepest_first() {
        assert_eq!(ancestors("a.b.c"), ["a.b.c", "a.b", "a"]);
        assert_eq!(ancestors("a"), ["a"]);
        assert_eq!(
            ancestors("w9c.inh.a.b"),
            ["w9c.inh.a.b", "w9c.inh.a", "w9c.inh", "w9c"]
        );
    }
}
