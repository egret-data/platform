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
use sqlx::{PgConnection, PgPool};
use tokio::io::AsyncBufRead;
use uuid::Uuid;

use super::access::{require, Access, Grant};
use super::path::NsPath;
use super::{CreatedBy, NsError};
use crate::storage::inputs::Guarded;
use crate::storage::lock::LockedSession;
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
    let parsed = parsed.map_err(bad)?;
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

// ─── The write side (W9b spec §4.2-§4.4, §8.4; Task 3) ──────────────────────
//
// # The lock
//
// Every writer locks the path's parent directories and the path, ascending by
// `lock_path` (a parent sorts first, being a prefix): `lock_path` drops a
// directory's trailing '/', so a file `/a` and a directory `/a/` share a key,
// and a writer of `/a/x` (which locks `/a` as a parent) and a writer of the
// file `/a` are serialised. The key is `hashtextextended(text, 0)` computed in
// SQL over the bound text `w9b:{workspace}:{namespace}:{lock_path}` (a value,
// never SQL). One statement per key, in order: a set-returning form over
// `unnest` does not guarantee the order, and every holder taking its keys in
// one global order is what keeps two of them from deadlocking.
//
// `put`, `create_directory` and `soft_delete` take transaction locks
// (`pg_advisory_xact_lock`), held across the upload and the row insert, so
// that no second writer computes the same revision. `purge_revisions` and
// `move_path` take the same keys as SESSION locks ([`LockedSession`]) held
// across "delete rows, commit, delete objects": a transaction lock would be
// released by that commit, and a writer could then take the purged revision
// number again and upload to its key, which the purge would then delete -- a
// row naming nothing (plan item 7). The two kinds of lock on one key conflict.

/// What [`put`] does when the path already holds a live file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The next revision (Kestra's default).
    Overwrite,
    /// `NsError::Conflict` naming the path; nothing is written.
    Error,
    /// Nothing is written; [`Written::Skipped`].
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    Written(NsEntry),
    Skipped(NsPath),
}

/// No cap on a read of rows the write side must see whole (a target's
/// history, a stale file's revisions): `limit` makes it `i64::MAX`.
const UNCAPPED: u64 = u64::MAX;

/// The lock key's text of one `lock_path`, bound as a value.
fn lock_text(grant: &Grant, lock_path: &str) -> String {
    format!(
        "w9b:{}:{}:{lock_path}",
        grant.workspace().hyphenated(),
        grant.namespace()
    )
}

