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

/// `id`, then [`Row`] ([`file_rows_page`]).
type RowWithId = (
    i64,
    Uuid,
    String,
    String,
    i32,
    bool,
    i64,
    bool,
    DateTime<Utc>,
);

/// [`Row`], then `created_by_task_run` ([`latest_written`]).
type RowWithTaskRun = (
    Uuid,
    String,
    String,
    i32,
    bool,
    i64,
    bool,
    DateTime<Utc>,
    Option<Uuid>,
);

/// egret's bug, not the author's and not an outage: a 500.
fn internal(message: String) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::anyhow!(message)))
}

/// A purge function handed a grant that is not a purge grant: egret's own
/// bug. NOT `NsError::Db`, which the handler treats as an outage and
/// re-claims forever; an `Author` error fails the task, loudly and once.
fn wrong_grant(e: egret_core::Error) -> NsError {
    NsError::Author(format!("internal error (an egret bug, not retried): {e}"))
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

/// The namespaces of `workspace` that hold at least one row and are `root`
/// or one of its children on a dot boundary (`root + "."`-prefixed; `a.bc`
/// is not a child of `a.b`), in byte order: `PurgeFiles`' candidates (W9b
/// Task 7; spec §8.4, the user's ruling R2 and the coordinator's ruling B).
/// Fetches at most `cap + 1` names; the `+1` is the caller's refusal.
pub async fn namespaces_with_files(
    conn: &mut PgConnection,
    workspace: Uuid,
    root: &str,
    cap: u64,
) -> Result<Vec<String>, NsError> {
    sqlx::query_scalar(
        "SELECT DISTINCT namespace COLLATE \"C\" AS namespace FROM workflow.namespace_files \
          WHERE workspace_id = $1 \
            AND (namespace = $2 OR left(namespace, length($2) + 1) = $2 || '.') \
          ORDER BY 1 LIMIT $3",
    )
    .bind(workspace)
    .bind(root)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// One page of the grant's namespace's FILE rows -- every revision, live or
/// soft-deleted -- with each row's `id`, by path (byte order) then revision,
/// strictly after `after` (`(path, revision)`; `None` = from the start):
/// `PurgeFiles`' keyset listing (W9b Task 7, fix round 1 I3). Directories
/// are not listed ([`purgeable_directories_page`]). Fetches at most
/// `cap + 1` rows; the caller pages on.
pub async fn file_rows_page(
    conn: &mut PgConnection,
    grant: &Grant,
    after: Option<(&NsPath, u32)>,
    cap: u64,
) -> Result<Vec<(i64, NsEntry)>, NsError> {
    let (after_path, after_revision) = match after {
        Some((p, r)) => (
            Some(p.as_str().to_string()),
            i32::try_from(r).unwrap_or(i32::MAX),
        ),
        None => (None, 0),
    };
    let rows: Vec<RowWithId> = sqlx::query_as(concat!(
        "SELECT id, ",
        entry_cols!(),
        " FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND NOT is_directory \
            AND ($3::text IS NULL OR path COLLATE \"C\" > $3::text COLLATE \"C\" \
                 OR (path = $3::text AND revision > $4)) \
          ORDER BY path COLLATE \"C\", revision LIMIT $5"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(after_path)
    .bind(after_revision)
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|(id, w, n, p, r, d, s, del, at)| Ok((id, entry((w, n, p, r, d, s, del, at))?)))
        .collect()
}

/// One page of the grant's namespace's directory rows a purge may remove --
/// soft-deleted ones, and with `live_too` live ones as well -- in DESCENDING
/// byte order (a directory's children sort after it, so a child comes
/// first), strictly before `before`. At most `cap + 1` rows. Whether one is
/// removed is decided under its lock ([`purge_empty_directory`]).
pub async fn purgeable_directories_page(
    conn: &mut PgConnection,
    grant: &Grant,
    before: Option<&NsPath>,
    live_too: bool,
    cap: u64,
) -> Result<Vec<NsEntry>, NsError> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        " FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND is_directory AND (deleted OR $3) \
            AND ($4::text IS NULL OR path COLLATE \"C\" < $4::text COLLATE \"C\") \
          ORDER BY path COLLATE \"C\" DESC LIMIT $5"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(live_too)
    .bind(before.map(|p| p.as_str().to_string()))
    .bind(limit(cap))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    entries(rows)
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
// (`pg_advisory_xact_lock`), held from the revision's choice to the row
// insert, so that no second writer computes the same revision. `put` takes
// them only once its body is in a staging object (the user's ruling of
// 2026-10-06): the lock window is server-paced, never a client's. `purge_revisions` and
// `move_path` take the same keys as SESSION locks ([`LockedSession`]) held
// across "delete rows, commit, delete objects": a transaction lock would be
// released by that commit, and a writer could then take the purged revision
// number again and upload to its key, which the purge would then delete -- a
// row naming nothing (plan item 7). The two kinds of lock on one key conflict.

/// How a 409 over kept history names the one tool that removes it (W9b Task
/// 8): the namespace file API has no purge route, so "purge it first" alone
/// would send an API caller looking for one. Worded for both readers (fix
/// round 1, M-6): a task's author and an API caller see the same text.
pub const PURGE_FILES_IS_THE_TOOL: &str =
    "only a flow's io.kestra.plugin.core.namespace.PurgeFiles task, run from this namespace \
     or a parent of it, removes history; the namespace file API has no purge route";

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
    /// A `CreatedBy::TaskRun` write whose task run already committed the
    /// path's latest revision (an earlier attempt, re-claimed after an
    /// infrastructure failure): nothing is written (Task 6 fix round 1, I1,
    /// judged under the path's lock since Task 8 fix round 1).
    Already(NsEntry),
}

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

/// `(created_by_user, created_by_execution, created_by_task_run)`.
fn writer(by: CreatedBy) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match by {
        CreatedBy::User(u) => (Some(u), None, None),
        CreatedBy::Execution(e) => (None, Some(e), None),
        CreatedBy::TaskRun {
            execution,
            task_run,
        } => (None, Some(execution), Some(task_run)),
    }
}

/// The latest row of `path` (live or not) and the task run that wrote it
/// (`None` for a user's or a bare execution's write). `UploadFiles` reads it
/// before each write: a latest revision its own task run wrote, of the
/// source's size, is a write an earlier attempt of that run already
/// committed (W9b Task 6 fix round 1, I1).
pub async fn latest_written(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
) -> Result<Option<(NsEntry, Option<Uuid>)>, NsError> {
    let row: Option<RowWithTaskRun> = sqlx::query_as(concat!(
        "SELECT ",
        entry_cols!(),
        ", created_by_task_run FROM workflow.namespace_files \
              WHERE workspace_id = $1 AND namespace = $2 AND path = $3 \
              ORDER BY revision DESC LIMIT 1"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(path.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    row.map(|(w, n, p, r, d, s, del, at, run)| Ok((entry((w, n, p, r, d, s, del, at))?, run)))
        .transpose()
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
/// locks. A write never purges history implicitly, in either direction (the
/// user's ruling, 2026-10-05, which extends the I2 ruling on a move to every
/// write):
/// - a live row of the other kind at `path` is `Conflict`;
/// - a soft-deleted FILE of the other kind at `path` (it always has
///   revisions) is `Conflict` naming it: `PurgeFiles` it first;
/// - a soft-deleted DIRECTORY of the other kind is `Conflict` when paths
///   under it still hold history, and is removed here when it is alone (a
///   directory has no object and no revisions, so its row is all there is);
/// - a FILE where one of its parent directories goes, live or soft-deleted,
///   is `Conflict` naming it.
///
/// `subject` opens each message ("namespace file '/a' cannot be written").
async fn check_kinds(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
    subject: &str,
) -> Result<(), NsError> {
    let ns = grant.namespace();
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
        if !other.is_directory {
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': '{}' is a deleted file whose history is kept; \
                 purge it first ({PURGE_FILES_IS_THE_TOOL})",
                other.path
            )));
        }
        let under = rows_at_or_under(conn, grant, other.path.as_str(), 1).await?;
        if under.len() > 1 {
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': '{}' is a deleted directory whose files' history \
                 is kept; purge them first ({PURGE_FILES_IS_THE_TOOL})",
                other.path
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
    }
    for parent in path.parents() {
        if let Some(file) = latest(conn, grant, parent.lock_path()).await? {
            let what = if file.deleted {
                format!(
                    "a deleted file whose history is kept; purge it first \
                     ({PURGE_FILES_IS_THE_TOOL})"
                )
            } else {
                "a file".to_string()
            };
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': its parent '{}' is {what}",
                file.path
            )));
        }
    }
    Ok(())
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
    let (user, execution, task_run) = writer(by);
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, \
            created_by_user, created_by_execution, created_by_task_run) \
         SELECT $1, $2, dir, 1, true, 0, $4, $5, $6 FROM unnest($3::text[]) AS dir \
         ON CONFLICT ON CONSTRAINT workflow_namespace_files_revision_key \
           DO UPDATE SET deleted = false WHERE workflow.namespace_files.deleted",
    )
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(&dirs)
    .bind(user)
    .bind(execution)
    .bind(task_run)
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
    let (user, execution, task_run) = writer(by);
    let revision = i32::try_from(revision)
        .map_err(|_| internal(format!("revision {revision} of '{path}' is out of range")))?;
    let size = i64::try_from(size)
        .map_err(|_| internal(format!("size {size} of '{path}' is out of range")))?;
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted, \
            created_by_user, created_by_execution, created_by_task_run) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ",
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
    .bind(task_run)
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

