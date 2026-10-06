//! `io.kestra.plugin.core.namespace.{UploadFiles, DownloadFiles}` -- the
//! core namespace-file tasks W9b Task 6 ports (W9b spec §8.1, §8.2). Task 7
//! adds `DeleteFiles` and `PurgeFiles` here.
//!
//! # The shapes are Kestra's, measured, except the chosen cells
//!
//! Each class's properties (name, `required`, `default`) and outputs are
//! [`DECLARED`], and `tests/nsfiles_tasks.rs` holds that table against
//! `oracle-exec/w9b-namespace-files.json`'s `tasks.{UploadFiles,
//! DownloadFiles}.schema` (egret's answer, the top-level value of each cell).
//!
//! | class | properties | outputs |
//! |---|---|---|
//! | `UploadFiles` | `conflict` (default `OVERWRITE`), `destination` (default `/`), `files`, `filesMap`, `namespace` (default the flow's) | `files` (declared; never set, as Kestra's run) |
//! | `DownloadFiles` | `destination` (default `""`), `files` (required), `namespace` (required) | `files`: path -> `kestra://` URI |
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
//! - **Every `files` and `filesMap` URI is resolved before the first write**,
//!   and one that does not resolve fails the task naming it, writing nothing
//!   (Kestra skips it silently). A `filesMap` value that is not a
//!   `kestra://` URI is the file's contents.
//! - **`destination` is a directory**: a missing leading `/` is added, one
//!   trailing `/` dropped, and each name joined with exactly one `/`
//!   (`up2` + `m.txt` -> `/up2/m.txt`; Kestra wrote `/up2m.txt`). A file
//!   `files` matched keeps its path under its task run (`content.txt`, or
//!   `sub/x.txt` for an `outputDirectory` file). The same `(path, source)`
//!   named twice in `files` (a URI and a glob that also matches it) is
//!   written once.
//! - **`DownloadFiles` joins `destination` and the path with exactly one
//!   `/`** (`dl/` or `dl` + `/up/m.txt` -> `…/{taskRunId}/dl/up/m.txt`; `""`
//!   -> `…/{taskRunId}/up/m.txt`). Kestra concatenates (`dl` would give
//!   `dlup/m.txt`).
//!
//! # Bounds and errors (spec §7, §11)
//!
//! The patterns are compiled off the async runtime under `glob`'s count and
//! per-list budget; matching runs on the blocking pool too, and no database
//! connection is held while it does. `UploadFiles` lists at most
//! `max_namespace_files + 1` readable execution files ([`readable_execution_files`]),
//! `DownloadFiles` at most `max_namespace_files + 1` namespace rows
//! (`list_live`); the `+1` is the refusal, naming the cap. `DownloadFiles`
//! refuses matches whose rows hold more than `max_task_bytes` together.
//! `NsError::{Store, Db}` and a store that fails are infrastructure: `Err`,
//! so the lease lapses and the run is re-claimed with no attempt spent;
//! everything else is a failed task naming the path, the property or the cap.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
use crate::storage::nsfiles::access::{authorize, Access, Actor};
use crate::storage::nsfiles::glob::PathMatcher;
use crate::storage::nsfiles::path::NsPath;
use crate::storage::nsfiles::stage::ListProp;
use crate::storage::nsfiles::store::{list_live, open, put, Conflict, NsEntry, Written};
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
];

/// Register the namespace tasks, which need the pool. `runtime::handlers_for`
/// calls this for every deployment, beside `storage::register`.
pub fn register(
    handlers: &mut super::super::Handlers,
    pool: PgPool,
    files: Option<Arc<FileStore>>,
) {
    handlers.add_publishable(Arc::new(UploadFiles::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(DownloadFiles::new(pool, files)));
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

/// The running flow's namespace: the render scope's (Task 5's
/// `flow_namespace`) and nothing else.
fn flow_namespace(task: &TaskInput) -> Option<String> {
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
fn split(type_id: &str, e: NsError) -> Result<String> {
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
struct UploadSpec {
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
    /// A readable execution file.
    Execution { uri: String },
    /// A `filesMap` value that is not a URI: the file's contents.
    Contents(String),
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
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let dir = destination_dir(destination.as_deref().unwrap_or("/"));

        // Every entry resolved, and its path checked, before the first write.
        let mut planned: Vec<(NsPath, Source)> = Vec::new();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut plan_execution_file =
            |planned: &mut Vec<(NsPath, Source)>, shown: &str, file: &str| {
                let parsed = uri::parse(file).map_err(|why| format!("{shown}: {file}: {why}"))?;
                let path = upload_path(&dir, tail_file(&parsed.tail))
                    .map_err(|why| format!("{shown} ({file}): {why}"))?;
                if seen.insert((path.as_str().to_string(), file.to_string())) {
                    planned.push((
                        path,
                        Source::Execution {
                            uri: file.to_string(),
                        },
                    ));
                }
                Ok::<_, String>(())
            };
        let mut globs = Vec::new();
        for (i, item) in files.unwrap_or_default().into_iter().enumerate() {
            if !item.starts_with("kestra://") {
                globs.push(item);
                continue;
            }
            // W9a's one readability decision: the render snapshot.
            if let Err(e) = scope.0.lookup(&item) {
                return fail(format!(
                    "`files[{i}]` names {item}, which cannot be uploaded: {}",
                    e.message()
                ));
            }
            if let Err(why) = plan_execution_file(&mut planned, &format!("`files[{i}]`"), &item) {
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
            for (file, _) in matched {
                if let Err(why) = plan_execution_file(&mut planned, "`files`", &file) {
                    return fail(why);
                }
            }
        }
        for (name, value) in files_map.unwrap_or_default() {
            let shown = format!("`filesMap.{name}`");
            let path = match upload_path(&dir, &name) {
                Ok(p) => p,
                Err(why) => return fail(format!("{shown}: {why}")),
            };
            if value.starts_with("kestra://") {
                if let Err(e) = scope.0.lookup(&value) {
                    return fail(format!(
                        "{shown} names {value}, which cannot be uploaded: {}",
                        e.message()
                    ));
                }
                planned.push((path, Source::Execution { uri: value }));
            } else {
                planned.push((path, Source::Contents(value)));
            }
        }

        let by = CreatedBy::Execution(task.execution_id);
        for (path, source) in planned {
            let written = match &source {
                Source::Execution { uri: file } => {
                    let object = match fs.object_uri(task.workspace_id, file) {
                        Ok(o) => o,
                        Err(why) => return fail(why),
                    };
                    let mut body = match fs.store.get_stream(&object).await {
                        Ok(b) => b,
                        Err(egret_storage::StorageError::NotFound(_)) => {
                            return fail(format!(
                                "{file} is indexed but its contents are gone from internal \
                                 storage, so '{path}' was not written"
                            ))
                        }
                        Err(e) => return Err(store_down(T, file, &e)),
                    };
                    put(&self.pool, fs, &grant, &path, &mut body, conflict, by).await
                }
                Source::Contents(text) => {
                    let mut body: &[u8] = text.as_bytes();
                    put(&self.pool, fs, &grant, &path, &mut body, conflict, by).await
                }
            };
            match written {
                Ok(Written::Written(_)) => {}
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
struct DownloadSpec {
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
