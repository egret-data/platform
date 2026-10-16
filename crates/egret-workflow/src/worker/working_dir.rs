//! The `WorkingDirectory` block's own work, and its children's snapshots
//! (spec §4.3, §5.1–§5.4).
//!
//! A block has two claims that no handler runs: **prepare** (`PREPARING`,
//! before the first child) and **finish** (`FINISHING`, after the last one,
//! whatever a child did). The worker completes them in
//! `claim::run_block_claim`, through the same lease, fence and settle a leaf
//! uses, and the functions here are the work inside that lease.
//!
//! Every failure here is the block's: [`run_claim`] returns
//! [`BlockDone::Failed`] rather than an `Err`, so a block never sits in
//! `PREPARING` for want of a completed claim (V3 and the no-silent-stall rule).

use std::collections::BTreeMap;

use serde_json::Value;
use sqlx::PgPool;

use super::claim::Claimed;
use super::commands::{
    output_problem, relative_problem, CollectTo, Naming, StoredInput, WORKING_DIR,
};
use super::handlers::script::{STORAGE_SCHEME, WORKING_DIR_VAR};
use super::WorkingDirCarry;
use crate::models::{Flow, Task, TaskKind, WorkingDirectory};
use crate::runners::files::is_namespace_file_uri;
use crate::runners::{render_str, RenderContext};
use crate::storage::collect::collect_one;
use crate::storage::nsfiles::glob::wildcard_match;
use crate::storage::nsfiles::stage::{
    check_task_bytes, merge, resolve_ns_file, stage, NamespaceFilesSpec,
};
use crate::storage::transfer::{write_tar, EntryKind, TarReader, TarSource};
use crate::storage::{FileStore, StoredFile};

/// The object a block's prepare claim writes: the directory the first child
/// starts from. Its name is under the block's prefix, so the finish claim's
/// discard removes it with the children's snapshots.
const PREPARE_OBJECT: &str = "prepare.tar";

/// What a block's own claim produced, for `claim::run_block_claim` to commit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BlockDone {
    /// The prepare claim is done; `snapshot` is the pointer the first child
    /// starts from.
    Prepared { snapshot: String },
    /// The finish claim is done: the block's `outputs` (the `outputFiles` map,
    /// when any is declared) and the files it stored, to index under the block.
    Finished {
        outputs: Option<Value>,
        files: Vec<StoredFile>,
    },
    /// The claim could not be completed; the block fails with this reason.
    Failed(String),
}

/// The carry of a child of a `WorkingDirectory` block, or `Err` with the reason
/// it cannot start: a block that has run no prepare claim has no snapshot, and
/// a child on an empty directory would be a silent wrong answer (spec §5.2).
pub(crate) fn carry_of(
    c: &Claimed,
    flow: &Flow,
    render: &RenderContext,
) -> Result<Option<WorkingDirCarry>, String> {
    let Some(block) = c.block_id else {
        return Ok(None);
    };
    let Some(start_from) = c.block_snapshot.clone() else {
        return Err(format!(
            "corrupt: the WorkingDirectory block of {} has no snapshot to start it from",
            c.task_id
        ));
    };
    let Some(store) = crate::storage::current() else {
        return Err(format!(
            "{} is a child of a WorkingDirectory, which needs internal storage for its \
             snapshots, and none is installed",
            c.task_id
        ));
    };
    // The block's declared outputs, rendered as the block does: a child checks
    // its own copy-out against them (spec §5.3, `check_child_outputs`).
    let Some(parent) = parent_block_of(flow, &c.task_id) else {
        return Err(format!(
            "corrupt: {} is a child of a WorkingDirectory that its flow does not declare",
            c.task_id
        ));
    };
    Ok(Some(WorkingDirCarry {
        start_from,
        carry_to: store.working_dir_uri(
            c.workspace_id,
            c.execution_id,
            block,
            &format!("{}.tar", c.id),
        ),
        cap: store.caps.max_task_bytes,
        outputs: rendered_outputs(render, parent)?,
    }))
}