/// The lock texts of `paths` and of every parent of each, ascending by
/// `lock_path`, each once.
fn lock_chain(grant: &Grant, paths: &[&NsPath]) -> Vec<String> {
    let mut lock_paths: Vec<String> = paths
        .iter()
        .flat_map(|p| {
            p.parents()
                .iter()
                .map(|parent| parent.lock_path().to_string())
                .chain(std::iter::once(p.lock_path().to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    lock_paths.sort();
    lock_paths.dedup();
    lock_paths.iter().map(|lp| lock_text(grant, lp)).collect()
}

/// `pg_advisory_xact_lock` on each text's hash, one statement per key, in the
/// order given.
async fn xact_lock(conn: &mut PgConnection, texts: &[String]) -> Result<(), NsError> {
    for text in texts {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(text)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

/// The session lock keys of `texts`, in the order given: one
/// `SELECT hashtextextended($1, 0)` each, so the hash is SQL's, the same one
/// [`xact_lock`] takes.
async fn session_keys(pool: &PgPool, texts: &[String]) -> Result<Vec<i64>, NsError> {
    let mut keys = Vec::with_capacity(texts.len());
    for text in texts {
        let key: i64 = sqlx::query_scalar("SELECT hashtextextended($1, 0)")
            .bind(text)
            .fetch_one(pool)
            .await
            .map_err(db_err)?;
        keys.push(key);
    }
    Ok(keys)
}

/// Unlocks `session`; a failure is logged, not returned: the session's
/// connection is then closed, which releases its locks, and the work done
/// under them stands.
async fn release(session: LockedSession, what: &str) {
    if let Err(e) = session.unlock().await {
        tracing::warn!(operation = %what, error = %e, "a namespace file session lock was released by closing its connection");
    }
}

/// Runs `work` on a detached task and awaits it (fix round 1, I3): dropping
/// the caller's future (a client that disconnects, a task timeout) no longer
/// stops a move or a purge part-way. Once started, it runs to completion --
/// its rollback included -- and only the answer is lost. A panic inside is
/// egret's bug, answered as `Internal`.
async fn detached<T, F>(what: &'static str, work: F) -> Result<T, NsError>
where
    F: std::future::Future<Output = Result<T, NsError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(work).await {
        Ok(result) => result,
        Err(e) => Err(internal(format!("a namespace file {what} stopped: {e}"))),
    }
}

fn writer(by: CreatedBy) -> (Option<Uuid>, Option<Uuid>) {
    match by {
        CreatedBy::User(u) => (Some(u), None),
        CreatedBy::Execution(e) => (None, Some(e)),
    }
}

fn need_write(grant: &Grant) -> Result<(), NsError> {
    require(grant, Access::Write).map_err(NsError::Db)
}

/// The latest row of `path` (a stored-form path string), live or not.
async fn latest(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &str,
) -> Result<Option<NsEntry>, NsError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND path = $3 \
          ORDER BY revision DESC LIMIT 1"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    row.map(entry).transpose()
}

/// Every row (every revision) of `path` and, when `path` is a directory, of
/// every path under it; by path (byte order) then revision. At most `cap + 1`.
async fn rows_at_or_under(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &str,
    cap: u64,
) -> Result<Vec<NsEntry>, NsError> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 \
            AND (path = $3 OR (right($3, 1) = '/' AND left(path, length($3)) = $3)) \
          ORDER BY path COLLATE \"C\", revision LIMIT $4"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    entries(rows)
}

/// The other kind's form of `path`: `/a/` for the file `/a`, `/a` for the
/// directory `/a/`.
fn other_kind(path: &NsPath) -> String {
    if path.is_dir() {
        path.lock_path().to_string()
    } else {
        format!("{}/", path.as_str())
    }
}

/// The file/directory rules of a write of `path` (§4.2 step 4), under its
/// locks. A live row of the other kind at `path`, or a live FILE where one of
/// its parent directories goes, is `Conflict`; a soft-deleted directory of
/// the other kind is purged here when it is alone (a directory has no object,
/// so its row is all there is) and is `Conflict` when paths under it still
/// hold history (fix round 1, M4: history is never purged implicitly, as the
/// user's ruling on I2 for a move); a soft-deleted FILE in either place is returned, to be
/// purged before the write is retried (its objects must go after a commit,
/// [`purge_stale`]). `subject` opens each message ("namespace file '/a'
/// cannot be written").
async fn check_kinds(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
    subject: &str,
) -> Result<Vec<NsPath>, NsError> {
    let ns = grant.namespace();
    let mut stale = Vec::new();
    if let Some(other) = latest(conn, grant, &other_kind(path)).await? {
        if !other.deleted {
            let what = if other.is_directory {
                "a directory"
            } else {
                "a file"
            };
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': {what} already exists at this path"
            )));
        }
        if other.is_directory {
            let under = rows_at_or_under(conn, grant, other.path.as_str(), 1).await?;
            if under.len() > 1 {
                return Err(NsError::Conflict(format!(
                    "{subject} in namespace '{ns}': a deleted directory at this path still \
                     holds the history of the files under it; purge them first"
                )));
            }
            sqlx::query(
                "DELETE FROM workflow.namespace_files \
                  WHERE workspace_id = $1 AND namespace = $2 AND path = $3",
            )
            .bind(grant.workspace())
            .bind(ns)
            .bind(other.path.as_str())
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        } else {
            stale.push(other.path);
        }
    }
    for parent in path.parents() {
        if let Some(file) = latest(conn, grant, parent.lock_path()).await? {
            if !file.deleted {
                return Err(NsError::Conflict(format!(
                    "{subject} in namespace '{ns}': its parent '{}' is a file",
                    file.path
                )));
            }
            stale.push(file.path);
        }
    }
    Ok(stale)
}

/// The rows of `path`'s missing parent directories -- and, with `itself`,
/// of `path` (a directory) -- inserted; a soft-deleted one is made live again
/// (a directory is always revision 1, so it cannot get a new revision, and a
/// live file under a deleted directory would be invisible to its listing).
/// An existing live one is left alone.
async fn ensure_directories(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
    itself: bool,
    by: CreatedBy,
) -> Result<(), NsError> {
    let mut dirs: Vec<String> = path
        .parents()
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    if itself {
        dirs.push(path.as_str().to_string());
    }
    if dirs.is_empty() {
        return Ok(());
    }
    let (user, execution) = writer(by);
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, \
            created_by_user, created_by_execution) \
         SELECT $1, $2, dir, 1, true, 0, $4, $5 FROM unnest($3::text[]) AS dir \
         ON CONFLICT ON CONSTRAINT workflow_namespace_files_revision_key \
           DO UPDATE SET deleted = false WHERE workflow.namespace_files.deleted",
    )
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(&dirs)
    .bind(user)
    .bind(execution)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Insert one revision's row and answer it, as `store`'s `entry` builds it.