/// The most bytes one namespace file may hold, and the setting that says
/// so: `workflow.storage.max_file_bytes`, or -- when that is configured above
/// it -- [`egret_storage::MAX_SINGLE_COPY_BYTES`] (5 GiB), the most one
/// server-side copy moves from the staging object to the revision's key
/// (W9b Task 8 fix round 1: refused, not copied in parts).
pub fn namespace_file_cap(caps: &crate::storage::Caps) -> (u64, &'static str) {
    if caps.max_file_bytes <= egret_storage::MAX_SINGLE_COPY_BYTES {
        (caps.max_file_bytes, "workflow.storage.max_file_bytes")
    } else {
        (
            egret_storage::MAX_SINGLE_COPY_BYTES,
            "the most one internal-storage copy moves, which is below \
             workflow.storage.max_file_bytes",
        )
    }
}

/// §4.2, the user's ruling of 2026-10-06 (W9b Task 8 fix round 1, I-1):
/// **the body is uploaded before any lock is taken.**
///
/// 1. A staging row, then the body streamed to its staging object
///    ([`staging`](super::staging)) under the file cap
///    ([`namespace_file_cap`]) -- holding NO pool connection and NO advisory
///    lock, so a caller that sends slowly (a client) holds nothing another
///    request needs.
/// 2. Then ONE short, server-paced transaction: lock the path's chain ->
///    a task run's own earlier write is [`Written::Already`] (Task 6's
///    re-claim, judged under the lock) -> [`check_kinds`] -> the conflict rule
///    -> revision = max + 1 (1 when none) -> server-side `copy` of the
///    staging object to the revision's key -> `head` (its size must be the
///    staged size) -> insert the row -> insert missing parent directory rows
///    -> commit.
/// 3. The staging object and its row are deleted, whatever happened
///    (best-effort; the staging sweep takes what is left). Steps 2 and 3 run
///    on a detached task ([`detached`]): once the body is staged, dropping
///    the caller's future no longer leaves a staged object behind.
///
/// A row of the other kind, or a file where a parent goes, is `Conflict` when
/// it is live or still holds history: a write never purges history
/// implicitly ([`check_kinds`]; the user's ruling, 2026-10-05). Commits
/// immediately: namespace files are not in any execution's transaction (§6).
///
/// A failed copy rolls the transaction back and leaves at most an unindexed
/// object at the revision's key, which the next write of that revision
/// overwrites. A body that fails to read (not the store) is `NsError::Store`
/// too: the caller's source is internal storage or a request body, neither
/// the author's; the API tells its own body's failures apart.
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
    let staged = super::staging::record(pool, fs, grant.workspace()).await?;
    let size = match stage_body(fs, grant, path, body, &staged.uri).await {
        Ok(size) => size,
        Err(e) => {
            super::staging::forget(pool, fs, &staged).await;
            return Err(e);
        }
    };
    // The body is staged (re-review m-1, 2026-10-06): the commit and the
    // staging object's deletion run on a detached task, as `move_path` does,
    // so a caller whose future is dropped from here on (a client that goes
    // away, a task timeout) still gets its write committed or refused and its
    // staging object and row deleted; only the answer is lost. A future
    // dropped while the body is still being read leaves at most the row (an
    // S3 multipart upload is aborted on drop) and whatever the last
    // single-request upload landed, which the staging sweep takes.
    let (pool, fs, grant, path) = (pool.clone(), fs.clone(), grant.clone(), path.clone());
    detached("write", async move {
        let result =
            commit_staged(&pool, &fs, &grant, &path, &staged.uri, size, conflict, by).await;
        super::staging::forget(&pool, &fs, &staged).await;
        result
    })
    .await
}

