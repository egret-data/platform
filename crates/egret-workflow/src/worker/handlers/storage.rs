//! `io.kestra.plugin.core.storage.{Write, Size, Delete, PurgeCurrentExecutionFiles}`
//! -- the core storage tasks W9a Task 11 ports (spec §9.3). Task 12 adds the
//! other five.
//!
//! # The shapes are Kestra's, measured
//!
//! Each class's properties (name, `required`, `default`) and outputs are
//! [`DECLARED`], and `tests/storage_tasks.rs` holds that table against
//! `oracle-exec/w9a-storage.json`'s `core_storage`, the plugin schema and one
//! run per class on `kestra/kestra:v2.0.0-rc13`.
//!
//! | class | properties | outputs |
//! |---|---|---|
//! | `Write` (`Write.java`) | `content` (required), `extension` | `uri` |
//! | `Size` (`Size.java`) | `uri` (required) | `size`, bytes |
//! | `Delete` (`Delete.java`) | `uri` (required), `errorOnMissing` (default `false`) | `uri`, `deleted` |
//! | `PurgeCurrentExecutionFiles` | none | `uris` |
//!
//! # Where egret differs, on purpose
//!
//! - **`Write`'s file name is fixed, `content{extension}`** (`.tmp` when there is
//!   no `extension:`, Kestra's `createTempFile` default). Kestra's is a random
//!   temp-file number (`17404172313986282227.txt`, measured); a re-claim of
//!   the same task run has to overwrite its own object (spec §4.3), as
//!   `egret.sql`'s fixed `result.ion` does. The extension is used verbatim, as
//!   Kestra's `createTempFile` suffix is; one holding `/` is refused (Kestra's
//!   `Files.createTempFile` throws for it too).
//! - **`Delete` and `PurgeCurrentExecutionFiles` act on the running
//!   execution's LOOP FAMILY only** -- its loop root and every iteration under
//!   it (spec §4.4 items 1-2, §9.3). A trigger's or a replay source's file is
//!   readable and is never deletable: `Delete` of one fails the task naming
//!   it (`w9a-storage.json` `core_storage.other_execution`, chosen; Kestra OSS
//!   deleted another execution's file).
//! - **The index row goes first, then the object** (spec §9.3). A file exists
//!   for a flow only while its row does (§4.2), so once the row is gone the
//!   file is deleted for every reader, and a store that then fails to delete
//!   the object leaves an ORPHAN -- swept at retention (§10) -- and never a
//!   row naming an object that is gone. The task succeeds and logs the orphan
//!   at `WARN`. The row is deleted in its own transaction, before the run's
//!   outcome commits: a worker fenced after that has still deleted the file,
//!   and its re-claim answers `deleted: false`, as a second `Delete` would.
//! - **`PurgeCurrentExecutionFiles` deletes the objects of the rows it
//!   deleted, one by one**, not each execution's whole prefix: a sibling
//!   iteration may be uploading a file whose row its own commit will insert,
//!   and deleting that object would leave the row dangling. Its `uris` lists
//!   what it deleted the way Kestra's `deleteByPrefix` does -- every file, then
//!   each directory above it up to the execution's, in post-order, measured
//!   (`core_storage.PurgeCurrentExecutionFiles.runs`). Kestra's directory
//!   order is the file system's; egret's is sorted.
//!
//! `Write`, `Delete` and `PurgeCurrentExecutionFiles` take the store as a
//! parameter (spec §4.6); `Delete` and `PurgeCurrentExecutionFiles` also take
//! the pool, because no [`TaskInput`] carries a connection and the index row
//! must be deleted before the object. `Size` answers from the run's render
//! snapshot alone (the index, spec §4.2) and reads no byte.

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_core::{Error, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sqlx::PgPool;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::logs::LogLevel;
use crate::models::Property;
use crate::runners::FileUnavailable;
use crate::storage::{uri, FileStore, StoredFile};

/// `io.kestra.plugin.core.storage.Write`.
pub const WRITE: &str = "io.kestra.plugin.core.storage.Write";
/// `io.kestra.plugin.core.storage.Size`.
pub const SIZE: &str = "io.kestra.plugin.core.storage.Size";
/// `io.kestra.plugin.core.storage.Delete`.
pub const DELETE: &str = "io.kestra.plugin.core.storage.Delete";
/// `io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles`.
pub const PURGE_CURRENT_EXECUTION_FILES: &str =
    "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles";

/// The fixed stem of `Write`'s file name (see the module doc).
pub const WRITE_FILE_STEM: &str = "content";
/// `Write`'s extension when none is given: Java's `Files.createTempFile`
/// suffix default, measured (`w_other`'s URI ends `.tmp`).
pub const DEFAULT_EXTENSION: &str = ".tmp";

/// One property of a Kestra class, as its plugin schema declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prop {
    pub name: &'static str,
    pub required: bool,
    /// The schema's `default`, as its text.
    pub default: Option<&'static str>,
}