/// Do the block's own claim named by `c.block_phase`. Never `Err`: whatever
/// goes wrong, the block is failed, and the failure is committed like any
/// outcome. `render` is the block run's own render context, the one a leaf of
/// the same flow renders with (spec §5.1).
pub(crate) async fn run_claim(pool: &PgPool, c: &Claimed, render: &RenderContext) -> BlockDone {
    let Some(store) = crate::storage::current() else {
        return BlockDone::Failed(format!(
            "the WorkingDirectory {} needs internal storage for its snapshots, and none is installed",
            c.task_id
        ));
    };
    match c.block_phase.as_deref() {
        Some("PREPARING") => match prepare(pool, &store, c, render).await {
            Ok(snapshot) => BlockDone::Prepared { snapshot },
            Err(reason) => BlockDone::Failed(reason),
        },
        Some("FINISHING") => match finish(pool, &store, c, render).await {
            Ok((outputs, files)) => BlockDone::Finished { outputs, files },
            Err(reason) => BlockDone::Failed(reason),
        },
        other => BlockDone::Failed(format!(
            "the WorkingDirectory {} was claimed in {other:?}, which is not a phase it runs",
            c.task_id
        )),
    }
}

/// The block's definition, found by its task id in the frozen flow source.
/// `None` when the flow has no such block, which is corruption.
fn block_of<'a>(flow: &'a Flow, id: &str) -> Option<&'a WorkingDirectory> {
    fn walk<'a>(task: &'a Task, id: &str) -> Option<&'a WorkingDirectory> {
        if task.id == id {
            return match &task.kind {
                TaskKind::WorkingDirectory(w) => Some(w),
                _ => None,
            };
        }
        task.children()
            .into_iter()
            .chain(task.error_children())
            .find_map(|c| walk(c, id))
    }
    flow.tasks.iter().find_map(|t| walk(t, id))
}

/// Renders one string of the block with the block's render context, as a
/// script task's property renders: the same `render_str`, with `{{ workingDir }}`
/// bound to the directory a child sees.
fn render_with(cx: &RenderContext, what: &str, s: &str) -> Result<String, String> {
    let mut cx = cx.clone();
    cx.insert(WORKING_DIR_VAR, serde_json::json!(WORKING_DIR));
    render_str(s, &cx).map_err(|e| format!("{what} did not render: {e}"))
}

/// The prepare claim's work (spec §5.1, §5.2): the directory the first child
/// starts from, written as the block's first snapshot. Returns the object's
/// URI, which the block's pointer names. `Err` is the block's failure.
///
/// The order is Kestra's (`preExecuteTasks`): `namespaceFiles` first, then
/// the `inputFiles` that overwrite them (spec §2.2). `namespaceFiles` is
/// staged by the same [`stage`] a script task uses, with the flow's own
/// namespace as the default.
async fn prepare(
    pool: &PgPool,
    store: &FileStore,
    c: &Claimed,
    render: &RenderContext,
) -> Result<String, String> {
    let flow = crate::spec::parse(&c.source).map_err(|e| {
        format!(
            "the WorkingDirectory {} cannot be prepared: its flow does not parse: {}",
            c.task_id, e.message
        )
    })?;
    let Some(block) = block_of(&flow, &c.task_id) else {
        return Err(format!(
            "the WorkingDirectory {} is not a block of its flow",
            c.task_id
        ));
    };
    // The flow's own namespace: the default of `namespaceFiles`, and the one an
    // `inputFiles` namespace-file URI is read as (W9b spec §7).
    let own = render.files().and_then(|f| f.0.flow_namespace());
    let mut files: BTreeMap<String, StoredInput> = BTreeMap::new();
    if let Some(v) = &block.namespace_files {
        let spec = NamespaceFilesSpec::parse(v).map_err(|(path, message)| {
            if path.is_empty() {
                format!("namespaceFiles {message}")
            } else {
                format!("namespaceFiles.{path} {message}")
            }
        })?;
        let rendered = spec.render(|what, s| render_with(render, what, s))?;
        if rendered.enabled {
            let own = own.ok_or_else(|| {
                format!(
                    "namespace files cannot be staged for {}: this run is not a task run of \
                     a flow's execution, whose namespace they are read as",
                    c.task_id
                )
            })?;
            files = match stage(pool, store, c.workspace_id, own, &rendered).await {
                Ok(Ok(staged)) => {
                    for line in &staged.warnings {
                        tracing::warn!(task_id = %c.task_id, "{line}");
                    }
                    staged.files
                }
                Ok(Err(why)) => return Err(why),
                Err(e) => {
                    return Err(format!(
                        "the namespace files of {} could not be staged: {e}",
                        c.task_id
                    ))
                }
            };
        }
    }
    // `inputFiles` load after `namespaceFiles` and overwrite a namespace file
    // of their name, whatever `ifExists` says (spec §2.2, the W9b ruling).
    let (contents, input_stored) =
        input_files(pool, store, c, render, own, block.input_files.as_ref()).await?;
    let merged = merge(files, &contents, input_stored);
    check_task_bytes(&merged, store.caps.max_task_bytes)?;
    let uri = store.working_dir_uri(c.workspace_id, c.execution_id, c.id, PREPARE_OBJECT);
    write_archive(store, &uri, &contents, &merged)
        .await
        .map_err(|e| {
            format!(
                "the WorkingDirectory {} could not write its snapshot: {e}",
                c.task_id
            )
        })?;
    Ok(uri)
}