async fn insert_row(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
    revision: u32,
    size: u64,
    deleted: bool,
    by: CreatedBy,
) -> Result<NsEntry, NsError> {
    let (user, execution) = writer(by);
    let revision = i32::try_from(revision)
        .map_err(|_| internal(format!("revision {revision} of '{path}' is out of range")))?;
    let size = i64::try_from(size)
        .map_err(|_| internal(format!("size {size} of '{path}' is out of range")))?;
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted, \
            created_by_user, created_by_execution) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING ",
        entry_cols!()
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path.as_str())
    .bind(revision)
    .bind(path.is_dir())
    .bind(size)
    .bind(deleted)
    .bind(user)
    .bind(execution)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    entry(row)
}

/// A store failure: logged with the `s3://` key, answered naming the
/// namespace and path only (§11).
fn store_failed(
    grant: &Grant,
    path: &NsPath,
    doing: &str,
    key: &str,
    err: &dyn std::fmt::Display,
) -> NsError {
    tracing::error!(key = %key, error = %err, namespace = %grant.namespace(), path = %path, "a namespace file could not be {doing}");
    NsError::Store(format!(
        "namespace file '{path}' in namespace '{}' could not be {doing}: internal storage is \
         unavailable; retry later",
        grant.namespace()
    ))
}

/// §8.4's hard delete of the given `(path, revision)` rows, on a connection
/// that holds every relevant key as a SESSION lock: the rows are deleted
/// (RETURNING) and committed, then each deleted file's object, in the order
/// `targets` gives. An object delete that fails after the commit is an orphan
/// logged at WARN, not an error: the row is gone, so the file is gone for
/// every reader (§11). Answers how many rows were deleted.
async fn purge_locked(
    conn: &mut PgConnection,
    fs: &FileStore,
    grant: &Grant,
    targets: &[(NsPath, u32)],
) -> Result<u64, NsError> {
    if targets.is_empty() {
        return Ok(0);
    }
    let paths: Vec<String> = targets
        .iter()
        .map(|(p, _)| p.as_str().to_string())
        .collect();
    let revisions: Vec<i32> = targets
        .iter()
        .map(|(_, r)| i32::try_from(*r).unwrap_or(i32::MAX))
        .collect();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(db_err)?;
    let gone: Vec<(String, i32)> = sqlx::query_as(
        "DELETE FROM workflow.namespace_files f \
          USING unnest($3::text[], $4::int[]) AS t(path, revision) \
          WHERE f.workspace_id = $1 AND f.namespace = $2 \
            AND f.path = t.path AND f.revision = t.revision \
          RETURNING f.path, f.revision",
    )
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(&paths)
    .bind(&revisions)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let gone: std::collections::BTreeSet<(String, i32)> = gone.into_iter().collect();
    for (path, revision) in targets {
        let r = i32::try_from(*revision).unwrap_or(i32::MAX);
        if path.is_dir() || !gone.contains(&(path.as_str().to_string(), r)) {
            continue;
        }
        let key = fs.namespace_object_uri(grant, path, *revision);
        if let Err(e) = fs.store.delete(&key).await {
            tracing::warn!(key = %key, error = %e, "a purged namespace file's object could not be deleted: it is an orphan");
        }
    }
    Ok(gone.len() as u64)
}