/// A class's properties and outputs (`name`, JSON type).
#[derive(Debug, Clone, Copy)]
pub struct Declared {
    pub type_id: &'static str,
    pub properties: &'static [Prop],
    pub outputs: &'static [(&'static str, &'static str)],
}

/// The four classes, as `w9a-storage.json`'s `core_storage` records them.
pub const DECLARED: [Declared; 4] = [
    Declared {
        type_id: WRITE,
        properties: &[
            Prop {
                name: "content",
                required: true,
                default: None,
            },
            Prop {
                name: "extension",
                required: false,
                default: None,
            },
        ],
        outputs: &[("uri", "string")],
    },
    Declared {
        type_id: SIZE,
        properties: &[Prop {
            name: "uri",
            required: true,
            default: None,
        }],
        outputs: &[("size", "integer")],
    },
    Declared {
        type_id: DELETE,
        properties: &[
            Prop {
                name: "errorOnMissing",
                required: false,
                default: Some("false"),
            },
            Prop {
                name: "uri",
                required: true,
                default: None,
            },
        ],
        outputs: &[("deleted", "boolean"), ("uri", "string")],
    },
    Declared {
        type_id: PURGE_CURRENT_EXECUTION_FILES,
        properties: &[],
        outputs: &[("uris", "array")],
    },
];