/// The block's `inputFiles`, rendered and split as a script task's are (spec
/// §2.2, W9b Task 5): inline contents; a `kestra://` value, a file already in
/// internal storage, looked up through the run's render scope; and a
/// namespace-file URI, resolved by [`resolve_ns_file`]. A name with a problem,
/// or a value that cannot be read, is the block's failure.
async fn input_files(
    pool: &PgPool,
    store: &FileStore,
    c: &Claimed,
    render: &RenderContext,
    own: Option<&str>,
    declared: Option<&Value>,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, StoredInput>), String> {
    let mut contents = BTreeMap::new();
    let mut stored = BTreeMap::new();
    let Some(declared) = declared else {
        return Ok((contents, stored));
    };
    let Value::Object(map) = declared else {
        return Err(format!(
            "the WorkingDirectory {} declares inputFiles, which is not a map of name to value",
            c.task_id
        ));
    };
    for (name, value) in map {
        if let Some(problem) = relative_problem(name) {
            return Err(format!("inputFiles: {problem}"));
        }
        let Value::String(raw) = value else {
            return Err(format!(
                "inputFiles {name:?}: its value is not a string, which is what it must be"
            ));
        };
        let value = render_with(render, &format!("inputFiles {name:?}"), raw)?;
        if is_namespace_file_uri(&value) {
            let own = own.ok_or_else(|| {
                format!(
                    "inputFiles {name:?}: {value} is read as a namespace file, and this run is \
                     not a task run of a flow's execution"
                )
            })?;
            let mut conn = pool.acquire().await.map_err(|e| {
                format!(
                    "the namespace files of {} could not be read: {e}",
                    c.task_id
                )
            })?;
            match resolve_ns_file(&mut conn, store, c.workspace_id, own, &value).await {
                Ok(Ok(found)) => {
                    stored.insert(name.clone(), found.input);
                }
                Ok(Err(why)) => return Err(format!("inputFiles {name:?}: {why}")),
                Err(e) => {
                    return Err(format!(
                        "the namespace files of {} could not be read: {e}",
                        c.task_id
                    ))
                }
            }
        } else if value.starts_with(STORAGE_SCHEME) {
            let fail = |why: String| format!("inputFiles {name:?}: {why}");
            let scope = render.files().ok_or_else(|| {
                fail(format!(
                    "{value} cannot be read: this render has no file scope"
                ))
            })?;
            let meta = scope
                .0
                .lookup(&value)
                .map_err(|u| fail(u.message().to_string()))?;
            let object_uri = store
                .object_uri(c.workspace_id, &value)
                .map_err(|e| fail(e.to_string()))?;
            stored.insert(
                name.clone(),
                StoredInput {
                    uri: value.clone(),
                    object_uri,
                    size: meta.size,
                },
            );
        } else {
            contents.insert(name.clone(), value);
        }
    }
    Ok((contents, stored))
}

