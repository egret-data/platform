//! `io.kestra.plugin.core.namespace.{UploadFiles, DownloadFiles, DeleteFiles,
//! PurgeFiles}` -- the core namespace-file tasks W9b Tasks 6 and 7 port (W9b
//! spec §8.1-§8.4).
//!
//! # The shapes are Kestra's, measured, except the chosen cells
//!
//! Each class's properties (name, `required`, `default`) and outputs are
//! [`DECLARED`], and `tests/nsfiles_tasks.rs` holds that table against
//! `oracle-exec/w9b-namespace-files.json`'s `tasks.<class>.schema` (egret's
//! answer, the top-level value of each cell).
//!
//! | class | properties | outputs |
//! |---|---|---|
//! | `UploadFiles` | `conflict` (default `OVERWRITE`), `destination` (default `/`), `files`, `filesMap`, `namespace` (default the flow's) | `files` (declared; never set, as Kestra's run) |
//! | `DownloadFiles` | `destination` (default `""`), `files` (required), `namespace` (required) | `files`: path -> `kestra://` URI |
//! | `DeleteFiles` | `deleteParentFolder` (default false), `files` (required), `namespace` (default the flow's) | `files` (declared; never set, as Kestra's run) |
//! | `PurgeFiles` | `behavior` (default `{type: version, keepAmount: 1}`), `filePattern`, `includeChildNamespaces` (default true), `namespacePattern`, `namespaces` | `size`: revisions purged |
//!
//! # Where egret differs, on purpose (spec §2)
//!
//! - **A task writes only to the flow's OWN namespace.** `UploadFiles` to
//!   any other is refused by `nsfiles::access::authorize` (Write), naming the
//!   namespace. Its `namespace` is optional and defaults to the flow's (the
//!   coordinator's ruling D; Kestra requires it at publish).
//! - **`UploadFiles.files` matches the execution's READABLE files** (W9a
//!   §4.4's readable set: the running execution, its loop family, its
//!   trigger's and replay source's), each matched on its URI's file path
//!   (the `Tail`'s `file`, with a leading `/`); Kestra globs its working
//!   directory, which a native handler does not have. A whole `kestra:///`
//!   string is one file, resolved through the render snapshot -- W9a's one
//!   readability decision.
//! - **Every `files` and `filesMap` source is resolved before the first
//!   write**, and one that does not resolve fails the task naming it, writing
//!   nothing (Kestra skips it silently). A `kestra:///…/_files/…` or
//!   `nsfile://…` source is a namespace file of the workspace, read through
//!   `authorize` (Read) as staging reads it (Task 5's `resolve_input_uri`)
//!   and copied into the flow's own namespace -- under `files` at its
//!   namespace path, under `filesMap` at its key (fix round 1, M2). A
//!   `filesMap` value that is neither is the file's contents.
//! - **`destination` is a directory**: a missing leading `/` is added, one
//!   trailing `/` dropped, and each name joined with exactly one `/`
//!   (`up2` + `m.txt` -> `/up2/m.txt`; Kestra wrote `/up2m.txt`). A file
//!   `files` matched keeps its path under its task run (`content.txt`, or
//!   `sub/x.txt` for an `outputDirectory` file).
//! - **One source per destination path** (the coordinator's ruling of
//!   2026-10-06, R3; fix round 1, I2): the same source named twice (a URI and
//!   a glob that also matches it) is written once; two DIFFERENT sources
//!   that land on one path (two Writes' `content.txt` under `**.txt`) refuse
//!   the task before anything is written, naming the path and both sources.
//!   Kestra writes both, in its listing's order.
//! - **Per file, not atomic across files; a re-claim is idempotent** (fix
//!   round 1, I1). Each file commits on its own (§4.2), so an author error
//!   on entry k (a conflict under ERROR, a 409 of another kind, an object
//!   gone behind its row) leaves entries 1..k-1 written, as Kestra's
//!   per-file `putFile` does. An infrastructure failure re-claims the same
//!   task run, and it skips each file whose latest revision that task run
//!   wrote (`created_by_task_run`) with the source's size, under every
//!   conflict mode: no second OVERWRITE revision and no ERROR against its
//!   own write. Another task run of the same execution is not "itself".
//! - **`DownloadFiles` joins `destination` and the path with exactly one
//!   `/`** (`dl/` or `dl` + `/up/m.txt` -> `…/{taskRunId}/dl/up/m.txt`; `""`
//!   -> `…/{taskRunId}/up/m.txt`). Kestra concatenates (`dl` would give
//!   `dlup/m.txt`).
//! - **`DeleteFiles` writes, so only the flow's own namespace** (its
//!   `namespace` optional, default the flow's: ruling D). It is a soft delete
//!   (§4.3; the user's ruling R1): history stays listed and readable by
//!   revision until `PurgeFiles`, and no object is removed. With
//!   `deleteParentFolder`, each parent of a deleted file is soft-deleted,
//!   deepest first, with its empty subdirectories, when no live FILE is left
//!   at any depth under it (Kestra's rule; the user's ruling of 2026-10-06),
//!   judged in one transaction holding its lock (`store::soft_delete_if_empty`).
//! - **`PurgeFiles` purges the flow's own namespace and its children**, never
//!   another (§5). Default: the flow's own namespace (Kestra: every namespace
//!   holding a flow), plus its children with `includeChildNamespaces`;
//!   children match on a DOT boundary (`a.bc` is not a child of `a.b`;
//!   Kestra's `startsWith` takes it). A listed `namespaces` entry outside the
//!   rule fails the task naming it, before anything is purged.
//!   `namespacePattern` is Kestra's wildcard (`FilenameUtils.wildcardMatch`:
//!   `*`, `?`, case-sensitive; never a regex) matched only against the
//!   namespaces holding files that are the flow's own or its children (R2;
//!   the coordinator's ruling B): it is a FILTER, so it never refuses and
//!   never reaches another namespace. A path whose latest revision is
//!   soft-deleted is purged entirely, whatever `behavior` keeps (Kestra's
//!   `keepAmount: 1` keeps its deleted latest revision). A soft-deleted
//!   directory with nothing left under it is purged too (and with
//!   `keepAmount: 0` a live one), counted in `size`. `before` is `<=`, as
//!   Kestra's; a `behavior` with neither property is refused (the user's
//!   ruling of 2026-10-06).
//!
//! # Bounds and errors (spec §7, §11)
//!
//! The patterns are compiled off the async runtime under `glob`'s count and
//! per-list budget; matching runs on the blocking pool too, and no database
//! connection is held while it does. `UploadFiles` lists at most
//! `max_namespace_files + 1` readable execution files ([`readable_execution_files`]),
//! `DownloadFiles` at most `max_namespace_files + 1` namespace rows
//! (`list_live`); the `+1` is the refusal, naming the cap. `DownloadFiles`
//! refuses matches whose rows hold more than `max_task_bytes` together;
//! `UploadFiles` refuses more resolved sources than `max_namespace_files`,
//! or more bytes than `max_task_bytes`, before its first write (fix round
//! 1, M3). `DeleteFiles` lists at most `max_namespace_files + 1` live rows.
//! `PurgeFiles` takes at most [`MAX_NAMESPACES`] namespaces (listed, and
//! covered), and reads each namespace's rows in keyset pages of
//! `max_namespace_files + 1`; it purges each path at once by ROW id
//! (`store::purge_rows`, under the path's session lock), so a stale plan
//! never removes a newer file that reused a revision number, and it refuses
//! only one file with more revisions than the cap ([`run_purge`]). It runs
//! on a detached task, to completion once started.
//! `NsError::{Store, Db}` and a store that fails are infrastructure: `Err`,
//! so the lease lapses and the run is re-claimed with no attempt spent;
//! everything else is a failed task naming the path, the property or the cap.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use egret_core::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use super::super::{Outcome, TaskHandler, TaskInput};
use super::storage::{no_storage, outage, spec};
use crate::logs::LogLevel;
use crate::models::Property;
use crate::runners::files::is_namespace_file_uri;
use crate::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use crate::storage::nsfiles::glob::{wildcard_match, wildcard_matches, PathMatcher};
use crate::storage::nsfiles::path::NsPath;
use crate::storage::nsfiles::stage::{resolve_input_uri, ListProp, MAX_NAMESPACES};
use crate::storage::nsfiles::store::{
    file_rows_page, latest_written, list_live, namespaces_with_files, open, purge_empty_directory,
    purge_rows, purgeable_directories_page, put, soft_delete, soft_delete_if_empty, Conflict,
    NsEntry, Written,
};
use crate::storage::nsfiles::{CreatedBy, NsError};
use crate::storage::uri::{self, Tail};
use crate::storage::{readable_executions, FileStore, StoredFile};

