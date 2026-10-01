//! W9a: internal storage -- the files an execution writes and reads
//! (`docs/en/specs/2026-10-01-workflow-w9a-internal-storage-design.md`).
//!
//! - [`uri`]: `kestra://` URIs, Kestra's layout (spec §3).
//! - [`index`]: the readable set and [`index::resolve_readable_many`], **the
//!   one function that decides whether a URI is readable** (spec §4.4).
//! - [`scope`]: the render snapshot the four file functions read through
//!   (spec §7.2), resolved before every render.
//! - [`transfer`]: the streamed ustar archive that carries files into a task
//!   container, and the reader that takes them back out (spec §5).
//! - [`collect`]: files out of a task container, streamed into the store
//!   under the task run's prefix (spec §5, §6.2).
//! - this module: the [`FileStore`] (the object store, its bucket, prefix and
//!   caps), the process-wide slot it is installed in, and the index writers.
//!
//! # Why a process-wide store, and what does NOT go through it
//!
//! `read()` runs inside `settle`, reached from about eight entry points none of
//! which holds a store handle (spec §4.6). So the **object store** is installed
//! once by `egret-bin` ([`install`]) and read back with [`current`]. **Index
//! lookups never use it**: they run on the connection the caller holds, so a
//! lookup inside a transaction sees that transaction's rows and no pool
//! outlives a test's runtime. Every function here that decides anything takes
//! the store or the connection as a parameter; only wiring points call
//! [`current`].

pub mod collect;
pub mod index;
pub mod scope;
pub mod transfer;
pub mod uri;

use std::sync::{Arc, OnceLock};

use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

pub use index::{
    loop_family, loop_root, readable_executions, resolve_readable, resolve_readable_many,
};

/// The size caps (spec §4.5, §7.2). Defaults are `egret-config`'s
/// [`egret_config::WorkflowStorageConfig::default`], so the two cannot differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub max_file_bytes: u64,
    pub max_task_bytes: u64,
    pub max_read_bytes: u64,
}

impl From<&egret_config::WorkflowStorageConfig> for Caps {
    fn from(c: &egret_config::WorkflowStorageConfig) -> Self {
        Self {
            max_file_bytes: c.max_file_bytes,
            max_task_bytes: c.max_task_bytes,
            max_read_bytes: c.max_read_bytes,
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Self::from(&egret_config::WorkflowStorageConfig::default())
    }
}

/// The object store an execution's files live in, and where in it.
pub struct FileStore {
    pub store: Arc<dyn egret_storage::ObjectStore>,
    pub bucket: String,
    /// `workflow.storage.prefix`, `workflow/` by default; the key is
    /// `{prefix}{workspace}/{path of the URI}`.
    pub prefix: String,
    pub caps: Caps,
    /// `workflow.storage_retention_days`; `0` keeps files for ever.
    pub retention_days: u32,
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("caps", &self.caps)
            .finish_non_exhaustive()
    }
}

impl FileStore {
    /// `s3://{bucket}/{prefix}{workspace}{path of uri}` — the workspace is the caller's, never the URI's.
    ///
    /// The error names the URI and why it does not parse.
    pub fn object_uri(&self, workspace: Uuid, uri: &str) -> std::result::Result<String, String> {
        let parsed = uri::parse(uri).map_err(|reason| format!("{uri}: {reason}"))?;
        let canonical = parsed.to_uri();
        let path = &canonical[uri::SCHEME_PREFIX.len()..];
        Ok(format!(
            "s3://{}/{}{}/{path}",
            self.bucket,
            self.prefix,
            workspace.hyphenated()
        ))
    }

    /// `s3://{bucket}/{prefix}{workspace}/{ns}/{flow-slug}/executions/{exec}/`,
    /// ending with `/` as `ObjectStore::delete_prefix` requires.
    pub fn execution_prefix(
        &self,
        workspace: Uuid,
        namespace: &str,
        flow_id: &str,
        execution: Uuid,
    ) -> String {
        format!(
            "s3://{}/{}{}/{}/{}/executions/{}/",
            self.bucket,
            self.prefix,
            workspace.hyphenated(),
            uri::namespace_as_path(namespace),
            uri::slugify_str(flow_id),
            execution.hyphenated()
        )
    }
}