/// Write one ustar archive at `uri`, streamed: [`write_tar`] writes into a
/// pipe that the store reads, so no archive is held whole (spec §4.4). The
/// entries are `contents` (inline) and `stored` (files in storage). The two
/// never share a name: [`merge`] removed the namespace file a name of
/// `contents` would shadow. A failed write removes the object, so a half
/// archive is never a pointer.
async fn write_archive(
    store: &FileStore,
    uri: &str,
    contents: &BTreeMap<String, String>,
    stored: &BTreeMap<String, StoredInput>,
) -> Result<(), String> {
    let mut sources: BTreeMap<String, TarSource<'_>> = contents
        .iter()
        .map(|(name, text)| (name.clone(), TarSource::Contents(text)))
        .collect();
    for (name, input) in stored {
        sources.insert(name.clone(), TarSource::Stored(input));
    }
    let (mut writer, mut reader) = tokio::io::duplex(256 * 1024);
    let write = async {
        let res = write_tar(&mut writer, &sources, Some(store)).await;
        // Closing the write half is what ends the upload.
        drop(writer);
        res
    };
    // `reader` moves into the upload, so a failed upload closes the pipe and
    // the writer stops, rather than blocking on a full buffer.
    let upload = async move { store.store.put_stream(uri, &mut reader, None).await };
    let (written, stored) = tokio::join!(write, upload);
    match (written, stored) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(e), _) => {
            let _ = store.store.delete(uri).await;
            Err(e.to_string())
        }
        (Ok(()), Err(e)) => {
            let _ = store.store.delete(uri).await;
            Err(e.to_string())
        }
    }
}

/// The `WorkingDirectory` whose `tasks` hold the child `child`, found in the
/// frozen flow. `None` when no block of the flow holds it.
fn parent_block_of<'a>(flow: &'a Flow, child: &str) -> Option<&'a WorkingDirectory> {
    fn walk<'a>(task: &'a Task, child: &str) -> Option<&'a WorkingDirectory> {
        if let TaskKind::WorkingDirectory(w) = &task.kind {
            if w.tasks.iter().any(|t| t.id == child) {
                return Some(w);
            }
        }
        task.children()
            .into_iter()
            .chain(task.error_children())
            .find_map(|c| walk(c, child))
    }
    flow.tasks.iter().find_map(|t| walk(t, child))
}

/// The block's `outputFiles`, rendered and checked (spec §2.2): a list of names
/// whose wildcards are the ones [`output_problem`] accepts. Empty when the
/// block declares none.
fn rendered_outputs(
    render: &RenderContext,
    block: &WorkingDirectory,
) -> Result<Vec<String>, String> {
    let Some(declared) = &block.output_files else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = declared else {
        return Err("outputFiles is not a list of names".to_string());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Value::String(raw) = item else {
            return Err("outputFiles: a name is not a string".to_string());
        };
        let name = render_with(render, "outputFiles", raw)?;
        if let Some(problem) = output_problem(&name) {
            return Err(format!("outputFiles: {problem}"));
        }
        out.push(name);
    }
    Ok(out)
}

/// How a declared name meets an archive entry's name: `Some(true)` when a
/// literal name is that entry, `Some(false)` when only a `*`/`?` pattern
/// matches it, `None` when neither does. A pattern matches within the one
/// directory the declared name gives (spec §2.2, as a script task's does).
fn declared_hit(globs: &[String], name: &str) -> Option<bool> {
    if globs
        .iter()
        .any(|g| !crate::worker::commands::is_pattern(g) && g == name)
    {
        return Some(true);
    }
    let matches_pattern = globs.iter().any(|g| {
        if !crate::worker::commands::is_pattern(g) {
            return false;
        }
        let (gdir, gfile) = g.rsplit_once('/').unwrap_or(("", g));
        let (ndir, nfile) = name.rsplit_once('/').unwrap_or(("", name));
        gdir == ndir && wildcard_match(gfile, nfile)
    });
    matches_pattern.then_some(false)
}

/// The refusal for a declared name that names a directory or a non-file,
/// which cannot be collected. A pattern never gets one: it skips such entries.
fn not_collectable(name: &str, kind: &EntryKind) -> String {
    match kind {
        EntryKind::Directory => format!(
            "`outputFiles` declares {name:?}, which is a DIRECTORY; name the files inside it \
             instead, or write them to the output directory (`outputDirectory: true`)"
        ),
        _ => format!("`outputFiles` declares {name:?}, which is not a regular file"),
    }
}

