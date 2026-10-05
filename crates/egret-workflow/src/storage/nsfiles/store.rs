//! The read side of `workflow.namespace_files` (migration 0169) and the
//! objects behind it (W9b spec §4.1, §4.5).
//!
//! Every statement is scoped `WHERE workspace_id = $1 AND namespace = $2`,
//! both taken from the [`Grant`]: the workspace is the caller's, never a
//! URI's. A path's state is its HIGHEST revision's row; a soft-deleted path is
//! one whose latest row is `deleted`. The latest row is taken first and the
//! `deleted` filter applied after it -- filtering first would resurrect an
//! older live revision of a deleted path. Children are matched with
//! `left(path, length($3)) = $3`, never `LIKE` (`_` is common in file names).
//! A listing is in byte order (`COLLATE "C"`), whatever the database's
//! collation.
//!
//! # The index
//!
//! The unique constraint `workflow_namespace_files_revision_key` is
//! `(workspace_id, namespace, path, revision)`: it leads with
//! `(workspace_id, namespace, path)`, so it IS spec §4.1's index for the latest
//! row (`ORDER BY revision DESC LIMIT 1` scans it backwards, and `DISTINCT ON
//! (path) … ORDER BY path, revision DESC` reads it in order).

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use tokio::io::AsyncBufRead;
use uuid::Uuid;

use super::access::Grant;
use super::path::NsPath;
use super::NsError;
use crate::storage::FileStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsEntry {
    /// The row's workspace and namespace: [`open`] refuses an entry whose
    /// pair is not its grant's (fix round 1 M2).
    pub workspace: Uuid,
    pub namespace: String,
    pub path: NsPath,
    pub revision: u32,
    pub is_directory: bool,
    pub size: u64,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Latest,
    Revision(u32),
}

/// The columns of one [`NsEntry`], in [`Row`]'s order.
macro_rules! entry_cols {
    () => {
        "workspace_id, namespace, path, revision, is_directory, size, deleted, created_at"
    };
}

/// The latest row of every path of the grant's namespace whose path starts
/// with `$3` and is not `$3` itself, live or not: the inner query of every
/// listing. `DISTINCT ON` keeps each path's first row in `revision DESC`
/// order, its highest.
macro_rules! latest_under {
    () => {
        concat!(
            "SELECT DISTINCT ON (path) ",
            entry_cols!(),
            " FROM workflow.namespace_files \
              WHERE workspace_id = $1 AND namespace = $2 \
                AND left(path, length($3)) = $3 AND path <> $3 \
              ORDER BY path, revision DESC"
        )
    };
}

type Row = (Uuid, String, String, i32, bool, i64, bool, DateTime<Utc>);

/// egret's bug, not the author's and not an outage: a 500.
fn internal(message: String) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::anyhow!(message)))
}

fn db_err(e: sqlx::Error) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::Error::new(e)))
}

fn entry(row: Row) -> Result<NsEntry, NsError> {
    let (workspace, namespace, path, revision, is_directory, size, deleted, created_at) = row;
    let parsed = if is_directory {
        NsPath::parse_dir(&path)
    } else {
        NsPath::parse_file(&path)
    };
    let bad = |why: String| {
        NsError::Db(egret_core::Error::Internal(anyhow::anyhow!(
            "workflow.namespace_files holds {path:?}, which egret would not have written: {why}"
        )))
    };
    let parsed = parsed.map_err(&bad)?;
    if parsed.as_str() != path {
        return Err(bad("it is not in its stored form".into()));
    }
    Ok(NsEntry {
        workspace,
        namespace,
        path: parsed,
        revision: u32::try_from(revision).map_err(|_| bad(format!("revision {revision}")))?,
        is_directory,
        size: u64::try_from(size).map_err(|_| bad(format!("size {size}")))?,
        deleted,
        created_at,
    })
}

fn entries(rows: Vec<Row>) -> Result<Vec<NsEntry>, NsError> {
    rows.into_iter().map(entry).collect()
}

/// `LIMIT cap + 1`, as Postgres takes it.
fn limit(cap: u64) -> i64 {
    i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX)
}

/// `rows` holds at most `cap + 1` rows; the `+1` is the refusal.
fn capped(rows: Vec<NsEntry>, cap: u64, what: &str) -> Result<Vec<NsEntry>, NsError> {
    if rows.len() as u64 > cap {
        return Err(NsError::TooLarge(format!(
            "{what} holds more than {cap} namespace files, the most one answer may hold \
             (workflow.storage.max_namespace_files)"
        )));
    }
    Ok(rows)
}

fn not_found(grant: &Grant, path: &NsPath) -> NsError {
    NsError::NotFound(format!(
        "namespace file '{path}' does not exist in namespace '{}'",
        grant.namespace()
    ))
}