static INSTALLED: OnceLock<Arc<FileStore>> = OnceLock::new();

/// Process-wide, set once by egret-bin. See "Why a process-wide store" above.
///
/// A second call returns the store it was given back, and the first stays.
pub fn install(fs: Arc<FileStore>) -> std::result::Result<(), Arc<FileStore>> {
    INSTALLED.set(fs)
}

/// The installed store, `None` when this process has none (the four file
/// functions, FILE inputs and `outputFiles` then fail naming `[storage]`).
pub fn current() -> Option<Arc<FileStore>> {
    INSTALLED.get().cloned()
}

/// Idempotent: MemStore, default caps. Returns whatever store is installed --
/// the first caller in a test binary installs the `MemStore`.
#[cfg(any(test, feature = "test-util"))]
pub fn install_mem_for_tests() -> Arc<FileStore> {
    INSTALLED
        .get_or_init(|| {
            Arc::new(FileStore {
                store: Arc::new(egret_storage::MemStore::new()),
                bucket: "egret-test".to_string(),
                prefix: egret_config::WorkflowStorageConfig::default().prefix,
                caps: Caps::default(),
                retention_days: 30,
            })
        })
        .clone()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFile {
    pub uri: String,
    pub execution_id: Uuid,
    pub task_run_id: Option<Uuid>,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    #[error("{uri} is not a storage URI egret can read: {reason}")]
    Malformed { uri: String, reason: String },
    /// Its execution is not in this execution's readable set (loop family, trigger execution, replay chain).
    /// Deliberately one variant: the message never says whether the file exists.
    #[error(
        "{uri} belongs to an execution this flow may not read: a flow reads only its own \
         execution's files, its loop family's (its loop root and every iteration under it), \
         its trigger execution's and, for a replay, its source's"
    )]
    NotReadable { uri: String },
    /// Its execution is readable and no such file was ever committed.
    #[error("{uri} does not exist")]
    Missing { uri: String },
}

/// A readable file whose bytes are not UTF-8, refused by `read()` (spec §7.2;
/// `w9a-storage.json` `file_functions.not_utf8.read`, the token `NotUtf8`).
/// Kestra decodes lossily (U+FFFD); egret refuses, naming the URI.
///
/// **Not an [`Unreadable`] variant**, on purpose: `Unreadable` is the answer of
/// the readability decision ([`resolve_readable_many`]), made from the index
/// alone, and that decision cannot know a file's bytes. This is a property of
/// the contents, found by the reader after the file was resolved readable --
/// `fileSize`, `fileExists` and `isFileEmpty` on the same URI succeed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{uri} is not UTF-8 text: read() returns a string and does not decode other bytes")]
pub struct NotUtf8 {
    pub uri: String,
}