/// Purges every revision of each soft-deleted FILE in `stale` (found by
/// [`check_kinds`] where a directory is to go), each under the session locks
/// of its chain (its parents and itself, as every writer: fix round 1, I1),
/// after the write's transaction was rolled back (it held the same keys as
/// transaction locks). A file that is live again by then is left alone: the
/// retried write finds it and answers `Conflict`. Callers run it
/// [`detached`].
async fn purge_stale(
    pool: PgPool,
    fs: FileStore,
    grant: Grant,
    stale: Vec<NsPath>,
) -> Result<(), NsError> {
    let (pool, fs, grant) = (&pool, &fs, &grant);
    for file in &stale {
        let keys = session_keys(pool, &lock_chain(grant, &[file])).await?;
        let mut session = LockedSession::acquire(pool, &keys)
            .await
            .map_err(NsError::Db)?;
        let result = async {
            let conn = session.conn();
            match latest(conn, grant, file.as_str()).await? {
                Some(l) if l.deleted => {
                    let rows = rows_at_or_under(conn, grant, file.as_str(), UNCAPPED).await?;
                    let targets: Vec<(NsPath, u32)> =
                        rows.into_iter().map(|e| (e.path, e.revision)).collect();
                    purge_locked(conn, fs, grant, &targets).await.map(|_| ())
                }
                _ => Ok(()),
            }
        }
        .await;
        release(session, "purge a soft-deleted file in a directory's way").await;
        result?;
    }
    Ok(())
}