/// Register `Delete` and `PurgeCurrentExecutionFiles`, which need the pool
/// `Handlers::with_core_tasks` does not have. `runtime::handlers_for` calls
/// this for every deployment; `Write` and `Size` are in `with_core_tasks`.
pub fn register(
    handlers: &mut super::super::Handlers,
    pool: PgPool,
    files: Option<Arc<FileStore>>,
) {
    handlers.add_publishable(Arc::new(Delete::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(PurgeCurrentExecutionFiles::new(pool, files)));
}

fn no_storage(type_id: &str) -> String {
    format!(
        "{type_id} needs internal storage, and egret has none configured (the `[storage]` \
         section of its configuration)"
    )
}

/// The task's properties, or why not -- the message of a failed run.
fn spec<T: DeserializeOwned>(type_id: &str, task: &TaskInput) -> std::result::Result<T, String> {
    serde_json::from_value(task.spec.clone()).map_err(|e| format!("{type_id}: {e}"))
}

/// Spec §11 (Task 4): a `read()` inside a property that met a store OUTAGE
/// failed its render, and that failure is not the author's -- `Err`, so the
/// run is re-claimed with no attempt spent.
fn outage(type_id: &str, task: &TaskInput) -> Result<()> {
    match task
        .render
        .files()
        .and_then(|f| f.0.infrastructure_failure())
    {
        Some(why) => Err(Error::Internal(anyhow::anyhow!("{type_id}: {why}"))),
        None => Ok(()),
    }
}

fn db_err(type_id: &str, e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{type_id}: {e}"))
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WriteSpec {
    content: Property<String>,
    #[serde(default)]
    extension: Option<Property<String>>,
}

/// `core.storage.Write`: the rendered `content` as a file of this task run.
#[derive(Debug, Clone, Default)]
pub struct Write {
    files: Option<Arc<FileStore>>,
}

impl Write {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for Write {
    fn type_id(&self) -> &'static str {
        WRITE
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{WRITE}: {why}")));
        let spec: WriteSpec = match spec(WRITE, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(WRITE)));
        };
        let prefix = match super::task_storage_prefix(task, WRITE) {
            Ok(p) => p,
            Err(why) => return fail(why),
        };
        let content = spec.content.render(&task.render);
        let extension = spec.extension.map(|e| e.render(&task.render)).transpose();
        outage(WRITE, task)?;
        let content = match content {
            Ok(c) => c,
            Err(e) => return fail(e.to_string()),
        };
        let extension = match extension {
            Ok(e) => e.unwrap_or_else(|| DEFAULT_EXTENSION.to_string()),
            Err(e) => return fail(e.to_string()),
        };
        if extension.contains('/') {
            return fail(format!(
                "`extension` is {extension:?}: it is the end of the file's name and may not \
                 hold `/`"
            ));
        }
        let file = match uri::verbatim_file_uri(&prefix, &format!("{WRITE_FILE_STEM}{extension}")) {
            Ok(file) => file,
            Err(why) => return fail(why),
        };
        let object = match fs.object_uri(task.workspace_id, &file) {
            Ok(object) => object,
            Err(why) => return fail(why),
        };
        let bytes = content.into_bytes();
        let size = bytes.len() as u64;
        let caps = fs.caps;
        if size > caps.max_file_bytes {
            return fail(format!(
                "{file} would be {size} bytes, over the {} one file may be \
                 (workflow.storage.max_file_bytes)",
                caps.max_file_bytes
            ));
        }
        if size > caps.max_task_bytes {
            return fail(format!(
                "{file} would be {size} bytes, over the {} one task run may store \
                 (workflow.storage.max_task_bytes)",
                caps.max_task_bytes
            ));
        }
        let unavailable = |what: String| -> Result<Outcome> {
            Err(Error::Internal(anyhow::anyhow!("{WRITE}: {what}")))
        };
        let mut body: &[u8] = &bytes;
        let uploaded = match fs.store.put_stream(&object, &mut body, Some(size)).await {
            Ok(n) => n,
            Err(e) => {
                return unavailable(format!(
                    "{file} could not be stored: internal storage is unavailable: {e}"
                ))
            }
        };
        // `head` for the real size (spec §4.2): what the index row will hold.
        match fs.store.head(&object).await {
            Ok(Some(held)) if held == size && uploaded == size => {}
            Ok(Some(held)) => {
                return unavailable(format!(
                    "{file} holds {held} bytes in internal storage after {size} were written \
                     ({uploaded} uploaded)"
                ))
            }
            Ok(None) => {
                return unavailable(format!(
                    "{file} was written ({uploaded} bytes) but internal storage does not have it"
                ))
            }
            Err(e) => {
                return unavailable(format!(
                    "{file} could not be checked: internal storage is unavailable: {e}"
                ))
            }
        }
        Ok(Outcome {
            files: vec![StoredFile {
                uri: file.clone(),
                size,
            }],
            ..Outcome::succeeded_with(serde_json::json!({ "uri": file }))
        })
    }

    /// Holds nothing past run: the upload is awaited inside `run`, and
    /// dropping the future stops it; an object it left is unindexed, an
    /// orphan the pruner sweeps (spec §10).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UriSpec {
    uri: Property<String>,
}

/// `core.storage.Size`: a readable file's size in bytes, from the index.
#[derive(Debug, Clone, Copy, Default)]
pub struct Size;

#[async_trait::async_trait]
impl TaskHandler for Size {
    fn type_id(&self) -> &'static str {
        SIZE
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let spec: UriSpec = match spec(SIZE, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let rendered = spec.uri.render(&task.render);
        outage(SIZE, task)?;
        let file = match rendered {
            Ok(u) => u,
            Err(e) => return Ok(Outcome::failed_with(format!("{SIZE}: {e}"))),
        };
        let Some(scope) = task.render.files() else {
            return Ok(Outcome::failed_with(no_storage(SIZE)));
        };
        match scope.0.lookup(&file) {
            Ok(meta) => Ok(Outcome::succeeded_with(
                serde_json::json!({ "size": meta.size }),
            )),
            Err(why) => Ok(Outcome::failed_with(format!("{SIZE}: {}", why.message()))),
        }
    }

    /// Holds nothing past run: it answers from the run's snapshot.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DeleteSpec {
    uri: Property<String>,
    #[serde(rename = "errorOnMissing", default)]
    error_on_missing: Option<Property<bool>>,
}