/// `io.kestra.plugin.core.namespace.UploadFiles`.
pub const UPLOAD_FILES: &str = "io.kestra.plugin.core.namespace.UploadFiles";
/// `io.kestra.plugin.core.namespace.DownloadFiles`.
pub const DOWNLOAD_FILES: &str = "io.kestra.plugin.core.namespace.DownloadFiles";
/// `io.kestra.plugin.core.namespace.DeleteFiles` (Task 7).
pub const DELETE_FILES: &str = "io.kestra.plugin.core.namespace.DeleteFiles";
/// `io.kestra.plugin.core.namespace.PurgeFiles` (Task 7).
pub const PURGE_FILES: &str = "io.kestra.plugin.core.namespace.PurgeFiles";

/// One class: its type, its properties `(name, required, default)` and its
/// outputs, as `w9b-namespace-files.json`'s `tasks.<class>.schema` records
/// them (egret's answer: a chosen cell's top level, never its `kestra`).
pub type DeclaredClass = (
    &'static str,
    &'static [(&'static str, bool, Option<&'static str>)],
    &'static [&'static str],
);

/// The classes this module runs. `pub` (not `pub(crate)`) so the
/// integration test that holds it against the artifact can read it.
pub const DECLARED: &[DeclaredClass] = &[
    (
        UPLOAD_FILES,
        &[
            ("conflict", false, Some("OVERWRITE")),
            ("destination", false, Some("/")),
            ("files", false, None),
            ("filesMap", false, None),
            // Chosen (ruling D): Kestra's is `required: true`.
            ("namespace", false, Some("the flow's namespace")),
        ],
        &["files"],
    ),
    (
        DOWNLOAD_FILES,
        &[
            ("destination", false, Some("")),
            ("files", true, None),
            ("namespace", true, None),
        ],
        &["files"],
    ),
    (
        DELETE_FILES,
        &[
            ("deleteParentFolder", false, Some("false")),
            ("files", true, None),
            // Chosen (ruling D): Kestra's is `required: true`.
            ("namespace", false, Some("the flow's namespace")),
        ],
        &["files"],
    ),
    (
        PURGE_FILES,
        &[
            (
                "behavior",
                false,
                Some("{\"type\":\"version\",\"keepAmount\":1}"),
            ),
            ("filePattern", false, None),
            ("includeChildNamespaces", false, Some("true")),
            ("namespacePattern", false, None),
            ("namespaces", false, None),
        ],
        &["size"],
    ),
];

/// Register the namespace tasks, which need the pool. `runtime::handlers_for`
/// calls this for every deployment, beside `storage::register`.
pub fn register(
    handlers: &mut super::super::Handlers,
    pool: PgPool,
    files: Option<Arc<FileStore>>,
) {
    handlers.add_publishable(Arc::new(UploadFiles::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(DownloadFiles::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(DeleteFiles::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(PurgeFiles::new(pool, files)));
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

/// The running flow's namespace: the render scope's (Task 5's
/// `flow_namespace`) and nothing else.
pub(crate) fn flow_namespace(task: &TaskInput) -> Option<String> {
    task.render
        .files()
        .and_then(|f| f.0.flow_namespace().map(str::to_string))
}

fn not_a_flow_run() -> String {
    "this run is not a task run of a flow's execution, so it has no namespace of its own \
     to read or write namespace files from"
        .to_string()
}

/// `NsError::{Store, Db}` are infrastructure (`Err`); anything else is the
/// author's, the message of a failed run.
pub(super) fn split(type_id: &str, e: NsError) -> Result<String> {
    match e {
        NsError::Db(e) => Err(e),
        NsError::Store(m) => Err(Error::Internal(anyhow::anyhow!("{type_id}: {m}"))),
        other => Ok(format!("{type_id}: {other}")),
    }
}

fn db_err(type_id: &str, e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(
        "{type_id}: the database is unavailable: {e}"
    ))
}

/// A store that failed (not a missing object): the store's error goes to the
/// server log only, it may name the `s3://` key.
fn store_down(type_id: &str, what: &str, e: &dyn std::fmt::Display) -> Error {
    tracing::error!(task = type_id, file = what, error = %e, "internal storage failed");
    Error::Internal(anyhow::anyhow!(
        "{type_id}: {what}: internal storage is unavailable; retry later"
    ))
}

/// Render one string of the task's spec; `shown` names it in the message.
fn render_text(task: &TaskInput, shown: &str, s: &str) -> std::result::Result<String, String> {
    crate::runners::render_str(s, &task.render).map_err(|e| format!("`{shown}`: {e}"))
}

/// `glob`'s compile, off the async runtime (it is bounded CPU work).
async fn compile(includes: Vec<String>) -> std::result::Result<Arc<PathMatcher>, String> {
    tokio::task::spawn_blocking(move || PathMatcher::new(&includes, &[]))
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
        .map(Arc::new)
}

/// The items `matcher` accepts, judged on `key(item)`, on the blocking pool.
async fn matching<T: Send + 'static>(
    matcher: &Arc<PathMatcher>,
    items: Vec<T>,
    key: fn(&T) -> Option<String>,
) -> Vec<T> {
    let matcher = matcher.clone();
    tokio::task::spawn_blocking(move || {
        items
            .into_iter()
            .filter(|i| key(i).is_some_and(|k| matcher.matches_str(&k)))
            .collect()
    })
    .await
    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// An execution file's path under its execution, as its URI's `Tail` holds it.
fn tail_file(tail: &Tail) -> &str {
    match tail {
        Tail::Input { file, .. }
        | Tail::Task { file, .. }
        | Tail::Trigger { file, .. }
        | Tail::Execution { file } => file,
    }
}

/// The readable execution files of `workspace`: every row whose execution is
/// in `readable` (`storage::readable_executions` of the running execution),
/// at most `cap + 1` of them, in byte order of the URI. The `+1` is how the
/// caller tells a set over the cap, without fetching the rest.
pub async fn readable_execution_files(
    conn: &mut PgConnection,
    workspace: Uuid,
    readable: &[Uuid],
    cap: u64,
) -> Result<Vec<(String, u64)>> {
    let limit = i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX);
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT uri, size FROM workflow.execution_files \
          WHERE workspace_id = $1 AND execution_id = ANY($2) \
          ORDER BY uri COLLATE \"C\" LIMIT $3",
    )
    .bind(workspace)
    .bind(readable)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| db_err(UPLOAD_FILES, e))?;
    Ok(rows
        .into_iter()
        .map(|(uri, size)| (uri, u64::try_from(size).unwrap_or(0)))
        .collect())
}

// ---------------------------------------------------------------------------
// UploadFiles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct UploadSpec {
    #[serde(default)]
    namespace: Option<Property<String>>,
    #[serde(default)]
    files: Option<Value>,
    #[serde(rename = "filesMap", default)]
    files_map: Option<Value>,
    #[serde(default)]
    destination: Option<Property<String>>,
    #[serde(default)]
    conflict: Option<Property<String>>,
}

/// Where one file of an `UploadFiles` comes from.
#[derive(Debug)]
enum Source {
    /// A readable execution file (the snapshot, or the glob listing).
    Execution { uri: String, size: u64 },
    /// A namespace file of the workspace, read through `authorize` (Read)
    /// and `store::resolve` (Task 5's `resolve_input_uri`): `uri` as given,
    /// `object` its revision's key.
    Namespace {
        uri: String,
        object: String,
        size: u64,
    },
    /// A `filesMap` value that is not a URI: the file's contents. `shown`
    /// names the entry.
    Contents { shown: String, text: String },
}

impl Source {
    fn size(&self) -> u64 {
        match self {
            Self::Execution { size, .. } | Self::Namespace { size, .. } => *size,
            Self::Contents { text, .. } => text.len() as u64,
        }
    }

    /// What a message calls it.
    fn shown(&self) -> String {
        match self {
            Self::Execution { uri, .. } | Self::Namespace { uri, .. } => uri.clone(),
            Self::Contents { shown, .. } => format!("the contents given in {shown}"),
        }
    }

    /// Two entries with the same identity are the same source.
    fn identity(&self) -> String {
        match self {
            Self::Execution { uri, .. } => format!("execution {uri}"),
            Self::Namespace { object, .. } => format!("namespace {object}"),
            Self::Contents { shown, .. } => format!("contents {shown}"),
        }
    }
}

/// The resolved writes, one per destination path, in the order planned.
#[derive(Debug, Default)]
struct Plan {
    entries: Vec<(NsPath, Source)>,
    by_path: BTreeMap<String, usize>,
}