/// A child's copy-out checked against the block's declared outputs, before the
/// child's outcome is recorded (spec §5.3): a declared name that is a directory
/// or a non-file, or a file over a cap, fails the child by name. A copy-out
/// that left no object is not checked here; the child already failed for it.
pub(crate) async fn check_child_outputs(wd: &WorkingDirCarry) -> Result<(), String> {
    let Some(store) = crate::storage::current() else {
        return Ok(());
    };
    if !matches!(store.store.head(&wd.carry_to).await, Ok(Some(_))) {
        return Ok(());
    }
    let mut src = store
        .store
        .get_stream(&wd.carry_to)
        .await
        .map_err(|e| format!("the directory left by the child could not be read: {e}"))?;
    let mut reader = TarReader::new(&mut src);
    let mut total: u64 = 0;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| format!("the directory left by the child could not be read: {e}"))?
    {
        // A directory's entry is named with its trailing `/`, which a declared
        // name never has.
        let bare = entry.name.trim_end_matches('/');
        let Some(literal) = declared_hit(&wd.outputs, bare) else {
            continue;
        };
        match entry.kind {
            EntryKind::File => {
                if entry.size > store.caps.max_file_bytes {
                    return Err(format!(
                        "the output file {:?} is {} bytes, over the {} one file may be \
                         (workflow.storage.max_file_bytes)",
                        entry.name, entry.size, store.caps.max_file_bytes
                    ));
                }
                total = total.saturating_add(entry.size);
                if total > store.caps.max_task_bytes {
                    return Err(format!(
                        "the output files of this WorkingDirectory come to {total} bytes with \
                         {:?}, over the {} one task run may store (workflow.storage.max_task_bytes)",
                        entry.name, store.caps.max_task_bytes
                    ));
                }
            }
            EntryKind::Directory if !literal => {}
            kind => {
                if literal {
                    return Err(not_collectable(bare, &kind));
                }
            }
        }
    }
    Ok(())
}

/// The finish claim's `outputFiles` (spec §5.5): every file of the last
/// snapshot that a declared name matches is collected into internal storage
/// under the block run's own task prefix, and the block's outputs map each
/// name to its `kestra://` URI. `Ok((None, []))` when no output is declared.
async fn finish(
    pool: &PgPool,
    store: &FileStore,
    c: &Claimed,
    render: &RenderContext,
) -> Result<(Option<Value>, Vec<StoredFile>), String> {
    let flow = crate::spec::parse(&c.source).map_err(|e| {
        format!(
            "the WorkingDirectory {} cannot be finished: its flow does not parse: {}",
            c.task_id, e.message
        )
    })?;
    let Some(block) = block_of(&flow, &c.task_id) else {
        return Err(format!(
            "the WorkingDirectory {} is not a block of its flow",
            c.task_id
        ));
    };
    let globs = rendered_outputs(render, block)?;
    if globs.is_empty() {
        return Ok((None, Vec::new()));
    }
    let pointer: Option<String> =
        sqlx::query_scalar("SELECT working_dir_snapshot FROM workflow.task_runs WHERE id = $1")
            .bind(c.id)
            .fetch_one(pool)
            .await
            .map_err(|e| {
                format!(
                    "the WorkingDirectory {} could not read its snapshot: {e}",
                    c.task_id
                )
            })?;
    let Some(pointer) = pointer else {
        return Err(format!(
            "corrupt: the WorkingDirectory {} has no snapshot to collect its outputs from",
            c.task_id
        ));
    };
    let task_prefix = crate::storage::uri::task_prefix(
        &flow.namespace,
        &flow.id,
        c.execution_id,
        &c.task_id,
        c.id,
    )
    .map_err(|e| format!("the WorkingDirectory {}: {e}", c.task_id))?;
    let to = CollectTo {
        workspace: c.workspace_id,
        task_prefix,
        naming: Naming::KestraOutputName,
    };
    let collected = collect_outputs(store, &pointer, &to, &globs).await?;
    let map: serde_json::Map<String, Value> = collected
        .iter()
        .map(|(name, file)| (name.clone(), Value::String(file.uri.clone())))
        .collect();
    let outputs = serde_json::json!({ "outputFiles": map });
    Ok((Some(outputs), collected.into_values().collect()))
}