/// `core.storage.Delete`: a file of this execution's loop family, index row
/// first (see the module doc).
#[derive(Debug, Clone)]
pub struct Delete {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl Delete {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

/// The refusal for a readable file outside the loop family. It names the
/// URI and says nothing the readability decision has not already said: the
/// URI was readable, so its execution is the caller's to know about.
fn not_deletable(file: &str) -> String {
    format!(
        "{DELETE}: {file} belongs to an execution outside this one's loop family: Delete \
         removes only files of the running execution, its loop root and the iterations under \
         it; a trigger's or a replay source's file is readable, never deletable"
    )
}

#[async_trait::async_trait]
impl TaskHandler for Delete {
    fn type_id(&self) -> &'static str {
        DELETE
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(why));
        let spec: DeleteSpec = match spec(DELETE, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let rendered = spec.uri.render(&task.render);
        let error_on_missing = spec
            .error_on_missing
            .map(|p| p.render(&task.render))
            .transpose();
        outage(DELETE, task)?;
        let file = match rendered {
            Ok(u) => u,
            Err(e) => return fail(format!("{DELETE}: {e}")),
        };
        let error_on_missing = match error_on_missing {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return fail(format!("{DELETE}: {e}")),
        };
        let Some(fs) = &self.files else {
            return fail(no_storage(DELETE));
        };
        let Some(scope) = task.render.files() else {
            return fail(no_storage(DELETE));
        };
        // The one readability decision first (the snapshot): an unreadable
        // URI is refused exactly as `read()` refuses it, and `Missing` -- a
        // readable execution, no such file -- goes on, to be `deleted: false`
        // if it is the family's.
        match scope.0.lookup(&file) {
            Ok(_) | Err(FileUnavailable::Missing(_)) => {}
            Err(FileUnavailable::Refused(why)) => return fail(format!("{DELETE}: {why}")),
        }
        let parsed = match uri::parse(&file) {
            Ok(p) => p,
            Err(why) => return fail(format!("{DELETE}: {file}: {why}")),
        };
        let mut conn = self.pool.acquire().await.map_err(|e| db_err(DELETE, e))?;
        let family =
            crate::storage::loop_family(&mut conn, task.workspace_id, task.execution_id).await?;
        // Decided from the URI's `{exec}` alone, before any row of it is read.
        if !family.contains(&parsed.execution) {
            return fail(not_deletable(&file));
        }
        // THE INDEX ROW FIRST. Scoped to the family in SQL too, so the
        // statement itself cannot reach another execution's row.
        let gone: Option<String> = sqlx::query_scalar(
            "DELETE FROM workflow.execution_files \
              WHERE workspace_id = $1 AND uri = $2 AND execution_id = ANY($3) \
              RETURNING uri",
        )
        .bind(task.workspace_id)
        .bind(&file)
        .bind(&family)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| db_err(DELETE, e))?;
        drop(conn);
        let deleted = gone.is_some();
        if deleted {
            delete_object(fs, task, &file).await;
        }
        if error_on_missing && !deleted {
            // `Delete.java`: `throw new NoSuchElementException("Unable to
            // find file '" + render + "'")`, measured verbatim.
            return fail(format!("Unable to find file '{file}'"));
        }
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "uri": file, "deleted": deleted }),
        ))
    }

    /// Holds nothing past run: the row is deleted and committed inside `run`,
    /// and an object delete cut short is an orphan the pruner sweeps.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// Delete the object of a file whose index row is already gone. A failure
/// leaves an orphan, swept at retention (spec §10), and is logged, never an
/// error: the file no longer exists for any reader.
async fn delete_object(fs: &FileStore, task: &TaskInput, file: &str) {
    let failed = match fs.object_uri(task.workspace_id, file) {
        Ok(object) => fs.store.delete(&object).await.err().map(|e| e.to_string()),
        Err(why) => Some(why),
    };
    if let Some(why) = failed {
        task.log
            .line(
                LogLevel::Warn,
                &format!(
                    "{file} is deleted (its index row is gone), but its object could not be \
                     removed from internal storage and is left as an orphan for the pruner: \
                     {why}"
                ),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// PurgeCurrentExecutionFiles
// ---------------------------------------------------------------------------

/// `core.storage.PurgeCurrentExecutionFiles`: every file of this execution's
/// loop family, index rows first (see the module doc).
#[derive(Debug, Clone)]
pub struct PurgeCurrentExecutionFiles {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl PurgeCurrentExecutionFiles {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for PurgeCurrentExecutionFiles {
    fn type_id(&self) -> &'static str {
        PURGE_CURRENT_EXECUTION_FILES
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(
                PURGE_CURRENT_EXECUTION_FILES,
            )));
        };
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| db_err(PURGE_CURRENT_EXECUTION_FILES, e))?;
        let family =
            crate::storage::loop_family(&mut conn, task.workspace_id, task.execution_id).await?;
        let gone: Vec<String> = sqlx::query_scalar(
            "DELETE FROM workflow.execution_files \
              WHERE workspace_id = $1 AND execution_id = ANY($2) \
              RETURNING uri",
        )
        .bind(task.workspace_id)
        .bind(&family)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| db_err(PURGE_CURRENT_EXECUTION_FILES, e))?;
        drop(conn);
        for file in &gone {
            delete_object(fs, task, file).await;
        }
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "uris": purged_listing(&gone) }),
        ))
    }

    /// Holds nothing past run: the rows are deleted and committed inside
    /// `run`, and an object delete cut short is an orphan the pruner sweeps.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// A directory of the listing: its children by name.