impl Plan {
    /// The same source named twice for one path is written once; two
    /// DIFFERENT sources for one path are refused before anything is written
    /// (the coordinator's ruling of 2026-10-06, an application of R3: their
    /// order would be the listing's, not the flow's, and under ERROR the task
    /// would conflict with its own write).
    fn add(&mut self, path: NsPath, source: Source) -> std::result::Result<(), String> {
        if let Some(&i) = self.by_path.get(path.as_str()) {
            let first = &self.entries[i].1;
            if first.identity() == source.identity() {
                return Ok(());
            }
            return Err(format!(
                "'{path}' would be written from two different sources, {} and {}: give one \
                 of them another destination, or upload them in two tasks",
                first.shown(),
                source.shown()
            ));
        }
        self.by_path
            .insert(path.as_str().to_string(), self.entries.len());
        self.entries.push((path, source));
        Ok(())
    }
}

/// `core.namespace.UploadFiles`: execution files (or literal contents) into
/// the flow's own namespace, each written by `nsfiles::store::put`.
#[derive(Debug, Clone)]
pub struct UploadFiles {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl UploadFiles {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

fn parse_conflict(s: &str) -> std::result::Result<Conflict, String> {
    match s.trim() {
        "OVERWRITE" => Ok(Conflict::Overwrite),
        "ERROR" => Ok(Conflict::Error),
        "SKIP" => Ok(Conflict::Skip),
        other => Err(format!(
            "`conflict` must be one of OVERWRITE, ERROR or SKIP, and it is {other:?}"
        )),
    }
}

/// `filesMap`, rendered: a map (each key and value rendered on its own), or
/// a string rendered to a JSON map, as Kestra's `filesMap` (`Object`,
/// `@PluginProperty(dynamic = true)`). Every value must be a string.
fn render_files_map(
    task: &TaskInput,
    v: &Value,
) -> std::result::Result<Vec<(String, String)>, String> {
    let must = "`filesMap` must be a map of file names to strings (a kestra:// URI, or the \
                file's contents), or an expression that renders to one";
    let strings = |map: &serde_json::Map<String, Value>| {
        map.iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                other => Err(format!("{must}; `filesMap.{k}` is {other}")),
            })
            .collect::<std::result::Result<Vec<_>, _>>()
    };
    match v {
        Value::Object(map) => strings(map)?
            .into_iter()
            .map(|(k, s)| {
                Ok((
                    render_text(task, &format!("filesMap key {k:?}"), &k)?,
                    render_text(task, &format!("filesMap.{k}"), &s)?,
                ))
            })
            .collect(),
        Value::String(s) => {
            let out = render_text(task, "filesMap", s)?;
            match serde_json::from_str::<Value>(&out) {
                Ok(Value::Object(map)) => strings(&map),
                _ => Err(format!("{must}; it rendered to {out:?}")),
            }
        }
        other => Err(format!("{must}, not {other}")),
    }
}

/// `destination` as a directory: a missing leading `/` added, one trailing
/// `/` dropped (`/` -> the root, `""`).
fn destination_dir(destination: &str) -> String {
    let mut dir = destination.to_string();
    if !dir.starts_with('/') {
        dir.insert(0, '/');
    }
    if dir.ends_with('/') {
        dir.pop();
    }
    dir
}

/// `dir` and `name` joined with exactly one `/`, through THE path rule.
fn upload_path(dir: &str, name: &str) -> std::result::Result<NsPath, String> {
    let name = name.strip_prefix('/').unwrap_or(name);
    NsPath::parse_file(&format!("{dir}/{name}"))
}

/// An execution file planned under `dir`, at its `Tail`'s path.
fn plan_execution_file(
    plan: &mut Plan,
    dir: &str,
    shown: &str,
    file: String,
    size: u64,
) -> std::result::Result<(), String> {
    let parsed = uri::parse(&file).map_err(|why| format!("{shown}: {file}: {why}"))?;
    let path = upload_path(dir, tail_file(&parsed.tail))
        .map_err(|why| format!("{shown} ({file}): {why}"))?;
    plan.add(path, Source::Execution { uri: file, size })
}

enum OpenFailed {
    /// No object behind the row: the author's error, naming the source.
    Gone,
    /// The store failed: infrastructure.
    Down(Error),
}

/// A source's object, streamed.
async fn open_source(
    fs: &FileStore,
    shown: &str,
    object: &str,
) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, OpenFailed> {
    match fs.store.get_stream(object).await {
        Ok(b) => Ok(b),
        Err(egret_storage::StorageError::NotFound(_)) => Err(OpenFailed::Gone),
        Err(e) => Err(OpenFailed::Down(store_down(UPLOAD_FILES, shown, &e))),
    }
}