/// §4.2: lock -> read latest -> conflict rule -> revision = max + 1 (1 when
/// none) -> `put_stream` to the revision's key under `max_file_bytes` ->
/// `head` -> insert the row -> insert missing parent directory rows ->
/// commit. A live row of the other kind is `Conflict`; a soft-deleted one is
/// purged first. Commits immediately: namespace files are not in any
/// execution's transaction (§6).
///
/// A failed upload rolls the transaction back and leaves at most an
/// unindexed object at the revision's key, which the next write of that
/// revision overwrites. A body that fails to read (not the store) is
/// `NsError::Store` too: the caller's source is internal storage or a
/// request body, neither the author's.
pub async fn put(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    conflict: Conflict,
    by: CreatedBy,
) -> Result<Written, NsError> {
    need_write(grant)?;
    if path.is_dir() {
        return Err(internal(format!(
            "egret wrote contents to '{path}', which is a directory"
        )));
    }
    let ns = grant.namespace();
    let subject = format!("namespace file '{path}' cannot be written");
    let texts = lock_chain(grant, &[path]);
    // At most one retry: after purging a soft-deleted file in a parent
    // directory's way (see `check_kinds`).
    for attempt in 0..2 {
        let mut tx = pool.begin().await.map_err(db_err)?;
        xact_lock(&mut tx, &texts).await?;
        let stale = check_kinds(&mut tx, grant, path, &subject).await?;
        if !stale.is_empty() {
            // Rolled back, and awaited: its locks are gone before the purge
            // asks for the same key (fix round 1, M2).
            tx.rollback().await.map_err(db_err)?;
            if attempt > 0 {
                return Err(NsError::Conflict(format!(
                    "{subject} in namespace '{ns}': the paths around it changed while it was \
                     being written; retry"
                )));
            }
            detached(
                "purge",
                purge_stale(pool.clone(), fs.clone(), grant.clone(), stale),
            )
            .await?;
            continue;
        }
        let current = latest(&mut tx, grant, path.as_str()).await?;
        if current.as_ref().is_some_and(|l| !l.deleted) {
            match conflict {
                Conflict::Overwrite => {}
                Conflict::Error => {
                    return Err(NsError::Conflict(format!(
                        "namespace file '{path}' already exists in namespace '{ns}' and \
                         conflict is set to ERROR"
                    )))
                }
                Conflict::Skip => return Ok(Written::Skipped(path.clone())),
            }
        }
        let revision = current.map_or(1, |l| l.revision + 1);
        let key = fs.namespace_object_uri(grant, path, revision);
        let cap = fs.caps.max_file_bytes;
        let mut guarded = Guarded {
            inner: body,
            cap,
            read: 0,
            over: false,
            failed: None,
        };
        let put = fs.store.put_stream(&key, &mut guarded, None).await;
        if guarded.over {
            return Err(NsError::TooLarge(format!(
                "namespace file '{path}' in namespace '{ns}' is over the {cap} bytes one file \
                 may be (workflow.storage.max_file_bytes)"
            )));
        }
        if let Some(e) = guarded.failed {
            return Err(store_failed(
                grant,
                path,
                "read while it was being stored",
                &key,
                &e,
            ));
        }
        put.map_err(|e| store_failed(grant, path, "stored", &key, &e))?;
        let size = match fs.store.head(&key).await {
            Ok(Some(size)) => size,
            Ok(None) => {
                return Err(store_failed(
                    grant,
                    path,
                    "stored",
                    &key,
                    &"written, but head finds no object",
                ))
            }
            Err(e) => return Err(store_failed(grant, path, "stored", &key, &e)),
        };
        let e = insert_row(&mut tx, grant, path, revision, size, false, by).await?;
        ensure_directories(&mut tx, grant, path, false, by).await?;
        tx.commit().await.map_err(db_err)?;
        return Ok(Written::Written(e));
    }
    unreachable!("the loop returns on its second attempt")
}

/// The directory and its missing parents; an existing live directory is Ok
/// (idempotent); a live file there (or where a parent goes) is `Conflict`.
/// Unlike Kestra, which makes no row for a parent, every missing parent gets
/// a row (w9b `layout.create_directory`, chosen).
pub async fn create_directory(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    by: CreatedBy,
) -> Result<NsEntry, NsError> {
    need_write(grant)?;
    if !path.is_dir() {
        return Err(internal(format!(
            "egret created '{path}' as a directory, which is a file path"
        )));
    }
    let ns = grant.namespace();
    let subject = format!("cannot create directory '{}'", path.lock_path());
    let texts = lock_chain(grant, &[path]);
    for attempt in 0..2 {
        let mut tx = pool.begin().await.map_err(db_err)?;
        xact_lock(&mut tx, &texts).await?;
        let stale = check_kinds(&mut tx, grant, path, &subject).await?;
        if !stale.is_empty() {
            tx.rollback().await.map_err(db_err)?;
            if attempt > 0 {
                return Err(NsError::Conflict(format!(
                    "{subject} in namespace '{ns}': the paths around it changed meanwhile; retry"
                )));
            }
            detached(
                "purge",
                purge_stale(pool.clone(), fs.clone(), grant.clone(), stale),
            )
            .await?;
            continue;
        }
        ensure_directories(&mut tx, grant, path, true, by).await?;
        let e = latest(&mut tx, grant, path.as_str())
            .await?
            .ok_or_else(|| internal(format!("directory '{path}' has no row once created")))?;
        tx.commit().await.map_err(db_err)?;
        return Ok(e);
    }
    unreachable!("the loop returns on its second attempt")
}