#[derive(Default)]
struct Dir(BTreeMap<String, Dir>);

/// What Kestra's `deleteByPrefix` answers for an execution's files: every
/// file, and every directory between it and the execution's own, each
/// directory after everything under it (post-order), down to
/// `kestra:///{ns}/{flow}/executions/{exec}` (measured,
/// `core_storage.PurgeCurrentExecutionFiles.runs`). Children are sorted;
/// executions too. A URI that does not parse (none can: egret indexed it) is
/// listed as itself.
pub(crate) fn purged_listing(uris: &[String]) -> Vec<String> {
    let mut roots: BTreeMap<String, Dir> = BTreeMap::new();
    let mut odd = Vec::new();
    for file in uris {
        let Some((root, rest)) = uri::parse(file).ok().and_then(|p| {
            let marker = format!("/executions/{}/", p.execution.hyphenated());
            file.find(&marker).map(|at| {
                let end = at + marker.len();
                (file[..end - 1].to_string(), file[end..].to_string())
            })
        }) else {
            odd.push(file.clone());
            continue;
        };
        let mut dir = roots.entry(root).or_default();
        for segment in rest.split('/') {
            dir = dir.0.entry(segment.to_string()).or_default();
        }
    }
    fn walk(path: &str, dir: &Dir, out: &mut Vec<String>) {
        for (name, child) in &dir.0 {
            let here = format!("{path}/{name}");
            walk(&here, child, out);
            out.push(here);
        }
    }
    let mut out = Vec::new();
    for (root, dir) in &roots {
        walk(root, dir, &mut out);
        out.push(root.clone());
    }
    out.extend(odd);
    out
}

#[cfg(test)]
mod tests {
    use super::super::testing::input;
    use super::*;
    use crate::executor::RunState;
    use crate::runners::RenderContext;
    use crate::storage::scope::Snapshot;
    use crate::storage::{Caps, IndexedFile, Unreadable};
    use egret_storage::{MemStore, ObjectStore, StorageError};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    const EX: Uuid = Uuid::from_u128(0xe1);
    const RUN: Uuid = Uuid::from_u128(0xa1);
    const OTHER: Uuid = Uuid::from_u128(0xe2);

    fn store_with(store: Arc<dyn ObjectStore>, caps: Caps) -> Arc<FileStore> {
        Arc::new(FileStore {
            store,
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps,
            retention_days: 30,
        })
    }

    fn mem() -> Arc<FileStore> {
        store_with(Arc::new(MemStore::new()), Caps::default())
    }

    fn prefix() -> String {
        uri::task_prefix("w9a.t11", "unit", EX, "t", RUN).unwrap()
    }