#[async_trait::async_trait]
impl TaskHandler for UploadFiles {
    fn type_id(&self) -> &'static str {
        UPLOAD_FILES
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = UPLOAD_FILES;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: UploadSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        if spec.files.is_none() && spec.files_map.is_none() {
            // Kestra's message, measured (`runs.up_nothing`), naming both.
            return fail(
                "files or filesMap is required: `files` (kestra:// URIs or globs over the \
                 execution's files) or `filesMap` (file name -> kestra:// URI or contents)"
                    .to_string(),
            );
        }
        let files = match spec.files.as_ref().map(|v| ListProp::parse("files", v)) {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err((key, why))) => return fail(format!("`{key}` {why}")),
        };
        // Everything renders first; then a store outage a render met is the
        // worker's, before any of its results is used.
        let namespace = spec
            .namespace
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`namespace`: {e}"));
        let destination = spec
            .destination
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`destination`: {e}"));
        let conflict = spec
            .conflict
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`conflict`: {e}"));
        let mut render = |shown: &str, s: &str| render_text(task, shown, s);
        let files = files.map(|f| f.render("files", &mut render)).transpose();
        let files_map = spec
            .files_map
            .as_ref()
            .map(|v| render_files_map(task, v))
            .transpose();
        outage(T, task)?;
        let (namespace, destination, conflict, files, files_map) =
            match (namespace, destination, conflict, files, files_map) {
                (Ok(n), Ok(d), Ok(c), Ok(f), Ok(m)) => (n, d, c, f, m),
                (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), ..) => return fail(e),
                (.., Err(e), _) | (.., Err(e)) => return fail(e),
            };
        let conflict = match conflict.as_deref().map(parse_conflict) {
            None => Conflict::Overwrite,
            Some(Ok(c)) => c,
            Some(Err(why)) => return fail(why),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let Some(scope) = task.render.files() else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        // THE write rule, before anything is read or written.
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::NamespaceFiles,
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let dir = destination_dir(destination.as_deref().unwrap_or("/"));

        // Every entry resolved, and its path checked, before the first write.
        let mut plan = Plan::default();
        // Namespace-file references, resolved together below on one
        // connection: (what names it, the reference, the name under `dir`,
        // `None` = the referenced path).
        let mut ns_refs: Vec<(String, String, Option<String>)> = Vec::new();
        let mut globs = Vec::new();
        for (i, item) in files.unwrap_or_default().into_iter().enumerate() {
            let shown = format!("`files[{i}]`");
            if is_namespace_file_uri(&item) {
                ns_refs.push((shown, item, None));
                continue;
            }
            if !item.starts_with("kestra://") {
                globs.push(item);
                continue;
            }
            // W9a's one readability decision: the render snapshot.
            let size = match scope.0.lookup(&item) {
                Ok(meta) => meta.size,
                Err(e) => {
                    return fail(format!(
                        "{shown} names {item}, which cannot be uploaded: {}",
                        e.message()
                    ))
                }
            };
            if let Err(why) = plan_execution_file(&mut plan, &dir, &shown, item, size) {
                return fail(why);
            }
        }
        if !globs.is_empty() {
            let matcher = match compile(globs).await {
                Ok(m) => m,
                Err(why) => return fail(format!("`files`: {why}")),
            };
            let cap = fs.caps.max_namespace_files;
            let listed = {
                let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
                let readable =
                    readable_executions(&mut conn, task.workspace_id, task.execution_id).await?;
                readable_execution_files(&mut conn, task.workspace_id, &readable, cap).await?
            };
            if listed.len() as u64 > cap {
                return fail(format!(
                    "the globs of `files` match over the execution's readable files, and \
                     there are more than {cap} of them, the most one task lists \
                     (workflow.storage.max_namespace_files)"
                ));
            }
            let matched = matching(&matcher, listed, |(file, _)| {
                uri::parse(file)
                    .ok()
                    .map(|p| format!("/{}", tail_file(&p.tail)))
            })
            .await;
            for (file, size) in matched {
                if let Err(why) = plan_execution_file(&mut plan, &dir, "`files`", file, size) {
                    return fail(why);
                }
            }
        }
        for (name, value) in files_map.unwrap_or_default() {
            let shown = format!("`filesMap.{name}`");
            if is_namespace_file_uri(&value) {
                ns_refs.push((shown, value, Some(name)));
                continue;
            }
            let path = match upload_path(&dir, &name) {
                Ok(p) => p,
                Err(why) => return fail(format!("{shown}: {why}")),
            };
            let source = if value.starts_with("kestra://") {
                match scope.0.lookup(&value) {
                    Ok(meta) => Source::Execution {
                        uri: value,
                        size: meta.size,
                    },
                    Err(e) => {
                        return fail(format!(
                            "{shown} names {value}, which cannot be uploaded: {}",
                            e.message()
                        ))
                    }
                }
            } else {
                Source::Contents { shown, text: value }
            };
            if let Err(why) = plan.add(path, source) {
                return fail(why);
            }
        }
        if !ns_refs.is_empty() {
            // Task 5's path: `authorize` (Read, any namespace of the
            // workspace) and `store::resolve`, on a connection returned
            // before anything is written.
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            for (shown, reference, name) in ns_refs {
                let input =
                    match resolve_input_uri(&mut conn, fs, task.workspace_id, &own, &reference)
                        .await
                    {
                        Ok(Ok(input)) => input,
                        Ok(Err(why)) => {
                            return fail(format!(
                                "{shown} names {reference}, which cannot be uploaded: {why}"
                            ))
                        }
                        Err(e) => return split(T, e).map(Outcome::failed_with),
                    };
                // Under `files` the file keeps its namespace path; under
                // `filesMap` it takes the key.
                let name = match name {
                    Some(n) => n,
                    None => match crate::storage::scope::ns_ref(&reference, &own, None)
                        .and_then(|r| NsPath::parse_file(&r.path))
                    {
                        Ok(p) => p.as_str().to_string(),
                        Err(why) => return fail(format!("{shown} ({reference}): {why}")),
                    },
                };
                let path = match upload_path(&dir, &name) {
                    Ok(p) => p,
                    Err(why) => return fail(format!("{shown} ({reference}): {why}")),
                };
                let source = Source::Namespace {
                    uri: reference,
                    object: input.object_uri,
                    size: input.size,
                };
                if let Err(why) = plan.add(path, source) {
                    return fail(why);
                }
            }
        }
        // Spec §8.1 bounds (fix round 1, M3): counted on what was resolved,
        // before the first write.
        let cap = fs.caps.max_namespace_files;
        if plan.entries.len() as u64 > cap {
            return fail(format!(
                "{} files would be uploaded, more than the {cap} one task writes \
                 (workflow.storage.max_namespace_files)",
                plan.entries.len()
            ));
        }
        let total: u64 = plan.entries.iter().map(|(_, s)| s.size()).sum();
        let max_task = fs.caps.max_task_bytes;
        if total > max_task {
            return fail(format!(
                "the {} files to upload hold {total} bytes, more than the {max_task} one task \
                 may write (workflow.storage.max_task_bytes)",
                plan.entries.len()
            ));
        }

        let by = CreatedBy::TaskRun {
            execution: task.execution_id,
            task_run: task.run_id,
        };
        for (path, source) in plan.entries {
            // I1: a re-claim of this task run after an infrastructure failure
            // skips what its earlier attempt committed, under every conflict
            // mode: no second OVERWRITE revision, no ERROR against itself.
            // Each path has exactly one source (I2), so "the latest revision
            // was written by this task run" already proves the file is done;
            // the first attempt's bytes are kept even if the source changed.
            let earlier = {
                let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
                match latest_written(&mut conn, &grant, &path).await {
                    Ok(e) => e,
                    Err(e) => return split(T, e).map(Outcome::failed_with),
                }
            };
            if let Some((e, Some(run))) = earlier {
                if run == task.run_id && !e.deleted && !e.is_directory {
                    task.log
                        .line(
                            LogLevel::Info,
                            &format!(
                                "namespace file '{path}' was already written by an earlier \
                                 attempt of this task run (revision {}): not written again",
                                e.revision
                            ),
                        )
                        .await;
                    continue;
                }
            }
            let written = match &source {
                Source::Execution { uri: file, .. } => {
                    let object = match fs.object_uri(task.workspace_id, file) {
                        Ok(o) => o,
                        Err(why) => return fail(why),
                    };
                    let mut body = match open_source(fs, file, &object).await {
                        Ok(b) => b,
                        Err(OpenFailed::Gone) => {
                            return fail(format!(
                                "{file} is indexed but its contents are gone from internal \
                                 storage, so '{path}' was not written"
                            ))
                        }
                        Err(OpenFailed::Down(e)) => return Err(e),
                    };
                    put(&self.pool, fs, &grant, &path, &mut body, conflict, by).await
                }
                Source::Namespace {
                    uri: reference,
                    object,
                    ..
                } => {
                    let mut body = match open_source(fs, reference, object).await {
                        Ok(b) => b,
                        Err(OpenFailed::Gone) => {
                            return fail(format!(
                                "{reference} is recorded but its contents are gone from \
                                 internal storage, so '{path}' was not written"
                            ))
                        }
                        Err(OpenFailed::Down(e)) => return Err(e),
                    };
                    put(&self.pool, fs, &grant, &path, &mut body, conflict, by).await
                }
                Source::Contents { text, .. } => {
                    let mut body: &[u8] = text.as_bytes();
                    put(&self.pool, fs, &grant, &path, &mut body, conflict, by).await
                }
            };
            match written {
                Ok(Written::Written(_)) => {}
                // The same re-claim, caught under the path's lock: an earlier
                // attempt of this run committed it after the check above.
                Ok(Written::Already(e)) => {
                    task.log
                        .line(
                            LogLevel::Info,
                            &format!(
                                "namespace file '{path}' was already written by an earlier \
                                 attempt of this task run (revision {}): not written again",
                                e.revision
                            ),
                        )
                        .await;
                }
                Ok(Written::Skipped(path)) => {
                    task.log
                        .line(
                            LogLevel::Info,
                            &format!(
                                "namespace file '{path}' already exists in namespace \
                                 '{target}' and conflict is SKIP: it was not written"
                            ),
                        )
                        .await;
                }
                // Only `Conflict::IfRevision` answers this, which no task uses.
                Ok(Written::Changed { .. }) => {
                    return Err(Error::Internal(anyhow::anyhow!(
                        "{T}: namespace file '{path}' was refused as changed under a \
                         conflict mode that does not compare revisions"
                    )))
                }
                Err(e) => return split(T, e).map(Outcome::failed_with),
            }
        }
        // Kestra's run returns an empty `Output` (`runs.*.outputs` is `{}`).
        Ok(Outcome::succeeded_with(serde_json::json!({})))
    }

    /// Holds nothing past run: each write commits inside `run`, and one cut
    /// short rolls back, leaving at most an unindexed object (spec §4.2).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DownloadFiles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct DownloadSpec {
    namespace: Property<String>,
    files: Value,
    #[serde(default)]
    destination: Option<Property<String>>,
}

/// `core.namespace.DownloadFiles`: the namespace files `files` matches, each
/// copied to a file of this task run, indexed at commit.
#[derive(Debug, Clone)]
pub struct DownloadFiles {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl DownloadFiles {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

/// Kestra's `files` (`Object`, `@PluginProperty(dynamic = true)`): a string
/// is one pattern, a list's items each one; each is rendered on its own.
fn render_patterns(task: &TaskInput, v: &Value) -> std::result::Result<Vec<String>, String> {
    let must = "`files` must be a string or a list of strings (globs, or `regex:` patterns)";
    match v {
        Value::String(s) => Ok(vec![render_text(task, "files", s)?]),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Value::String(s) => render_text(task, &format!("files[{i}]"), s),
                other => Err(format!("{must}; `files[{i}]` is {other}")),
            })
            .collect(),
        Value::Null => Err(format!("{must}, and it is required")),
        other => Err(format!("{must}, not {other}")),
    }
}

/// `destination` + `rel` with exactly one `/` between (none when
/// `destination` is empty or `/`).
fn download_rel(destination: &str, rel: &str) -> String {
    let dir = destination.trim_matches('/');
    if dir.is_empty() {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    }
}