/// Collect the files of the archive at `uri` that `globs` name (see
/// [`declared_hit`]), in archive order, into internal storage under `to`. A
/// directory or non-file named literally fails the block, naming it.
async fn collect_outputs(
    store: &FileStore,
    uri: &str,
    to: &CollectTo,
    globs: &[String],
) -> Result<BTreeMap<String, StoredFile>, String> {
    let unread = |e: &dyn std::fmt::Display| format!("the snapshot {uri} could not be read: {e}");
    let mut src = store.store.get_stream(uri).await.map_err(|e| unread(&e))?;
    let mut reader = TarReader::new(&mut src);
    let mut out = BTreeMap::new();
    let mut total: u64 = 0;
    while let Some(entry) = reader.next_entry().await.map_err(|e| unread(&e))? {
        let bare = entry.name.trim_end_matches('/');
        let Some(literal) = declared_hit(globs, bare) else {
            continue;
        };
        match entry.kind {
            EntryKind::File => {
                let file = collect_one(
                    store,
                    to,
                    &entry.name,
                    &mut reader.body(),
                    entry.size,
                    &mut total,
                )
                .await
                .map_err(|e| e.to_string())?;
                out.insert(entry.name.clone(), file);
            }
            EntryKind::Directory if !literal => {}
            kind => {
                if literal {
                    return Err(not_collectable(bare, &kind));
                }
            }
        }
    }
    Ok(out)
}