    /// A run of `type_id` over `spec`, rendering with a snapshot of `files`
    /// over `fs` whose task prefix is this run's.
    fn task_over(
        type_id: &str,
        spec: serde_json::Value,
        fs: &Arc<FileStore>,
        files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    ) -> TaskInput {
        let mut t = input(type_id, spec);
        t.execution_id = EX;
        t.run_id = RUN;
        let snap = Snapshot::from_parts(files, fs.clone(), t.workspace_id, Some(prefix()));
        t.render = RenderContext::new().with_files(snap.into_scope());
        t
    }

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool parses")
    }

    fn message(o: &Outcome) -> &str {
        assert_eq!(o.state, RunState::Failed, "{o:?}");
        o.message.as_deref().unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_stores_content_under_the_runs_prefix_and_hands_the_file_to_the_commit() {
        let fs = mem();
        let t = task_over(
            WRITE,
            serde_json::json!({"content": "l1\nl2\n", "extension": ".txt"}),
            &fs,
            BTreeMap::new(),
        );
        let out = Write::new(Some(fs.clone())).run(&t).await.unwrap();
        let file = format!("{}content.txt", prefix());
        assert_eq!(out.outputs, Some(serde_json::json!({ "uri": file })));
        assert_eq!(
            out.files,
            vec![StoredFile {
                uri: file.clone(),
                size: 6
            }]
        );
        assert_eq!(
            fs.store
                .get(&fs.object_uri(t.workspace_id, &file).unwrap())
                .await
                .unwrap(),
            b"l1\nl2\n"
        );
        // No `extension:` is Kestra's `.tmp`.
        let t = task_over(
            WRITE,
            serde_json::json!({"content": "x"}),
            &fs,
            BTreeMap::new(),
        );
        let out = Write::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({ "uri": format!("{}content.tmp", prefix()) }))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_refuses_an_extension_holding_a_slash() {
        let fs = mem();
        let t = task_over(
            WRITE,
            serde_json::json!({"content": "x", "extension": "/../y"}),
            &fs,
            BTreeMap::new(),
        );
        let out = Write::new(Some(fs.clone())).run(&t).await.unwrap();
        assert!(message(&out).contains("may not hold `/`"), "{out:?}");
        assert!(out.files.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_without_a_store_fails_naming_storage() {
        let fs = mem();
        let t = task_over(
            WRITE,
            serde_json::json!({"content": "x"}),
            &fs,
            BTreeMap::new(),
        );
        let out = Write::new(None).run(&t).await.unwrap();
        assert!(message(&out).contains("[storage]"), "{out:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_over_either_cap_fails_naming_it_and_stores_nothing() {
        for (caps, cap) in [
            (
                Caps {
                    max_file_bytes: 3,
                    ..Caps::default()
                },
                "workflow.storage.max_file_bytes",
            ),
            (
                Caps {
                    max_task_bytes: 3,
                    ..Caps::default()
                },
                "workflow.storage.max_task_bytes",
            ),
        ] {
            let fs = store_with(Arc::new(MemStore::new()), caps);
            let t = task_over(
                WRITE,
                serde_json::json!({"content": "four"}),
                &fs,
                BTreeMap::new(),
            );
            let out = Write::new(Some(fs.clone())).run(&t).await.unwrap();
            assert!(message(&out).contains(cap), "{out:?}");
            let object = fs
                .object_uri(t.workspace_id, &format!("{}content.tmp", prefix()))
                .unwrap();
            assert!(matches!(
                fs.store.get(&object).await,
                Err(StorageError::NotFound(_))
            ));
            // The control: at the cap it is written.
            let t = task_over(
                WRITE,
                serde_json::json!({"content": "thr"}),
                &fs,
                BTreeMap::new(),
            );
            assert_eq!(
                Write::new(Some(fs.clone())).run(&t).await.unwrap().state,
                RunState::Success
            );
        }
    }

    /// A store that refuses every write.
    struct Down;

    #[async_trait::async_trait]
    impl ObjectStore for Down {
        async fn put(&self, uri: &str, _: Vec<u8>) -> std::result::Result<(), StorageError> {
            Err(StorageError::S3(format!("{uri}: connection refused")))
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            Err(StorageError::S3(format!("{uri}: connection refused")))
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn copy(&self, _: &str, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            unreachable!()
        }
        async fn put_stream(
            &self,
            uri: &str,
            _: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_outage_while_writing_is_a_worker_failure_naming_the_uri() {
        let fs = store_with(Arc::new(Down), Caps::default());
        let t = task_over(
            WRITE,
            serde_json::json!({"content": "x"}),
            &fs,
            BTreeMap::new(),
        );
        let err = Write::new(Some(fs.clone()))
            .run(&t)
            .await
            .expect_err("an outage is Err");
        assert!(
            err.to_string()
                .contains(&format!("{}content.tmp", prefix())),
            "{err}"
        );
    }

    fn indexed(
        file: &str,
        execution: Uuid,
        size: u64,
    ) -> (String, std::result::Result<IndexedFile, Unreadable>) {
        (
            file.to_string(),
            Ok(IndexedFile {
                uri: file.to_string(),
                execution_id: execution,
                task_run_id: Some(RUN),
                size,
            }),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn size_answers_from_the_index_and_refuses_what_the_snapshot_refuses() {
        let fs = mem();
        let here = format!("{}a.txt", prefix());
        let gone = format!("{}gone.txt", prefix());
        let foreign = format!(
            "{}b.txt",
            uri::task_prefix("w9a.t11", "unit", OTHER, "t", RUN).unwrap()
        );
        let files = BTreeMap::from([
            indexed(&here, EX, 15),
            (gone.clone(), Err(Unreadable::Missing { uri: gone.clone() })),
            (
                foreign.clone(),
                Err(Unreadable::NotReadable {
                    uri: foreign.clone(),
                }),
            ),
        ]);
        let run = |u: &str| task_over(SIZE, serde_json::json!({ "uri": u }), &fs, files.clone());
        let out = Size.run(&run(&here)).await.unwrap();
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"size": 15})),
            "no byte read: the object does not even exist"
        );
        assert!(message(&Size.run(&run(&gone)).await.unwrap())
            .contains(&format!("{gone} does not exist")));
        assert!(message(&Size.run(&run(&foreign)).await.unwrap()).contains("may not read"));
        // Not in the snapshot at all (assembled at render): refused by name.
        let assembled = format!("{}c.txt", prefix());
        assert!(message(&Size.run(&run(&assembled)).await.unwrap()).contains("was not looked up"));
    }

    /// `Delete` asks the snapshot FIRST: an unreadable URI is refused exactly
    /// as `read()` refuses it, before any connection is taken (the pool here
    /// never connects, so reaching it would be an `Err`).
    #[tokio::test(flavor = "multi_thread")]
    async fn delete_refuses_an_unreadable_uri_before_touching_the_database() {
        let fs = mem();
        let foreign = format!(
            "{}b.txt",
            uri::task_prefix("w9a.t11", "unit", OTHER, "t", RUN).unwrap()
        );
        let files = BTreeMap::from([(
            foreign.clone(),
            Err(Unreadable::NotReadable {
                uri: foreign.clone(),
            }),
        )]);
        let t = task_over(DELETE, serde_json::json!({ "uri": foreign }), &fs, files);
        let out = Delete::new(lazy_pool(), Some(fs.clone()))
            .run(&t)
            .await
            .unwrap();
        assert!(message(&out).contains("may not read"), "{out:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn delete_and_purge_without_a_store_fail_naming_storage() {
        let fs = mem();
        let file = format!("{}a.txt", prefix());
        let t = task_over(
            DELETE,
            serde_json::json!({ "uri": file }),
            &fs,
            BTreeMap::from([indexed(&file, EX, 1)]),
        );
        let out = Delete::new(lazy_pool(), None).run(&t).await.unwrap();
        assert!(message(&out).contains("[storage]"), "{out:?}");
        let t = task_over(
            PURGE_CURRENT_EXECUTION_FILES,
            serde_json::json!({}),
            &fs,
            BTreeMap::new(),
        );
        let out = PurgeCurrentExecutionFiles::new(lazy_pool(), None)
            .run(&t)
            .await
            .unwrap();
        assert!(message(&out).contains("[storage]"), "{out:?}");
    }

    #[tokio::test]
    async fn a_missing_required_property_fails_the_task() {
        let fs = mem();
        assert!(message(
            &Write::new(Some(fs.clone()))
                .run(&input(WRITE, serde_json::json!({})))
                .await
                .unwrap()
        )
        .contains("content"));
        assert!(
            message(&Size.run(&input(SIZE, serde_json::json!({}))).await.unwrap()).contains("uri")
        );
        assert!(message(
            &Delete::new(lazy_pool(), Some(fs))
                .run(&input(DELETE, serde_json::json!({})))
                .await
                .unwrap()
        )
        .contains("uri"));
    }

    #[test]
    fn the_listing_is_post_order_down_to_the_execution() {
        let ex = uuid::Uuid::from_u128(7);
        let run = uuid::Uuid::from_u128(8);
        let base = format!("kestra:///a/b/f/executions/{ex}");
        let uris = vec![
            format!("{base}/tasks/w-a/{run}/x.txt"),
            format!("{base}/tasks/sh/{run}/y.txt"),
        ];
        assert_eq!(
            purged_listing(&uris),
            vec![
                format!("{base}/tasks/sh/{run}/y.txt"),
                format!("{base}/tasks/sh/{run}"),
                format!("{base}/tasks/sh"),
                format!("{base}/tasks/w-a/{run}/x.txt"),
                format!("{base}/tasks/w-a/{run}"),
                format!("{base}/tasks/w-a"),
                format!("{base}/tasks"),
                base.clone(),
            ]
        );
        assert!(
            purged_listing(&[]).is_empty(),
            "nothing purged lists nothing"
        );
    }
}