#[async_trait::async_trait]
impl TaskHandler for DownloadFiles {
    fn type_id(&self) -> &'static str {
        DOWNLOAD_FILES
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = DOWNLOAD_FILES;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: DownloadSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let namespace = spec
            .namespace
            .render(&task.render)
            .map_err(|e| format!("`namespace`: {e}"));
        let destination = spec
            .destination
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`destination`: {e}"));
        let patterns = render_patterns(task, &spec.files);
        outage(T, task)?;
        let (namespace, destination, patterns) = match (namespace, destination, patterns) {
            (Ok(n), Ok(d), Ok(p)) => (n, d.unwrap_or_default(), p),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return fail(e),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        // THE read rule: any namespace of the workspace.
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &namespace,
            Resource::NamespaceFiles,
            Access::Read,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let prefix = match super::task_storage_prefix(task, T) {
            Ok(p) => p,
            Err(why) => return fail(why),
        };
        let matcher = match compile(patterns).await {
            Ok(m) => m,
            Err(why) => return fail(format!("`files`: {why}")),
        };
        let cap = fs.caps.max_namespace_files;
        let listed = {
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            match list_live(&mut conn, &grant, None, true, cap).await {
                Ok(entries) => entries,
                Err(e) => return split(T, e).map(Outcome::failed_with),
            }
        };
        let matched: Vec<NsEntry> = matching(&matcher, listed, |e| {
            (!e.is_directory).then(|| e.path.as_str().to_string())
        })
        .await;
        let total: u64 = matched.iter().map(|e| e.size).sum();
        let max_task = fs.caps.max_task_bytes;
        if total > max_task {
            return fail(format!(
                "the {} namespace files `files` matches in namespace '{namespace}' hold {total} \
                 bytes, more than the {max_task} one task run may store \
                 (workflow.storage.max_task_bytes)",
                matched.len()
            ));
        }

        let mut stored = Vec::with_capacity(matched.len());
        let mut outputs = BTreeMap::new();
        for e in matched {
            let rel = download_rel(&destination, e.path.as_str().trim_start_matches('/'));
            let file = match uri::verbatim_file_uri(&prefix, &rel) {
                Ok(f) => f,
                Err(why) => return fail(format!("namespace file '{}': {why}", e.path)),
            };
            let object = match fs.object_uri(task.workspace_id, &file) {
                Ok(o) => o,
                Err(why) => return fail(why),
            };
            let reader = match open(fs, &grant, &e).await {
                Ok(r) => r,
                Err(err) => return split(T, err).map(Outcome::failed_with),
            };
            // One byte past the row's size: an object larger than its row is
            // told apart below without copying the rest.
            let mut body = reader.take(e.size.saturating_add(1));
            if let Err(err) = fs.store.put_stream(&object, &mut body, None).await {
                return Err(store_down(T, &file, &err));
            }
            let held = match fs.store.head(&object).await {
                Ok(Some(n)) => n,
                Ok(None) => return Err(store_down(T, &file, &"written, but head finds no object")),
                Err(err) => return Err(store_down(T, &file, &err)),
            };
            if held != e.size {
                return fail(format!(
                    "revision {} of namespace file '{}' in namespace '{namespace}' holds a \
                     different number of bytes than its row records ({}): it was not copied",
                    e.revision, e.path, e.size
                ));
            }
            stored.push(StoredFile {
                uri: file.clone(),
                size: held,
            });
            outputs.insert(e.path.as_str().to_string(), Value::String(file));
        }
        Ok(Outcome {
            files: stored,
            ..Outcome::succeeded_with(serde_json::json!({ "files": outputs }))
        })
    }

    /// Holds nothing past run: an upload cut short leaves an unindexed
    /// object under the run's prefix, an orphan the pruner sweeps (W9a §10).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DeleteFiles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct DeleteSpec {
    #[serde(default)]
    namespace: Option<Property<String>>,
    files: Value,
    #[serde(rename = "deleteParentFolder", default)]
    delete_parent_folder: Option<Property<bool>>,
}

/// `core.namespace.DeleteFiles`: the live namespace files `files` matches in
/// the flow's own namespace, each soft-deleted (§4.3, R1).
#[derive(Debug, Clone)]
pub struct DeleteFiles {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl DeleteFiles {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

/// Every parent directory of `deleted`, each once, deepest first (more
/// segments first; at one depth, in byte order): a parent is judged empty
/// only after every directory under it was.
fn parents_deepest_first(deleted: &[NsPath]) -> Vec<NsPath> {
    let mut seen = std::collections::BTreeSet::new();
    let mut parents: Vec<NsPath> = deleted
        .iter()
        .flat_map(NsPath::parents)
        .filter(|p| seen.insert(p.as_str().to_string()))
        .collect();
    let depth = |p: &NsPath| p.as_str().matches('/').count();
    parents.sort_by(|a, b| {
        depth(b)
            .cmp(&depth(a))
            .then_with(|| a.as_str().cmp(b.as_str()))
    });
    parents
}

#[async_trait::async_trait]
impl TaskHandler for DeleteFiles {
    fn type_id(&self) -> &'static str {
        DELETE_FILES
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = DELETE_FILES;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: DeleteSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let namespace = spec
            .namespace
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`namespace`: {e}"));
        // Kestra's `files` is an `Object`, as `DownloadFiles`'.
        let patterns = render_patterns(task, &spec.files);
        let parents = spec
            .delete_parent_folder
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`deleteParentFolder`: {e}"));
        outage(T, task)?;
        let (namespace, patterns, parents) = match (namespace, patterns, parents) {
            (Ok(n), Ok(p), Ok(d)) => (n, p, d.unwrap_or(false)),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return fail(e),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        // THE write rule, before anything is read or deleted.
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::NamespaceFiles,
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let matcher = match compile(patterns).await {
            Ok(m) => m,
            Err(why) => return fail(format!("`files`: {why}")),
        };
        let cap = fs.caps.max_namespace_files;
        let listed = {
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            match list_live(&mut conn, &grant, None, true, cap).await {
                Ok(entries) => entries,
                Err(e) => return split(T, e).map(Outcome::failed_with),
            }
        };
        // Files only, as Kestra's `findAllFilesMatching`.
        let matched: Vec<NsEntry> = matching(&matcher, listed, |e| {
            (!e.is_directory).then(|| e.path.as_str().to_string())
        })
        .await;
        let mut deleted = Vec::with_capacity(matched.len());
        for e in matched {
            match soft_delete(&self.pool, &grant, &e.path).await {
                Ok(marked) if !marked.is_empty() => deleted.push(e.path),
                // Deleted meanwhile: nothing of it is this run's.
                Ok(_) => {}
                Err(err) => return split(T, err).map(Outcome::failed_with),
            }
        }
        let mut folders = 0usize;
        if parents {
            // Kestra's rule (the user's ruling of 2026-10-06, M1): a parent
            // goes when no LIVE FILE is left at any depth under it, with its
            // empty subdirectories; judged and deleted in one locked
            // transaction (M2), so a concurrent write into it keeps it.
            for dir in parents_deepest_first(&deleted) {
                match soft_delete_if_empty(&self.pool, &grant, &dir).await {
                    Ok(marked) => {
                        folders += marked.iter().filter(|e| e.is_directory).count();
                    }
                    Err(e) => return split(T, e).map(Outcome::failed_with),
                }
            }
        }
        task.log
            .line(
                LogLevel::Info,
                &format!(
                    "deleted {} namespace files and {folders} directories left empty from \
                     namespace '{target}' (soft: their revisions stay until PurgeFiles)",
                    deleted.len()
                ),
            )
            .await;
        // Kestra's run returns an empty `Output` (`runs.*.outputs` is `{}`).
        Ok(Outcome::succeeded_with(serde_json::json!({})))
    }

    /// Holds nothing past run: each soft delete commits on its own.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PurgeFiles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PurgeSpec {
    #[serde(rename = "filePattern", default)]
    file_pattern: Option<Property<String>>,
    #[serde(default)]
    namespaces: Option<Value>,
    #[serde(rename = "namespacePattern", default)]
    namespace_pattern: Option<Property<String>>,
    #[serde(default)]
    behavior: Option<Property<BehaviorSpec>>,
    #[serde(rename = "includeChildNamespaces", default)]
    include_child_namespaces: Option<Property<bool>>,
}

/// Kestra's `FilesPurgeBehavior`, whose one subtype is `Version` (JSON
/// type name `version`, `FilesPurgeBehavior.java`): `keepAmount` or
/// `before`, never both (`FilesVersionBehaviorValidator`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BehaviorSpec {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(rename = "keepAmount", default)]
    keep_amount: Option<Value>,
    #[serde(default)]
    before: Option<Value>,
}

/// Which revisions of a live path survive a purge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    /// The N highest (`keepAmount`).
    Newest(u64),
    /// Every revision created AFTER the instant, and the highest whatever
    /// its age (`before`): Kestra's `updated <= before` purges one created
    /// exactly at it (fix round 1, C3/M3).
    Before(DateTime<Utc>),
}

