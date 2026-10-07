//! The read side of `workflow.kv_entries` (migration 0175) and the objects
//! behind it (W9c spec §4.1, §4.4-§4.6).
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

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use super::key::KvKey;
use super::value::{decode, KvType};
use crate::storage::nsfiles::access::{require, Access, Grant, Resource};
use crate::storage::nsfiles::store::Want;
use crate::storage::nsfiles::NsError;
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
            return Err(internal(
                "egret read the KV store of two workspaces in one walk".to_string(),
            ));
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

/// The API's inheritance listing: `grants` are the ANCESTORS (not the
/// namespace itself), deepest first; the live keys of each, the first
/// occurrence of a key winning (KVController.java:73-100). Bounded by `cap`
/// over the union (before a shadowed key is dropped).
pub async fn list_inherited(
    conn: &mut PgConnection,
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
    let what = format!(
        "the parents of namespace '{}'",
        grants.first().map_or("", |g| g.namespace())
    );
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

/// The entry's object, decoded. size > max_read_bytes: TooLarge naming the
/// cap (no download). A missing object: Author naming the key and the
/// revision (no fallback, §4.5). Any other store error: Store. Takes NO
/// connection: call it after the row was read and the connection let go.
pub async fn read_value(
    fs: &FileStore,
    grant: &Grant,
    e: &KvEntry,
) -> Result<serde_json::Value, NsError> {
    need(grant, Access::Read)?;
    // An entry another grant resolved names another namespace's object: egret's
    // bug, refused before the store is asked.
    if e.workspace != grant.workspace() || e.namespace != grant.namespace() {
        return Err(internal(format!(
            "egret read key '{}' of namespace '{}' holding a grant for namespace '{}' (or \
             another workspace)",
            e.key,
            e.namespace,
            grant.namespace()
        )));
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
    decode(e.value_type, &bytes, cap).map_err(|why| NsError::Author(format!("{what}: {why}")))
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