/// Latest = the highest revision, which must not be deleted (a soft-deleted
/// path is NotFound to a latest read); Revision(n) = that row, served even
/// when the path is soft-deleted and even when row n is the deleted one (the
/// user's ruling R1, 2026-10-05; w9b `layout.read_by_revision_after_rewrite`).
pub async fn resolve(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
    want: Want,
) -> Result<NsEntry, NsError> {
    let row: Option<Row> = match want {
        Want::Latest => sqlx::query_as(concat!(
            "SELECT ",
            entry_cols!(),
            " FROM workflow.namespace_files \
              WHERE workspace_id = $1 AND namespace = $2 AND path = $3 \
              ORDER BY revision DESC LIMIT 1"
        ))
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(path.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?,
        Want::Revision(n) => {
            let Ok(n) = i32::try_from(n) else {
                return Err(revision_not_found(grant, path, n));
            };
            sqlx::query_as(concat!(
                "SELECT ",
                entry_cols!(),
                " FROM workflow.namespace_files \
                  WHERE workspace_id = $1 AND namespace = $2 AND path = $3 AND revision = $4"
            ))
            .bind(grant.workspace())
            .bind(grant.namespace())
            .bind(path.as_str())
            .bind(n)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?
        }
    };
    let Some(row) = row else {
        return Err(match want {
            Want::Latest => not_found(grant, path),
            Want::Revision(n) => revision_not_found(grant, path, n),
        });
    };
    let e = entry(row)?;
    // After taking the latest row, never before (this module's doc).
    if want == Want::Latest && e.deleted {
        return Err(not_found(grant, path));
    }
    Ok(e)
}

fn revision_not_found(grant: &Grant, path: &NsPath, n: u32) -> NsError {
    NsError::NotFound(format!(
        "revision {n} of namespace file '{path}' does not exist in namespace '{}'",
        grant.namespace()
    ))
}

/// Live latest rows under `dir` (None = the namespace root); recursive or one
/// level. Fetches at most `cap + 1` rows; the `+1` is `NsError::TooLarge`
/// naming workflow.storage.max_namespace_files.
pub async fn list_live(
    conn: &mut PgConnection,
    grant: &Grant,
    dir: Option<&NsPath>,
    recursive: bool,
    cap: u64,
) -> Result<Vec<NsEntry>, NsError> {
    // A file prefix (`/a`) would match its siblings (`/ab/x`): egret's bug,
    // refused in every build, not only under `debug_assert!` (fix round 1 M1).
    if let Some(file) = dir.filter(|d| !d.is_dir()) {
        return Err(internal(format!(
            "egret listed namespace files under '{file}', which is a file, not a directory"
        )));
    }
    let prefix = dir.map_or("/", NsPath::as_str);
    // One level: nothing after the prefix holds a '/', but a directory's own
    // trailing one (`left(rest, -1)` drops the last character).
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM (",
        latest_under!(),
        ") latest \
          WHERE NOT deleted \
            AND ($4 OR strpos(left(substr(path, length($3) + 1), -1), '/') = 0) \
          ORDER BY path COLLATE \"C\" LIMIT $5"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(prefix)
    .bind(recursive)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let what = format!("directory '{prefix}' of namespace '{}'", grant.namespace());
    capped(entries(rows)?, cap, &what)
}

/// Every row of the path, ascending, `deleted` flagged; a soft-deleted path's
/// rows included (R1). No row: NotFound.
pub async fn revisions(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
) -> Result<Vec<NsEntry>, NsError> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND path = $3 \
          ORDER BY revision"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path.as_str())
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    if rows.is_empty() {
        return Err(not_found(grant, path));
    }
    entries(rows)
}

/// As Kestra's (R3): a case-insensitive substring of the path, files only,
/// live latest rows; `""` finds nothing (w9b `api.summary.search_rule`).
/// Fetches at most `cap + 1` rows, as [`list_live`].
pub async fn search(
    conn: &mut PgConnection,
    grant: &Grant,
    needle: &str,
    cap: u64,
) -> Result<Vec<NsEntry>, NsError> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM (",
        latest_under!(),
        ") latest \
          WHERE NOT deleted AND NOT is_directory AND strpos(lower(path), lower($4)) > 0 \
          ORDER BY path COLLATE \"C\" LIMIT $5"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind("/")
    .bind(needle)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let what = format!(
        "the search for {needle:?} in namespace '{}'",
        grant.namespace()
    );
    capped(entries(rows)?, cap, &what)
}

/// The namespaces of `workspace` that hold at least one row (PurgeFiles'
/// namespacePattern, Task 7).
pub async fn namespaces_with_files(
    conn: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<String>, NsError> {
    sqlx::query_scalar(
        "SELECT DISTINCT namespace FROM workflow.namespace_files \
          WHERE workspace_id = $1 ORDER BY namespace",
    )
    .bind(workspace)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// Stream the entry's object. A missing object is NsError::Author naming the
/// path and revision (§4.5, no fallback); any other store error is
/// NsError::Store. Neither message names the bucket or the key; the store's
/// own error goes to the server log.
pub async fn open(
    fs: &FileStore,
    grant: &Grant,
    e: &NsEntry,
) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, NsError> {
    let what = format!(
        "revision {} of namespace file '{}' in namespace '{}'",
        e.revision,
        e.path,
        grant.namespace()
    );
    // An entry another grant resolved names another namespace's object at
    // this path: egret's bug, refused before the store is asked (M2).
    if e.workspace != grant.workspace() || e.namespace != grant.namespace() {
        return Err(internal(format!(
            "egret opened namespace file '{}' of namespace '{}' holding a grant for \
             namespace '{}' (or another workspace)",
            e.path,
            e.namespace,
            grant.namespace()
        )));
    }
    if e.is_directory {
        return Err(NsError::Author(format!(
            "{what} is a directory: it has no contents"
        )));
    }
    let key = fs.namespace_object_uri(grant, &e.path, e.revision);
    match fs.store.get_stream(&key).await {
        Ok(r) => Ok(r),
        Err(egret_storage::StorageError::NotFound(_)) => {
            tracing::warn!(file = %what, "a namespace file row has no object");
            Err(NsError::Author(format!(
                "{what} is recorded but its contents are gone from internal storage"
            )))
        }
        Err(err) => {
            tracing::error!(file = %what, error = %err, "internal storage failed to open a namespace file");
            Err(NsError::Store(format!(
                "{what}: internal storage is unavailable; retry later"
            )))
        }
    }
}