/// `read()`'s decoding: the bytes as a `String`, or [`NotUtf8`] naming `uri`.
pub fn decode_utf8(uri: &str, bytes: Vec<u8>) -> std::result::Result<String, NotUtf8> {
    String::from_utf8(bytes).map_err(|_| NotUtf8 {
        uri: uri.to_string(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFile {
    pub uri: String,
    pub size: u64,
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// Index the files a task run wrote, in the caller's transaction (the one that
/// commits the run's outcome, spec §4.3).
///
/// Each file's `execution_id` is the `{exec}` of its URI, and it must be the
/// task run's own execution -- the writing execution, an iteration's own id
/// inside a `Loop` (spec §3) -- with a `tasks/{..}/{task_run}/` tail. A
/// mismatch is `Error::Internal`: egret built a wrong URI. A retried task run
/// writes the same deterministic URIs, so a conflict updates the row.
pub async fn index_task_files(
    conn: &mut PgConnection,
    workspace: Uuid,
    task_run: Uuid,
    files: &[StoredFile],
) -> Result<()> {
    // Fix round 1, M5: every outcome commit comes here, almost always with
    // nothing; nothing to index costs no statement on its transaction.
    if files.is_empty() {
        return Ok(());
    }
    let execution: Option<Uuid> = sqlx::query_scalar(
        "SELECT tr.execution_id FROM workflow.task_runs tr \
           JOIN workflow.executions e ON e.id = tr.execution_id \
          WHERE tr.id = $1 AND e.workspace_id = $2",
    )
    .bind(task_run)
    .bind(workspace)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let Some(execution) = execution else {
        return Err(Error::Internal(anyhow::anyhow!(
            "task run {task_run} is not in workspace {workspace}; its files cannot be indexed"
        )));
    };
    let want_run = task_run.hyphenated().to_string();
    for f in files {
        let parsed = uri::parse(&f.uri)
            .map_err(|r| Error::Internal(anyhow::anyhow!("egret built {}: {r}", f.uri)))?;
        let run_matches =
            matches!(&parsed.tail, uri::Tail::Task { task_run, .. } if *task_run == want_run);
        if parsed.execution != execution || !run_matches {
            return Err(Error::Internal(anyhow::anyhow!(
                "egret built {} for task run {task_run} of execution {execution}: \
                 the URI names another execution or task run",
                f.uri
            )));
        }
    }
    insert_rows(conn, workspace, execution, Some(task_run), files).await
}

/// Index an execution's input files (no task run), in the transaction that
/// commits the execution row (spec §7.1). Each URI must be an
/// `inputs/{input}/{file}` URI of `execution`; anything else is
/// `Error::Internal`.
pub async fn index_input_files(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
    files: &[StoredFile],
) -> Result<()> {
    for f in files {
        let parsed = uri::parse(&f.uri)
            .map_err(|r| Error::Internal(anyhow::anyhow!("egret built {}: {r}", f.uri)))?;
        if parsed.execution != execution || !matches!(parsed.tail, uri::Tail::Input { .. }) {
            return Err(Error::Internal(anyhow::anyhow!(
                "egret built {} as an input file of execution {execution}: \
                 the URI names another execution or is not an input",
                f.uri
            )));
        }
    }
    insert_rows(conn, workspace, execution, None, files).await
}

async fn insert_rows(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
    task_run: Option<Uuid>,
    files: &[StoredFile],
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let uris: Vec<&str> = files.iter().map(|f| f.uri.as_str()).collect();
    let sizes = files
        .iter()
        .map(|f| {
            i64::try_from(f.size).map_err(|_| {
                Error::Internal(anyhow::anyhow!(
                    "{}: size {} overflows bigint",
                    f.uri,
                    f.size
                ))
            })
        })
        .collect::<Result<Vec<i64>>>()?;
    sqlx::query(
        "INSERT INTO workflow.execution_files \
           (workspace_id, execution_id, task_run_id, uri, size) \
         SELECT $1, $2, $3, u.uri, u.size FROM UNNEST($4::text[], $5::bigint[]) AS u(uri, size) \
         ON CONFLICT (workspace_id, uri) DO UPDATE \
           SET size = EXCLUDED.size, task_run_id = EXCLUDED.task_run_id, created_at = now()",
    )
    .bind(workspace)
    .bind(execution)
    .bind(task_run)
    .bind(&uris)
    .bind(&sizes)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_readable_names_the_loop_family_trigger_and_replay_and_never_existence() {
        let msg = Unreadable::NotReadable {
            uri: "kestra:///a/f/executions/x/y".into(),
        }
        .to_string();
        for part in [
            "kestra:///a/f/executions/x/y",
            "loop family",
            "trigger execution",
            "replay",
        ] {
            assert!(msg.contains(part), "{msg:?} names {part:?}");
        }
        assert!(
            !msg.contains("exist"),
            "{msg:?} must not say whether it exists"
        );
    }

    #[test]
    fn non_utf8_is_refused_naming_the_uri_and_utf8_is_not() {
        let uri = "kestra:///a/f/executions/x/y";
        // `w9a-storage.json` `file_functions.not_utf8`: Kestra rendered three
        // U+FFFD for these bytes; egret refuses.
        let err = decode_utf8(uri, vec![0xff, 0xfe, 0xfd]).unwrap_err();
        assert_eq!(err, NotUtf8 { uri: uri.into() });
        assert!(err.to_string().starts_with(uri), "{err}");
        assert_eq!(decode_utf8(uri, "café".into()).unwrap(), "café");
    }

    #[test]
    fn default_caps_are_the_documented_defaults() {
        assert_eq!(
            Caps::default(),
            Caps {
                max_file_bytes: 1 << 30,
                max_task_bytes: 5 << 30,
                max_read_bytes: 10 << 20,
            }
        );
    }
}