/// Step 1 of [`put`]: `body` to the staging object under the cap; its size.
/// Holds nothing of the database.
async fn stage_body(
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    staging: &str,
) -> Result<u64, NsError> {
    let (cap, cap_name) = namespace_file_cap(&fs.caps);
    let mut guarded = Guarded {
        inner: body,
        cap,
        read: 0,
        over: false,
        failed: None,
    };
    let put = fs.store.put_stream(staging, &mut guarded, None).await;
    if guarded.over {
        return Err(NsError::TooLarge(format!(
            "namespace file '{path}' in namespace '{}' is over the {cap} bytes one file \
             may be ({cap_name})",
            grant.namespace()
        )));
    }
    if let Some(e) = guarded.failed {
        return Err(store_failed(
            grant,
            path,
            "read while it was being stored",
            staging,
            &e,
        ));
    }
    put.map_err(|e| store_failed(grant, path, "stored", staging, &e))
}

/// Step 2 of [`put`]: the short transaction over a staged body of `size`
/// bytes.
#[allow(clippy::too_many_arguments)]
async fn commit_staged(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    staging: &str,
    size: u64,
    conflict: Conflict,
    by: CreatedBy,
) -> Result<Written, NsError> {
    let ns = grant.namespace();
    let subject = format!("namespace file '{path}' cannot be written");
    let mut tx = pool.begin().await.map_err(db_err)?;
    xact_lock(&mut tx, &lock_chain(grant, &[path])).await?;
    // Task 6 (fix round 1, I1), under the lock: a re-claimed task run whose
    // earlier attempt committed this path's latest revision does not write
    // it again, under every conflict mode -- even when that attempt is still
    // running beside this one.
    let latest_now = latest_written(&mut tx, grant, path).await?;
    if let CreatedBy::TaskRun { task_run, .. } = by {
        if let Some((e, Some(run))) = &latest_now {
            if *run == task_run && !e.deleted && !e.is_directory {
                return Ok(Written::Already(e.clone()));
            }
        }
    }
    check_kinds(&mut tx, grant, path, &subject).await?;
    let current = latest_now.map(|(e, _)| e);
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
    fs.store
        .copy(staging, &key)
        .await
        .map_err(|e| store_failed(grant, path, "stored", &key, &e))?;
    match fs.store.head(&key).await {
        Ok(Some(n)) if n == size => {}
        Ok(Some(n)) => {
            return Err(store_failed(
                grant,
                path,
                "stored",
                &key,
                &format!("copied {n} bytes of a {size}-byte staging object"),
            ))
        }
        Ok(None) => {
            return Err(store_failed(
                grant,
                path,
                "stored",
                &key,
                &"copied, but head finds no object",
            ))
        }
        Err(e) => return Err(store_failed(grant, path, "stored", &key, &e)),
    }
    let e = insert_row(&mut tx, grant, path, revision, size, false, by).await?;
    ensure_directories(&mut tx, grant, path, false, by).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(Written::Written(e))
}