/// §4.3: the latest row of `path` and, when `path` is a directory, of every
/// path under it, marked deleted in ONE statement; objects kept. Nothing live
/// there is not an error: a delete is idempotent (spec §9, application of
/// R3) and answers an empty list. Returns what it marked, by path.
pub async fn soft_delete(
    pool: &PgPool,
    grant: &Grant,
    path: &NsPath,
) -> Result<Vec<NsEntry>, NsError> {
    need_write(grant)?;
    let mut tx = pool.begin().await.map_err(db_err)?;
    // The chain, as every writer (fix round 1, I1): a move of an ancestor
    // holds that ancestor's key, and a writer under `path` locks `path`.
    xact_lock(&mut tx, &lock_chain(grant, &[path])).await?;
    let rows: Vec<Row> = sqlx::query_as(
        "UPDATE workflow.namespace_files f SET deleted = true \
           FROM (SELECT DISTINCT ON (path) id, deleted FROM workflow.namespace_files \
                  WHERE workspace_id = $1 AND namespace = $2 \
                    AND (path = $3 OR ($4 AND left(path, length($3)) = $3)) \
                  ORDER BY path, revision DESC) latest \
          WHERE f.id = latest.id AND NOT latest.deleted \
          RETURNING f.workspace_id, f.namespace, f.path, f.revision, f.is_directory, \
                    f.size, f.deleted, f.created_at",
    )
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path.as_str())
    .bind(path.is_dir())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut marked = entries(rows)?;
    marked.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    Ok(marked)
}

/// §4.4: refuses a live target (`Conflict`) and a missing source
/// (`NotFound` naming it, application of R3); copies every revision of
/// `from` and of each path under it to `to`, keeping revision numbers and
/// `deleted` flags (`get_stream` -> `put_stream`, never `ObjectStore::copy`:
/// its copy source is not URL-encoded, `crates/egret-storage/src/s3.rs`, and
/// it is capped at 5 GiB per S3 operation), inserting rows as it goes; on any
/// failure purges every copy made, in reverse order, and reports both paths;
/// then creates `to`'s missing parent directories and purges the source
/// (rows, commit, objects).
///
/// A target with ANY history -- a live row, or soft-deleted revisions, at
/// `to`, under it, of the other kind at it, or a file where one of its parent
/// directories goes -- is refused with `Conflict` naming it, and nothing is
/// purged (the user's ruling on Task 3's review I2, 2026-10-05: history stays
/// until `PurgeFiles`).
///
/// **A move runs to completion once it has started** (fix round 1, I3): its
/// work runs on a detached task, so dropping this future does not stop it
/// part-way; the source is then purged or the copies rolled back as if the
/// caller had waited.
///
/// Holds the session locks of both paths' chains throughout. `from` and `to`
/// are the same kind (callers parse `to` as `from` is); a directory cannot
/// move under itself. Returns each `(source, copy)` pair, in copy order.
pub async fn move_path(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    from: &NsPath,
    to: &NsPath,
    by: CreatedBy,
) -> Result<Vec<(NsEntry, NsEntry)>, NsError> {
    need_write(grant)?;
    if from.is_dir() != to.is_dir() {
        return Err(internal(format!(
            "egret moved '{from}' to '{to}', which is not the same kind of path"
        )));
    }
    if from == to || (from.is_dir() && to.as_str().starts_with(from.as_str())) {
        return Err(NsError::Author(format!(
            "'{from}' cannot be moved to '{to}' in namespace '{}': a path cannot be moved onto \
             or under itself",
            grant.namespace()
        )));
    }
    let (pool, fs, grant) = (pool.clone(), fs.clone(), grant.clone());
    let (from, to) = (from.clone(), to.clone());
    detached("move", async move {
        let keys = session_keys(&pool, &lock_chain(&grant, &[&from, &to])).await?;
        let mut session = LockedSession::acquire(&pool, &keys)
            .await
            .map_err(NsError::Db)?;
        let result = move_locked(session.conn(), &fs, &grant, &from, &to, by).await;
        release(session, "move").await;
        result
    })
    .await
}