/// `behavior`, rendered, to a [`Keep`]; absent = Kestra's default
/// `{type: version, keepAmount: 1}`.
fn keep_rule(b: Option<BehaviorSpec>) -> std::result::Result<Keep, String> {
    let Some(b) = b else {
        return Ok(Keep::Newest(1));
    };
    if b.kind.as_deref() != Some("version") {
        return Err(format!(
            "`behavior.type` must be \"version\", the one files purge behavior, and it is {}",
            b.kind.map_or("missing".to_string(), |k| format!("{k:?}"))
        ));
    }
    let keep = match b.keep_amount {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
        Some(Value::String(s)) if s.trim().parse::<u64>().is_ok() => s.trim().parse().ok(),
        Some(other) => {
            return Err(format!(
                "`behavior.keepAmount` must be a whole number, 0 or more, and it is {other}"
            ))
        }
    };
    let before = match b.before {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(
            DateTime::parse_from_rfc3339(s.trim())
                .map(|t| t.with_timezone(&Utc))
                .map_err(|e| {
                    format!(
                        "`behavior.before` must be an ISO 8601 instant such as \
                         2025-01-01T00:00:00Z, and it is {s:?}: {e}"
                    )
                })?,
        ),
        Some(other) => {
            return Err(format!(
                "`behavior.before` must be an ISO 8601 instant such as 2025-01-01T00:00:00Z, \
                 and it is {other}"
            ))
        }
    };
    match (keep, before) {
        (Some(_), Some(_)) => {
            Err("`behavior`: Cannot set both 'before' and 'keepAmount' properties".to_string())
        }
        (Some(n), None) => Ok(Keep::Newest(n)),
        (None, Some(t)) => Ok(Keep::Before(t)),
        // The user's ruling (2026-10-06, C2): refused, where Kestra purges
        // every revision of every matched path, the live latest included.
        (None, None) => Err(
            "`behavior` sets neither `keepAmount` nor `before`, so it names nothing to keep: \
             set `keepAmount` (the newest revisions kept per file) or `before` (an instant); \
             `keepAmount: 0` purges every revision"
                .to_string(),
        ),
    }
}

/// The revisions of ONE path (`revs`, ascending, every row of it) a purge
/// removes. A path whose latest revision is soft-deleted loses every
/// revision, whatever `keep` says (spec §2, §8.4: the user's decision).
fn revisions_to_purge(revs: &[NsEntry], keep: Keep) -> Vec<u32> {
    let Some(latest) = revs.last() else {
        return Vec::new();
    };
    if latest.deleted {
        return revs.iter().map(|e| e.revision).collect();
    }
    let older = &revs[..revs.len() - 1];
    match keep {
        Keep::Newest(n) => {
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            revs[..revs.len().saturating_sub(n)]
                .iter()
                .map(|e| e.revision)
                .collect()
        }
        // Never the latest: `older` holds every revision but it.
        Keep::Before(t) => older
            .iter()
            .filter(|e| e.created_at <= t)
            .map(|e| e.revision)
            .collect(),
    }
}

/// The namespaces a purge covers (spec §8.4), before `authorize`:
/// - `pattern`: Kestra's wildcard over `candidates` -- the namespaces holding
///   files that are the flow's own or its children, ALREADY restricted by
///   `store::namespaces_with_files` -- so it selects among them and never
///   names another (R2; the coordinator's ruling B);
/// - `listed`: each, plus (with `include_children`) every candidate that is
///   its child on a dot boundary;
/// - neither: the flow's own, plus (with `include_children`) its children.
///
/// Sorted, each once. CPU on a pattern of any length: callers run it on the
/// blocking pool.
fn purge_namespaces(
    own: &str,
    listed: Option<&[String]>,
    pattern: Option<&str>,
    include_children: bool,
    candidates: &[String],
) -> Vec<String> {
    let child_of = |ns: &str, parent: &str| {
        ns.strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('.'))
    };
    let mut out = std::collections::BTreeSet::new();
    if let Some(pattern) = pattern {
        out.extend(
            candidates
                .iter()
                .filter(|ns| wildcard_match(pattern, ns))
                .cloned(),
        );
        return out.into_iter().collect();
    }
    let roots: Vec<String> = match listed {
        Some(l) => l.to_vec(),
        None => vec![own.to_string()],
    };
    for root in &roots {
        if include_children {
            out.extend(candidates.iter().filter(|ns| child_of(ns, root)).cloned());
        }
        out.insert(root.clone());
    }
    out.into_iter().collect()
}

/// One page's complete path groups (`(id, row)`, by path then revision) to
/// the rows [`revisions_to_purge`] takes from each path `file_pattern`
/// matches (Commons-IO wildcard on the path without its leading `/`, spec
/// §8.4), as `(id, revision)`. CPU on a pattern of any length: callers run
/// it on the blocking pool.
fn page_targets(
    rows: Vec<(i64, NsEntry)>,
    file_pattern: Option<&str>,
    keep: Keep,
) -> Vec<(NsPath, Vec<(i64, u32)>)> {
    let mut out = Vec::new();
    for group in rows.chunk_by(|a, b| a.1.path == b.1.path) {
        let path = &group[0].1.path;
        if file_pattern.is_some_and(|p| !wildcard_matches(p, path)) {
            continue;
        }
        let entries: Vec<NsEntry> = group.iter().map(|(_, e)| e.clone()).collect();
        let take = revisions_to_purge(&entries, keep);
        if take.is_empty() {
            continue;
        }
        let rows = group
            .iter()
            .filter(|(_, e)| take.contains(&e.revision))
            .map(|(id, e)| (*id, e.revision))
            .collect();
        out.push((path.clone(), rows));
    }
    out
}