/// The directory and its missing parents; an existing live directory is Ok
/// (idempotent); a file there or where a parent goes, live or soft-deleted
/// with its history, is `Conflict` ([`check_kinds`]). Unlike Kestra, which
/// makes no row for a parent, every missing parent gets a row (w9b
/// `layout.create_directory`, chosen).
pub async fn create_directory(
    pool: &PgPool,
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
    let subject = format!("cannot create directory '{}'", path.lock_path());
    let mut tx = pool.begin().await.map_err(db_err)?;
    xact_lock(&mut tx, &lock_chain(grant, &[path])).await?;
    check_kinds(&mut tx, grant, path, &subject).await?;
    ensure_directories(&mut tx, grant, path, true, by).await?;
    let e = latest(&mut tx, grant, path.as_str())
        .await?
        .ok_or_else(|| internal(format!("directory '{path}' has no row once created")))?;
    tx.commit().await.map_err(db_err)?;
    Ok(e)
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
    let marked = mark_deleted(&mut tx, grant, path).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(marked)
}

/// `DeleteFiles`' `deleteParentFolder` (W9b Task 7, fix round 1, M1/M2; the
/// user's ruling of 2026-10-06, Kestra's `isDirectoryEmpty`): the directory
/// `dir` is soft-deleted, with every row under it, only when no LIVE FILE
/// remains at any depth beneath it (live empty subdirectories do not keep
/// it). The check and the delete run in ONE transaction holding `dir`'s
/// chain lock, which every writer under `dir` also takes, so a file written
/// concurrently either commits first (and keeps `dir`) or waits until `dir`
/// is deleted (and then revives it). Returns what it marked; empty when
/// `dir` was kept or holds nothing live.
pub async fn soft_delete_if_empty(
    pool: &PgPool,
    grant: &Grant,
    dir: &NsPath,
) -> Result<Vec<NsEntry>, NsError> {
    need_write(grant)?;
    if !dir.is_dir() {
        return Err(internal(format!(
            "egret judged '{dir}' as a directory, which is a file path"
        )));
    }
    let mut tx = pool.begin().await.map_err(db_err)?;
    xact_lock(&mut tx, &lock_chain(grant, &[dir])).await?;
    let holds_a_file: bool = sqlx::query_scalar(concat!(
        "SELECT EXISTS (SELECT 1 FROM (",
        latest_under!(),
        ") latest WHERE NOT deleted AND NOT is_directory)"
    ))
    .bind(grant.workspace())
    .bind(grant.namespace())
    .bind(dir.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    let marked = if holds_a_file {
        Vec::new()
    } else {
        mark_deleted(&mut tx, grant, dir).await?
    };
    tx.commit().await.map_err(db_err)?;
    Ok(marked)
}

/// §4.3's one statement: the latest row of `path` and, for a directory, of
/// every path under it, marked deleted; by path. The caller holds the lock.
async fn mark_deleted(
    conn: &mut PgConnection,
    grant: &Grant,
    path: &NsPath,
) -> Result<Vec<NsEntry>, NsError> {
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
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut marked = entries(rows)?;
    marked.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    Ok(marked)
}

/// §4.4: refuses a live target (`Conflict`) and a missing source
/// (`NotFound` naming it, application of R3); copies every revision of
/// `from` and of each path under it to `to`, keeping revision numbers and
/// `deleted` flags (`get_stream` -> `put_stream`), inserting rows as it goes; on any
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
            format!(
                "holds the history of a deleted path; purge it first ({PURGE_FILES_IS_THE_TOOL})"
            )
        } else {
            "already exists".to_string()
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
                format!(
                    "a deleted file whose history is kept; purge it first \
                     ({PURGE_FILES_IS_THE_TOOL})"
                )
            } else {
                "a file".to_string()
            };
            return Err(NsError::Conflict(format!(
                "{subject} in namespace '{ns}': its parent '{}' is {what}; nothing was moved",
                file.path
            )));
        }
    }

    // The copies, then `to`'s missing parents -- only then, so a failed move
    // leaves no parent row behind (fix round 1, M5). A failure anywhere in
    // this block, the parents' insert included (fix round 2, M-R2), takes
    // the same way out: every copy made is purged.
    let mut copies: Vec<(NsEntry, NsEntry)> = Vec::with_capacity(plan.len());
    let made = async {
        for (src, dst) in plan {
            let copy = copy_one(conn, fs, grant, &src, &dst, by).await?;
            copies.push((src, copy));
        }
        ensure_directories(conn, grant, to, false, by).await
    }
    .await;
    if let Err(e) = made {
        // Every copy made, the last first; rows then objects (§8.4).
        let undo: Vec<(NsPath, u32)> = copies
            .iter()
            .rev()
            .map(|(_, c)| (c.path.clone(), c.revision))
            .collect();
        if let Err(undo) = purge_locked(conn, fs, grant, &undo).await {
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

/// `PurgeFiles`' hard delete of one file path's rows, named by ROW `id`
/// (W9b Task 7, fix round 1, I1): an id is never reused, while a revision
/// NUMBER is -- a path whose rows were all purged, or moved away, starts
/// again at revision 1 -- so a purge planned from an earlier listing removes
/// only the rows it saw, never a newer file that took the same numbers.
/// Objects are deleted only for the rows this call removed. Session-locked
/// on `path`'s chain and [`detached`], as [`purge_revisions`]. `rows` are
/// `(id, revision)`; the revision clause only restates what each id names.
/// Returns how many rows were removed.
pub async fn purge_rows(
    pool: &PgPool,
    fs: &FileStore,
    grant: &Grant,
    path: &NsPath,
    rows: &[(i64, u32)],
) -> Result<u64, NsError> {
    require(grant, Access::Purge).map_err(wrong_grant)?;
    if path.is_dir() {
        return Err(internal(format!(
            "egret purged '{path}' by row as a file, which is a directory"
        )));
    }
    if rows.is_empty() {
        return Ok(0);
    }
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let revisions: Vec<i32> = rows
        .iter()
        .map(|(_, r)| i32::try_from(*r).unwrap_or(i32::MAX))
        .collect();
    let (pool, fs, grant, path) = (pool.clone(), fs.clone(), grant.clone(), path.clone());
    detached("purge", async move {
        let keys = session_keys(&pool, &lock_chain(&grant, &[&path])).await?;
        let mut session = LockedSession::acquire(&pool, &keys)
            .await
            .map_err(NsError::Db)?;
        let result = async {
            let gone: Vec<i32> = sqlx::query_scalar(
                "DELETE FROM workflow.namespace_files f \
                  WHERE f.workspace_id = $1 AND f.namespace = $2 AND f.path = $3 \
                    AND NOT f.is_directory \
                    AND f.id = ANY($4) AND f.revision = ANY($5) \
                  RETURNING f.revision",
            )
            .bind(grant.workspace())
            .bind(grant.namespace())
            .bind(path.as_str())
            .bind(&ids)
            .bind(&revisions)
            .fetch_all(&mut *session.conn())
            .await
            .map_err(db_err)?;
            for revision in &gone {
                let key = fs.namespace_object_uri(&grant, &path, *revision as u32);
                if let Err(e) = fs.store.delete(&key).await {
                    tracing::warn!(key = %key, error = %e, "a purged namespace file's object could not be deleted: it is an orphan");
                }
            }
            Ok(gone.len() as u64)
        }
        .await;
        release(session, "purge").await;
        result
    })
    .await
}

/// `PurgeFiles`' removal of one directory row (W9b Task 7, fix round 1, I2):
/// under `dir`'s chain lock, the row goes only when it is soft-deleted (or,
/// with `live_too`, live) AND no row at all -- live or soft-deleted, file or
/// directory -- remains under it: the condition `put` already uses to remove
/// a lone soft-deleted directory (§4.2 step 4). A directory has no object.
/// [`detached`]. Returns 1 when the row was removed, else 0.
pub async fn purge_empty_directory(
    pool: &PgPool,
    grant: &Grant,
    dir: &NsPath,
    live_too: bool,
) -> Result<u64, NsError> {
    require(grant, Access::Purge).map_err(wrong_grant)?;
    if !dir.is_dir() {
        return Err(internal(format!(
            "egret purged '{dir}' as a directory, which is a file path"
        )));
    }
    let (pool, grant, dir) = (pool.clone(), grant.clone(), dir.clone());
    detached("purge", async move {
        let keys = session_keys(&pool, &lock_chain(&grant, &[&dir])).await?;
        let mut session = LockedSession::acquire(&pool, &keys)
            .await
            .map_err(NsError::Db)?;
        let result = sqlx::query(
            "DELETE FROM workflow.namespace_files f \
              WHERE f.workspace_id = $1 AND f.namespace = $2 AND f.path = $3 \
                AND f.is_directory AND (f.deleted OR $4) \
                AND NOT EXISTS (SELECT 1 FROM workflow.namespace_files u \
                                 WHERE u.workspace_id = $1 AND u.namespace = $2 \
                                   AND left(u.path, length($3)) = $3 AND u.path <> $3)",
        )
        .bind(grant.workspace())
        .bind(grant.namespace())
        .bind(dir.as_str())
        .bind(live_too)
        .execute(&mut *session.conn())
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err);
        release(session, "purge").await;
        result
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::nsfiles::access::Actor;

    /// A grant of the wrong kind is an egret bug: it fails the task
    /// (`Author`), never `Db`, which the handler would re-claim forever.
    #[tokio::test]
    async fn a_wrong_grant_to_a_purge_is_not_an_outage() {
        let pool = PgPool::connect_lazy("postgres://unused@localhost:1/none").unwrap();
        let fs = crate::storage::install_mem_for_tests();
        let write = crate::storage::nsfiles::access::authorize(
            Uuid::new_v4(),
            Actor::Api,
            "a",
            Access::Write,
        )
        .unwrap();
        let file = NsPath::parse_file("/a.txt").unwrap();
        let dir = NsPath::parse_dir("/d/").unwrap();
        let rows = purge_rows(&pool, &fs, &write, &file, &[(1, 1)]).await;
        assert!(matches!(rows, Err(NsError::Author(m)) if m.contains("egret bug")));
        let empty = purge_empty_directory(&pool, &write, &dir, false).await;
        assert!(matches!(empty, Err(NsError::Author(m)) if m.contains("egret bug")));
    }
}