async fn move_locked(
    conn: &mut PgConnection,
    fs: &FileStore,
    grant: &Grant,
    from: &NsPath,
    to: &NsPath,
    by: CreatedBy,
) -> Result<Vec<(NsEntry, NsEntry)>, NsError> {
    let ns = grant.namespace();
    let cap = fs.caps.max_namespace_files;
    let subject = format!("'{from}' cannot be moved to '{to}'");
    if !latest(conn, grant, from.as_str())
        .await?
        .is_some_and(|l| !l.deleted)
    {
        return Err(not_found(grant, from));
    }
    let what = format!("the move of '{from}' in namespace '{ns}'");
    let sources = capped(
        rows_at_or_under(conn, grant, from.as_str(), cap).await?,
        cap,
        &what,
    )?;
    // Each copy's path, before anything is written: a path too long is the
    // author's, and nothing has been touched.
    let mut plan = Vec::with_capacity(sources.len());
    for src in sources {
        let tail = &src.path.as_str()[from.as_str().len()..];
        let raw = format!("{}{tail}", to.as_str());
        let dst = if src.is_directory {
            NsPath::parse_dir(&raw)
        } else {
            NsPath::parse_file(&raw)
        }
        .map_err(|e| NsError::Author(format!("{subject} in namespace '{ns}': {e}")))?;
        plan.push((src, dst));
    }

    // The target: no row at all -- live or soft-deleted history -- at `to`,
    // under it (every copy's path is), or of the other kind at it (I2).
    let mut in_the_way = rows_at_or_under(conn, grant, to.as_str(), 1).await?;
    in_the_way.extend(rows_at_or_under(conn, grant, &other_kind(to), 1).await?);
    if let Some(first) = in_the_way.first() {
        // Its LATEST row says whether it is live or only history.
        let e = latest(conn, grant, first.path.as_str())
            .await?
            .unwrap_or_else(|| first.clone());
        let what = if e.deleted {
            "holds the history of a deleted path; purge it first"
        } else {
            "already exists"
        };
        return Err(NsError::Conflict(format!(
            "{subject} in namespace '{ns}': '{}' {what}; nothing was moved",
            e.path
        )));
    }
    // A file where one of `to`'s parent directories goes, live or deleted.
    for parent in to.parents() {
        if let Some(file) = latest(conn, grant, parent.lock_path()).await? {
            let what = if file.deleted {
                "a deleted file whose history is kept"
            } else {
                "a file"
            };
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': its parent '{}' is {what}; nothing was moved",
                file.path
            )));
        }
    }

    let mut copies: Vec<(NsEntry, NsEntry)> = Vec::with_capacity(plan.len());
    let mut failure = None;
    for (src, dst) in plan {
        match copy_one(conn, fs, grant, &src, &dst, by).await {
            Ok(copy) => copies.push((src, copy)),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    if let Some(e) = failure {
        // Every copy made, the last first; rows then objects (§8.4).
        let made: Vec<(NsPath, u32)> = copies
            .iter()
            .rev()
            .map(|(_, c)| (c.path.clone(), c.revision))
            .collect();
        if let Err(undo) = purge_locked(conn, fs, grant, &made).await {
            tracing::error!(error = %undo, from = %from, to = %to, "a failed namespace file move could not remove its copies");
        }
        let why = format!("{subject} in namespace '{ns}'; nothing was moved");
        return Err(match e {
            NsError::Author(m) => NsError::Author(format!("{why}: {m}")),
            NsError::NotFound(m) => NsError::Author(format!("{why}: {m}")),
            NsError::Conflict(m) => NsError::Conflict(format!("{why}: {m}")),
            NsError::TooLarge(m) => NsError::TooLarge(format!("{why}: {m}")),
            NsError::Store(m) => NsError::Store(format!("{why}: {m}")),
            NsError::Db(d) => internal(format!("{why}: {d}")),
        });
    }
    // Only now, so a failed move leaves no parent row behind (fix round 1, M5).
    ensure_directories(conn, grant, to, false, by).await?;
    let sources: Vec<(NsPath, u32)> = copies
        .iter()
        .map(|(s, _)| (s.path.clone(), s.revision))
        .collect();
    purge_locked(conn, fs, grant, &sources).await?;
    Ok(copies)
}

/// One revision of a move: its object streamed to the copy's key (a file),
/// then its row inserted. A copy whose row could not be inserted has its
/// object removed again.
async fn copy_one(
    conn: &mut PgConnection,
    fs: &FileStore,
    grant: &Grant,
    src: &NsEntry,
    dst: &NsPath,
    by: CreatedBy,
) -> Result<NsEntry, NsError> {
    if src.is_directory {
        return insert_row(conn, grant, dst, 1, 0, src.deleted, by).await;
    }
    let from_key = fs.namespace_object_uri(grant, &src.path, src.revision);
    let to_key = fs.namespace_object_uri(grant, dst, src.revision);
    let mut body = match fs.store.get_stream(&from_key).await {
        Ok(r) => r,
        Err(egret_storage::StorageError::NotFound(_)) => {
            return Err(NsError::Author(format!(
                "revision {} of namespace file '{}' is recorded but its contents are gone from \
                 internal storage",
                src.revision, src.path
            )))
        }
        Err(e) => return Err(store_failed(grant, &src.path, "read", &from_key, &e)),
    };
    let size = fs
        .store
        .put_stream(&to_key, &mut body, Some(src.size))
        .await
        .map_err(|e| store_failed(grant, dst, "stored", &to_key, &e))?;
    match insert_row(conn, grant, dst, src.revision, size, src.deleted, by).await {
        Ok(e) => Ok(e),
        Err(e) => {
            if let Err(d) = fs.store.delete(&to_key).await {
                tracing::warn!(key = %to_key, error = %d, "a moved namespace file's copy could not be deleted: it is an orphan");
            }
            Err(e)
        }
    }
}

/// Hard delete of the given revisions of one path (and, when `path` is a
/// directory, of nothing under it: callers pass files). Session-locked on
/// `path`'s chain (its parents and itself, as every writer; fix round 1, I1):
/// delete rows RETURNING, commit, delete objects, unlock. Runs [`detached`]:
/// once started, a purge deletes the objects of the rows it deleted even if
/// the caller goes away (I3). A
/// crash leaves orphan objects, never a row naming nothing (§8.4). Requires
/// `Access::Purge` or `Access::Write` (the API's DELETE never calls it; move
/// does). Returns how many revisions were purged (a revision with no row is
/// not counted).
pub async fn purge_revisions(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    revisions: &[u32],
) -> Result<u64, NsError> {
    if require(grant, Access::Purge).is_err() {
        need_write(grant)?;
    }
    let (pool, fs, grant) = (pool.clone(), fs.clone(), grant.clone());
    let targets: Vec<(NsPath, u32)> = revisions.iter().map(|r| (path.clone(), *r)).collect();
    let path = path.clone();
    detached("purge", async move {
        let keys = session_keys(&pool, &lock_chain(&grant, &[&path])).await?;
        let mut session = LockedSession::acquire(&pool, &keys)
            .await
            .map_err(NsError::Db)?;
        let result = purge_locked(session.conn(), &fs, &grant, &targets).await;
        release(session, "purge").await;
        result
    })
    .await
}