/// Remove every snapshot of block run `c.id`, after its finish claim has
/// committed. Best effort: an object left behind is the pruner's to sweep
/// (spec §7.2), and the block is already finished.
pub(crate) async fn discard(c: &Claimed) {
    let Some(store) = crate::storage::current() else {
        return;
    };
    let prefix = store.working_dir_prefix(c.workspace_id, c.execution_id, c.id);
    if let Err(e) = store.store.delete_prefix(&prefix).await {
        tracing::warn!(
            execution = %c.execution_id,
            run = %c.id,
            "the snapshots of a finished WorkingDirectory could not be removed (the pruner will \
             sweep them): {e}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::nsfiles::stage::testing::{self as ns};
    use crate::storage::transfer::TarReader;
    use crate::worker::{
        start_execution, tick_with, Handlers, Outcome, Registry, TaskInput, WorkerTiming,
    };
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    /// The real handlers, and a record of the snapshot each child starts from,
    /// read while the child runs. The finish claim discards every snapshot once
    /// the block ends, so a test cannot read the directory afterwards.
    struct CaptureStart {
        inner: Handlers,
    }

    /// `(execution, archive)` of each child that started on a snapshot, for
    /// every registry of this binary. A tick runs other tests' children with its
    /// own registry, so the record cannot live in one registry.
    static CHILD_STARTS: Mutex<Vec<(uuid::Uuid, Vec<u8>)>> = Mutex::new(Vec::new());

    #[async_trait::async_trait]
    impl Registry for CaptureStart {
        async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
            if let (Some(wd), Some(store)) = (&task.working_dir, crate::storage::current()) {
                if let Ok(bytes) = store.store.get(&wd.start_from).await {
                    CHILD_STARTS
                        .lock()
                        .expect("the lock")
                        .push((task.execution_id, bytes));
                }
            }
            self.inner.run(task).await
        }
        async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
            self.inner.cancel(task).await
        }
        async fn release(&self, task: &TaskInput) -> egret_core::Result<()> {
            self.inner.release(task).await
        }
        fn log_sink(
            &self,
        ) -> (
            std::sync::Arc<dyn crate::worker::handlers::LogSink>,
            crate::logs::LogLimits,
        ) {
            self.inner.log_sink()
        }
        fn orphan_sweep(&self) -> Option<std::sync::Arc<dyn crate::worker::commands::OrphanSweep>> {
            self.inner.orphan_sweep()
        }
    }

    impl CaptureStart {
        fn new(fs: std::sync::Arc<crate::storage::FileStore>) -> Self {
            Self {
                inner: Handlers::with_core_tasks(Some(fs)),
            }
        }

        /// The archive the first child of execution `ex` started from.
        fn first_start(&self, ex: uuid::Uuid) -> Vec<u8> {
            CHILD_STARTS
                .lock()
                .expect("the lock")
                .iter()
                .find(|(e, _)| *e == ex)
                .map(|(_, bytes)| bytes.clone())
                .expect("a child of this execution started")
        }
    }

    /// The task types this test binary serves: the Docker set a deployment with
    /// containers serves (`tests/common` installs the same one).
    fn install_served() {
        let mut cfg = egret_config::Config::default().workflow;
        cfg.container_runner = egret_config::ContainerRunner::Docker;
        crate::runtime::install_served_for_tests(crate::runtime::served_for(&cfg));
    }

    /// One flow of two `Return` children, with `block_props` written under
    /// the block (indented to its `tasks:` level).
    fn block_flow(namespace: &str, block_props: &str) -> String {
        format!(
            "id: wd-props\nnamespace: {namespace}\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n{block_props}    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n"
        )
    }

    /// Every regular file of the archive `bytes`, as `name -> contents`.
    async fn files_in(bytes: &[u8]) -> BTreeMap<String, String> {
        let mut src: &[u8] = bytes;
        let mut reader = TarReader::new(&mut src);
        let mut out = BTreeMap::new();
        while let Some(entry) = reader.next_entry().await.expect("a header") {
            if entry.kind == crate::storage::transfer::EntryKind::File {
                let mut body = Vec::new();
                reader
                    .body()
                    .read_to_end(&mut body)
                    .await
                    .expect("the body");
                out.insert(entry.name, String::from_utf8(body).expect("utf-8"));
            }
        }
        out
    }

    /// Tick until the block is terminal, or fail after `rounds` ticks.
    async fn settle(pool: &PgPool, reg: &dyn Registry, ex: uuid::Uuid) {
        for _ in 0..60 {
            tick_with(pool, 8, reg, WorkerTiming::default())
                .await
                .expect("a tick");
            let state: String = sqlx::query_scalar(
                "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
            )
            .bind(ex)
            .fetch_one(pool)
            .await
            .expect("the block");
            if matches!(state.as_str(), "SUCCESS" | "FAILED" | "WARNING") {
                return;
            }
        }
        panic!("the block did not settle");
    }

    /// `(state, message)` of the block.
    async fn block_state(pool: &PgPool, ex: uuid::Uuid) -> (String, Option<String>) {
        sqlx::query_as(
            "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
        )
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("the block")
    }

    /// Spec §2.2 (`namespaceFiles` loads before the first child): the file
    /// the flow's own namespace holds is in the directory the first child
    /// starts from, and publish no longer refuses the property.
    #[tokio::test]
    async fn namespace_files_are_in_the_directory_the_first_child_starts_from() {
        install_served();
        let pool = ns::pool().await.expect("the test database");
        let fs = crate::storage::install_mem_for_tests();
        let ws = ns::workspace(&pool).await;
        ns::write(&pool, &fs, ws, "w11.nsf", "/data/ref.txt", "from-namespace").await;
        ns::write(&pool, &fs, ws, "w11.nsf", "/other.txt", "not-included").await;
        let src = block_flow(
            "w11.nsf",
            "    namespaceFiles:\n      include: [\"data/**\"]\n",
        );

        let flow = crate::spec::parse(&src).expect("the flow parses");
        let refused: Vec<_> = crate::spec::validate(&flow)
            .errors
            .into_iter()
            .filter(|f| f.code == "working_directory_property_not_yet_run")
            .collect();
        assert!(
            refused.is_empty(),
            "namespaceFiles publishes once it runs: {refused:?}"
        );

        let reg = CaptureStart::new(fs.clone());
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        settle(&pool, &reg, ex).await;

        let files = files_in(&reg.first_start(ex)).await;
        assert_eq!(
            files.get("data/ref.txt").map(String::as_str),
            Some("from-namespace"),
            "the included namespace file is staged into the directory: {files:?}"
        );
        assert!(
            !files.contains_key("other.txt"),
            "the exclusion is the block's include: {files:?}"
        );
    }

    /// Spec §2.2 (the W9b ruling, which holds here): `inputFiles` load after
    /// `namespaceFiles`, so an input overwrites a namespace file of its name.
    /// The contents are rendered with the flow's context, as a script's are.
    #[tokio::test]
    async fn input_files_are_in_the_directory_and_overwrite_a_namespace_file_of_the_same_name() {
        install_served();
        let pool = ns::pool().await.expect("the test database");
        let fs = crate::storage::install_mem_for_tests();
        let ws = ns::workspace(&pool).await;
        ns::write(&pool, &fs, ws, "w11.inp", "/shared.txt", "from-namespace").await;
        ns::write(&pool, &fs, ws, "w11.inp", "/data/ref.txt", "kept").await;
        let src = block_flow(
            "w11.inp",
            "    namespaceFiles:\n      enabled: true\n    inputFiles:\n      shared.txt: from-input\n      in/rendered.txt: \"from-{{ flow.id }}\"\n",
        );

        let flow = crate::spec::parse(&src).expect("the flow parses");
        let refused: Vec<_> = crate::spec::validate(&flow)
            .errors
            .into_iter()
            .filter(|f| f.code == "working_directory_property_not_yet_run")
            .collect();
        assert!(
            refused.is_empty(),
            "inputFiles publishes once it runs: {refused:?}"
        );

        let reg = CaptureStart::new(fs.clone());
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        settle(&pool, &reg, ex).await;

        let files = files_in(&reg.first_start(ex)).await;
        assert_eq!(
            files.get("shared.txt").map(String::as_str),
            Some("from-input"),
            "an input file overwrites the namespace file of its name: {files:?}"
        );
        assert_eq!(
            files.get("data/ref.txt").map(String::as_str),
            Some("kept"),
            "the other namespace files stay: {files:?}"
        );
        assert_eq!(
            files.get("in/rendered.txt").map(String::as_str),
            Some("from-wd-props"),
            "the contents render with the flow's context: {files:?}"
        );
    }

    /// The block's `outputs` column, as the finish claim wrote it.
    async fn outputs_of(pool: &PgPool, ex: uuid::Uuid) -> Option<Value> {
        sqlx::query_scalar(
            "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
        )
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("the block's run")
    }

    /// Spec §5.5, plan Task 7 Step 1: `outputFiles` is collected from the
    /// directory after the last child, and the block's outputs carry each
    /// matched file under its name, as a `kestra://` URI whose bytes are the
    /// file's. A literal name and a `*` pattern both match; a file that
    /// matches neither is not collected.
    #[tokio::test]
    async fn output_files_are_collected_from_the_directory_when_the_block_finishes() {
        install_served();
        let pool = ns::pool().await.expect("the test database");
        let fs = crate::storage::install_mem_for_tests();
        let ws = ns::workspace(&pool).await;
        let src = block_flow(
            "w11.out",
            "    inputFiles:\n      out.txt: hi\n      a.csv: \"1\"\n      b.csv: \"2\"\n      other.txt: x\n    outputFiles:\n      - out.txt\n      - \"*.csv\"\n",
        );
        let reg = CaptureStart::new(fs.clone());
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        settle(&pool, &reg, ex).await;

        let (state, _) = block_state(&pool, ex).await;
        assert_eq!(state, "SUCCESS");
        let outputs = outputs_of(&pool, ex).await.expect("the block's outputs");
        let collected = outputs["outputFiles"]
            .as_object()
            .expect("a map of outputFiles");
        let names: Vec<&str> = collected.keys().map(String::as_str).collect();
        assert_eq!(names, ["a.csv", "b.csv", "out.txt"], "{outputs}");
        for (name, want) in [("out.txt", "hi"), ("a.csv", "1"), ("b.csv", "2")] {
            let uri = collected[name].as_str().expect("a URI");
            assert!(uri.starts_with("kestra:///"), "{uri}");
            let object = fs.object_uri(ws, uri).expect("the object of the URI");
            let bytes = fs.store.get(&object).await.expect("the collected bytes");
            assert_eq!(bytes, want.as_bytes(), "{name}");
        }
    }

    /// Spec §5.3 and §5.5: a declared output that is a directory cannot be
    /// collected, so the finish claim fails the block, naming it. The block
    /// is `FAILED` with the reason, not `SUCCESS` with the directory missing.
    #[tokio::test]
    async fn a_declared_output_that_is_a_directory_fails_the_block_at_finish() {
        install_served();
        let pool = ns::pool().await.expect("the test database");
        let fs = crate::storage::install_mem_for_tests();
        let ws = ns::workspace(&pool).await;
        let src = block_flow(
            "w11.outdir",
            "    inputFiles:\n      sub/x.txt: x\n    outputFiles:\n      - sub\n",
        );
        let reg = CaptureStart::new(fs.clone());
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        settle(&pool, &reg, ex).await;

        let (state, message) = block_state(&pool, ex).await;
        assert_eq!(state, "FAILED");
        let message = message.expect("the reason");
        assert!(
            message.contains("sub") && message.contains("DIRECTORY"),
            "{message}"
        );
    }
}