/// Whether `file_pattern` takes `path`, on the blocking pool.
async fn pattern_takes(file_pattern: &Option<Arc<str>>, path: &NsPath) -> bool {
    let Some(p) = file_pattern.clone() else {
        return true;
    };
    let path = path.clone();
    tokio::task::spawn_blocking(move || wildcard_matches(&p, &path))
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// The purge itself (spec §8.4; fix round 1, I1-I3), namespace by namespace:
///
/// 1. **Files**, in keyset pages of at most `cap + 1` rows by
///    `(path, revision)`. Each page's COMPLETE path groups are judged
///    ([`page_targets`]) and purged at once, path by path, by row id
///    ([`purge_rows`]); the last, possibly partial group starts the next
///    page. Only ONE path holding more than `cap` revisions that
///    `file_pattern` takes is refused, naming it (`Ok(Err)`); one it does not
///    take is skipped. No other count bounds a purge.
/// 2. **Directories**, deepest first, in pages of at most `cap + 1`: a
///    soft-deleted one -- and with `keepAmount: 0` a live one -- that
///    `file_pattern` takes is removed when nothing is left under it, decided
///    under its lock ([`purge_empty_directory`]).
///
/// Each path is purged atomically (rows, commit, objects); a failure leaves
/// the paths before it purged, and a re-claim recomputes from the rows as
/// they then are. Returns the rows removed, directories included.
async fn run_purge(
    pool: PgPool,
    fs: Arc<FileStore>,
    grants: Vec<Grant>,
    file_pattern: Option<Arc<str>>,
    keep: Keep,
    cap: u64,
) -> std::result::Result<std::result::Result<u64, String>, NsError> {
    let mut purged = 0u64;
    for grant in &grants {
        let mut after: Option<(NsPath, u32)> = None;
        loop {
            let mut page = {
                let mut conn = pool.acquire().await.map_err(|e| {
                    NsError::Db(Error::Internal(anyhow::anyhow!(
                        "the database is unavailable: {e}"
                    )))
                })?;
                file_rows_page(&mut conn, grant, after.as_ref().map(|(p, r)| (p, *r)), cap).await?
            };
            let full = page.len() as u64 > cap;
            if full {
                let last_path = page.last().map(|(_, e)| e.path.clone()).expect("full");
                if page.iter().all(|(_, e)| e.path == last_path) {
                    // One path alone holds more than `cap` revisions.
                    if pattern_takes(&file_pattern, &last_path).await {
                        return Ok(Err(format!(
                            "namespace file '{last_path}' in namespace '{}' has more than {cap} \
                             revisions, the most one task lists for one file \
                             (workflow.storage.max_namespace_files); the files before it were \
                             purged",
                            grant.namespace()
                        )));
                    }
                    after = Some((last_path, u32::MAX));
                    continue;
                }
                // The last group may be partial: it starts the next page.
                page.retain(|(_, e)| e.path != last_path);
            }
            let next = page.last().map(|(_, e)| (e.path.clone(), u32::MAX));
            let pattern = file_pattern.clone();
            let targets =
                tokio::task::spawn_blocking(move || page_targets(page, pattern.as_deref(), keep))
                    .await
                    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
            for (path, rows) in targets {
                purged += purge_rows(&pool, &fs, grant, &path, &rows).await?;
            }
            match (full, next) {
                (true, Some(n)) => after = Some(n),
                _ => break,
            }
        }
        let live_too = keep == Keep::Newest(0);
        let mut before: Option<NsPath> = None;
        loop {
            let mut page = {
                let mut conn = pool.acquire().await.map_err(|e| {
                    NsError::Db(Error::Internal(anyhow::anyhow!(
                        "the database is unavailable: {e}"
                    )))
                })?;
                purgeable_directories_page(&mut conn, grant, before.as_ref(), live_too, cap).await?
            };
            let full = page.len() as u64 > cap;
            if full {
                page.pop();
            }
            before = page.last().map(|e| e.path.clone());
            for dir in page {
                if pattern_takes(&file_pattern, &dir.path).await {
                    purged += purge_empty_directory(&pool, grant, &dir.path, live_too).await?;
                }
            }
            if !full || before.is_none() {
                break;
            }
        }
    }
    Ok(Ok(purged))
}

/// `core.namespace.PurgeFiles`: a hard purge of old revisions (and of
/// soft-deleted paths) in the flow's own namespace and its children.
#[derive(Debug, Clone)]
pub struct PurgeFiles {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl PurgeFiles {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for PurgeFiles {
    fn type_id(&self) -> &'static str {
        PURGE_FILES
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = PURGE_FILES;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: PurgeSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let namespaces = match spec
            .namespaces
            .as_ref()
            .filter(|v| !v.is_null())
            .map(|v| ListProp::parse("namespaces", v))
        {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err((key, why))) => return fail(format!("`{key}` {why}")),
        };
        let mut render = |shown: &str, s: &str| render_text(task, shown, s);
        let namespaces = namespaces
            .map(|n| n.render("namespaces", &mut render))
            .transpose();
        let pattern = spec
            .namespace_pattern
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`namespacePattern`: {e}"));
        let file_pattern = spec
            .file_pattern
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`filePattern`: {e}"));
        let children = spec
            .include_child_namespaces
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`includeChildNamespaces`: {e}"));
        let behavior = spec
            .behavior
            .as_ref()
            .map(|p| p.render(&task.render))
            .transpose()
            .map_err(|e| format!("`behavior`: {e}"));
        outage(T, task)?;
        let (namespaces, pattern, file_pattern, children, behavior) =
            match (namespaces, pattern, file_pattern, children, behavior) {
                (Ok(n), Ok(p), Ok(f), Ok(c), Ok(b)) => (n, p, f, c.unwrap_or(true), b),
                (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), ..) => return fail(e),
                (.., Err(e), _) | (.., Err(e)) => return fail(e),
            };
        // Kestra's emptiness tests: an empty list and a blank pattern are absent.
        let listed = namespaces.filter(|l| !l.is_empty());
        let pattern = pattern.filter(|p| !p.trim().is_empty());
        let file_pattern = file_pattern.filter(|p| !p.trim().is_empty());
        // Kestra's matching is kept (§8.4): the pattern is matched against
        // each path WITHOUT its leading '/', so one that starts with '/'
        // takes nothing. Said once, at WARN (W9b final review M-8).
        if let Some(p) = file_pattern.as_deref().filter(|p| p.starts_with('/')) {
            task.log
                .line(
                    LogLevel::Warn,
                    &format!(
                        "`filePattern` is matched against each path without its leading '/'; \
                         '{p}' matches nothing, write '{}'",
                        p.trim_start_matches('/')
                    ),
                )
                .await;
        }
        if listed.is_some() && pattern.is_some() {
            // Kestra's text (`PurgeTask.findNamespaces`), naming both.
            return fail(
                "Properties `namespaces` and `namespacePattern` can't be used at the same time \
                 — use one or the other."
                    .to_string(),
            );
        }
        if let Some(l) = &listed {
            if l.len() > MAX_NAMESPACES {
                return fail(format!(
                    "`namespaces` names {} namespaces, more than the {MAX_NAMESPACES} one task \
                     may name",
                    l.len()
                ));
            }
        }
        let keep = match keep_rule(behavior) {
            Ok(k) => k,
            Err(why) => return fail(why),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };

        // A listed namespace passes the purge rule first: one outside it is
        // refused by name before anything is listed under it.
        for ns in listed.iter().flatten() {
            if let Err(why) = authorize(
                task.workspace_id,
                Actor::Flow { namespace: &own },
                ns,
                Resource::NamespaceFiles,
                Access::Purge,
            ) {
                return fail(why);
            }
        }
        // The candidates: the namespaces holding files under the roots --
        // own for a pattern or the default, each LISTED one otherwise (fix
        // round 1, M5) -- restricted by the store to a root and its children
        // on a dot, at most MAX_NAMESPACES of them.
        let candidates = if pattern.is_some() || children {
            let roots: Vec<String> = match (&pattern, &listed) {
                (None, Some(l)) => l.clone(),
                _ => vec![own.clone()],
            };
            let too_many = || {
                format!(
                    "the namespaces under {} hold files in more than {MAX_NAMESPACES} \
                     namespaces, the most one task purges: list the ones to purge in \
                     `namespaces` with `includeChildNamespaces: false`, or narrow \
                     `namespacePattern`",
                    roots
                        .iter()
                        .map(|r| format!("'{r}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let mut found = std::collections::BTreeSet::new();
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            for root in &roots {
                let under = match namespaces_with_files(
                    &mut conn,
                    task.workspace_id,
                    root,
                    MAX_NAMESPACES as u64,
                )
                .await
                {
                    Ok(f) => f,
                    Err(e) => return split(T, e).map(Outcome::failed_with),
                };
                found.extend(under);
                if found.len() > MAX_NAMESPACES {
                    return fail(too_many());
                }
            }
            found.into_iter().collect()
        } else {
            Vec::new()
        };
        let resolved = {
            let own = own.clone();
            let listed = listed.clone();
            let pattern = pattern.clone();
            tokio::task::spawn_blocking(move || {
                purge_namespaces(
                    &own,
                    listed.as_deref(),
                    pattern.as_deref(),
                    children,
                    &candidates,
                )
            })
            .await
            .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
        };
        if resolved.len() > MAX_NAMESPACES {
            return fail(format!(
                "{} namespaces would be purged, more than the {MAX_NAMESPACES} one task purges",
                resolved.len()
            ));
        }
        // THE purge rule, for EVERY namespace before anything is purged: the
        // first refusal fails the task naming it.
        let mut grants = Vec::with_capacity(resolved.len());
        for ns in &resolved {
            // Cannot refuse by construction (listed namespaces were authorized
            // before listing; every other input is a child of a permitted
            // root); kept as defense in depth.
            match authorize(
                task.workspace_id,
                Actor::Flow { namespace: &own },
                ns,
                Resource::NamespaceFiles,
                Access::Purge,
            ) {
                Ok(g) => grants.push(g),
                Err(why) => return fail(why),
            }
        }

        task.log
            .line(
                LogLevel::Info,
                &format!(
                    "purging {} namespaces: [{}]",
                    resolved.len(),
                    resolved.join(", ")
                ),
            )
            .await;
        // Detached (spec §4.4): once it has started, the purge runs to
        // completion even if this run is dropped (a timeout, a shutdown).
        let file_pattern: Option<Arc<str>> = file_pattern.map(Arc::from);
        let cap = fs.caps.max_namespace_files;
        let purged = tokio::spawn(run_purge(
            self.pool.clone(),
            fs.clone(),
            grants,
            file_pattern,
            keep,
            cap,
        ))
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("{T}: the purge stopped: {e}")))?;
        let purged = match purged {
            Ok(Ok(n)) => n,
            Ok(Err(why)) => return fail(why),
            Err(e) => return split(T, e).map(Outcome::failed_with),
        };
        task.log
            .line(LogLevel::Info, &format!("purged {purged} files"))
            .await;
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "size": purged }),
        ))
    }

    /// Holds nothing past run: the purge is detached and finishes on its own.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_is_a_directory_joined_with_one_slash() {
        for (dest, name, want) in [
            ("up2", "m.txt", "/up2/m.txt"),
            ("/up/", "m.txt", "/up/m.txt"),
            ("/up/", "sub/n.txt", "/up/sub/n.txt"),
            ("/", "rootm.txt", "/rootm.txt"),
            ("", "rootm.txt", "/rootm.txt"),
            ("/in/", "/content.txt", "/in/content.txt"),
        ] {
            let got = upload_path(&destination_dir(dest), name).unwrap();
            assert_eq!(got.as_str(), want, "{dest:?} + {name:?}");
        }
        // `//` keeps its empty segment, which THE path rule refuses by name.
        let err = upload_path(&destination_dir("//"), "m.txt").unwrap_err();
        assert!(err.contains("//m.txt"), "{err}");
    }

    #[test]
    fn download_destination_joins_with_one_slash() {
        for (dest, rel, want) in [
            ("", "up/m.txt", "up/m.txt"),
            ("/", "up/m.txt", "up/m.txt"),
            ("dl/", "globbed/top.txt", "dl/globbed/top.txt"),
            ("dl", "globbed/top.txt", "dl/globbed/top.txt"),
            ("/dl/", "x.txt", "dl/x.txt"),
        ] {
            assert_eq!(download_rel(dest, rel), want, "{dest:?} + {rel:?}");
        }
    }

    /// M4: a run with no render scope (so no running flow's namespace) is
    /// refused by both, naming why, before anything is read or written.
    #[tokio::test]
    async fn a_run_that_is_not_a_flows_task_run_is_refused_naming_it() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        let fs = crate::storage::install_mem_for_tests();
        let cases: [(Arc<dyn TaskHandler>, Value); 2] = [
            (
                Arc::new(UploadFiles::new(pool.clone(), Some(fs.clone()))),
                serde_json::json!({ "filesMap": { "a.txt": "contents" } }),
            ),
            (
                Arc::new(DownloadFiles::new(pool, Some(fs))),
                serde_json::json!({ "namespace": "a.b", "files": ["**"] }),
            ),
        ];
        for (handler, spec) in cases {
            let task = super::super::testing::input(handler.type_id(), spec);
            assert!(task.render.files().is_none(), "the precondition");
            let out = handler
                .run(&task)
                .await
                .expect("an author's failure, not Err");
            let message = out.message.unwrap_or_default();
            assert!(
                message.contains("not a task run of a flow's execution"),
                "{}: {message}",
                handler.type_id()
            );
        }
    }

    fn entry(path: &str, revision: u32, deleted: bool, secs: i64) -> NsEntry {
        NsEntry {
            workspace: Uuid::nil(),
            namespace: "a.b".into(),
            path: NsPath::parse_file(path).unwrap(),
            revision,
            is_directory: false,
            size: 1,
            deleted,
            created_at: DateTime::from_timestamp(secs, 0).unwrap(),
        }
    }

    #[test]
    fn keep_amount_keeps_the_highest_and_before_never_takes_the_latest() {
        let revs: Vec<NsEntry> = (1..=4).map(|r| entry("/f", r, false, r as i64)).collect();
        assert_eq!(revisions_to_purge(&revs, Keep::Newest(1)), [1, 2, 3]);
        assert_eq!(revisions_to_purge(&revs, Keep::Newest(3)), [1]);
        assert_eq!(revisions_to_purge(&revs, Keep::Newest(9)), [] as [u32; 0]);
        assert_eq!(revisions_to_purge(&revs, Keep::Newest(0)), [1, 2, 3, 4]);
        let at = |s| Keep::Before(DateTime::from_timestamp(s, 0).unwrap());
        // At or before the instant, as Kestra's `<=` (C3): a tie is purged.
        assert_eq!(revisions_to_purge(&revs, at(2)), [1, 2]);
        assert_eq!(revisions_to_purge(&revs, at(1)), [1]);
        assert_eq!(revisions_to_purge(&revs, at(0)), [] as [u32; 0]);
        // The latest never, even when older.
        assert_eq!(revisions_to_purge(&revs, at(100)), [1, 2, 3]);
        // M6: a soft-deleted MIDDLE revision counts toward keepAmount, as
        // Kestra's skip over every version.
        let mut middle = revs[..3].to_vec();
        middle[1].deleted = true;
        assert_eq!(revisions_to_purge(&middle, Keep::Newest(2)), [1]);
        // A soft-deleted latest: every revision, whatever the rule.
        let mut deleted = revs.clone();
        deleted[3].deleted = true;
        for keep in [Keep::Newest(1), Keep::Newest(9), at(0)] {
            assert_eq!(revisions_to_purge(&deleted, keep), [1, 2, 3, 4], "{keep:?}");
        }
    }

    #[test]
    fn behavior_is_kestras_version_with_one_of_its_two_properties() {
        let parse = |v: Value| keep_rule(Some(serde_json::from_value(v).unwrap()));
        assert_eq!(keep_rule(None), Ok(Keep::Newest(1)));
        assert_eq!(
            parse(serde_json::json!({"type": "version", "keepAmount": 2})),
            Ok(Keep::Newest(2))
        );
        assert_eq!(
            parse(serde_json::json!({"type": "version", "keepAmount": "3"})),
            Ok(Keep::Newest(3))
        );
        assert_eq!(
            parse(serde_json::json!({"type": "version", "before": "2026-10-05T03:12:45Z"})),
            Ok(Keep::Before(
                "2026-10-05T03:12:45Z".parse::<DateTime<Utc>>().unwrap()
            ))
        );
        for (v, needle) in [
            (
                serde_json::json!({"type": "version", "keepAmount": 1, "before": "2026-10-05T03:12:45Z"}),
                "Cannot set both",
            ),
            (serde_json::json!({"type": "version"}), "neither"),
            (serde_json::json!({"keepAmount": 1}), "`behavior.type`"),
            (
                serde_json::json!({"type": "versions", "keepAmount": 1}),
                "\"versions\"",
            ),
            (
                serde_json::json!({"type": "version", "keepAmount": -1}),
                "`behavior.keepAmount`",
            ),
            (
                serde_json::json!({"type": "version", "before": "yesterday"}),
                "`behavior.before`",
            ),
        ] {
            let err = parse(v.clone()).unwrap_err();
            assert!(err.contains(needle), "{v}: {err}");
        }
    }

    #[test]
    fn purge_namespaces_take_children_on_a_dot_and_patterns_only_from_candidates() {
        // As the store answers them: own and its children, `a.b.cd` among
        // them -- a child of `a.b`, but not of `a.b.c`.
        let candidates: Vec<String> = ["a.b", "a.b.c", "a.b.c.d", "a.b.cd"]
            .into_iter()
            .map(String::from)
            .collect();
        let ns = |v: Vec<String>| v.join(" ");
        assert_eq!(
            ns(purge_namespaces("a.b", None, None, true, &candidates)),
            "a.b a.b.c a.b.c.d a.b.cd"
        );
        assert_eq!(
            ns(purge_namespaces("a.b", None, None, false, &candidates)),
            "a.b"
        );
        let listed = ["a.b.c".to_string()];
        assert_eq!(
            ns(purge_namespaces(
                "a.b",
                Some(&listed),
                None,
                true,
                &candidates
            )),
            "a.b.c a.b.c.d"
        );
        // A pattern selects among the candidates, never beyond them.
        assert_eq!(
            ns(purge_namespaces("a.b", None, Some("*"), false, &candidates)),
            "a.b a.b.c a.b.c.d a.b.cd"
        );
        assert_eq!(
            ns(purge_namespaces(
                "a.b",
                None,
                Some("a.b.c"),
                true,
                &candidates
            )),
            "a.b.c"
        );
        assert_eq!(
            ns(purge_namespaces(
                "a.b",
                None,
                Some("x.*"),
                true,
                &candidates
            )),
            ""
        );
    }

    #[test]
    fn parents_are_judged_deepest_first_each_once() {
        let deleted: Vec<NsPath> = ["/p/q/r/x.txt", "/p/q/y.txt", "/s/z.txt"]
            .into_iter()
            .map(|p| NsPath::parse_file(p).unwrap())
            .collect();
        let got: Vec<String> = parents_deepest_first(&deleted)
            .into_iter()
            .map(|p| p.as_str().to_string())
            .collect();
        assert_eq!(got, ["/p/q/r/", "/p/q/", "/p/", "/s/"]);
    }

    #[test]
    fn conflict_takes_kestras_three_values_only() {
        assert_eq!(parse_conflict("OVERWRITE"), Ok(Conflict::Overwrite));
        assert_eq!(parse_conflict("ERROR"), Ok(Conflict::Error));
        assert_eq!(parse_conflict("SKIP"), Ok(Conflict::Skip));
        let err = parse_conflict("skip").unwrap_err();
        assert!(
            err.contains("`conflict`") && err.contains("\"skip\""),
            "{err}"
        );
    }
}
