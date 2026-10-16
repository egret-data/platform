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
use super::commands::{relative_problem, StoredInput, WORKING_DIR};
use super::handlers::script::{STORAGE_SCHEME, WORKING_DIR_VAR};
use super::WorkingDirCarry;
use crate::models::{Flow, Task, TaskKind, WorkingDirectory};
use crate::runners::files::is_namespace_file_uri;
use crate::runners::{render_str, RenderContext};
use crate::storage::nsfiles::stage::{
    check_task_bytes, merge, resolve_ns_file, stage, NamespaceFilesSpec,
};
use crate::storage::transfer::{write_tar, TarSource};
use crate::storage::FileStore;

/// The object a block's prepare claim writes: the directory the first child
/// starts from. Its name is under the block's prefix, so the finish claim's
/// discard removes it with the children's snapshots.
const PREPARE_OBJECT: &str = "prepare.tar";

/// What a block's own claim produced, for `claim::run_block_claim` to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockDone {
    /// The prepare claim is done; `snapshot` is the pointer the first child
    /// starts from.
    Prepared { snapshot: String },
    /// The finish claim is done. Its work (`outputFiles`, cache) is Task 7's.
    Finished,
    /// The claim could not be completed; the block fails with this reason.
    Failed(String),
}

/// The carry of a child of a `WorkingDirectory` block, or `Err` with the reason
/// it cannot start: a block that has run no prepare claim has no snapshot, and
/// a child on an empty directory would be a silent wrong answer (spec §5.2).
pub(crate) fn carry_of(c: &Claimed) -> Result<Option<WorkingDirCarry>, String> {
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
    Ok(Some(WorkingDirCarry {
        start_from,
        carry_to: store.working_dir_uri(
            c.workspace_id,
            c.execution_id,
            block,
            &format!("{}.tar", c.id),
        ),
        cap: store.caps.max_task_bytes,
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
        Some("FINISHING") => BlockDone::Finished,
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
    use crate::worker::{start_execution, tick_with, Handlers, WorkerTiming};
    use serde_json::json;
    use tokio::io::AsyncReadExt;

    /// One flow of two `Return` children, with `block_props` written under
    /// the block (indented to its `tasks:` level).
    fn block_flow(namespace: &str, block_props: &str) -> String {
        format!(
            "id: wd-props\nnamespace: {namespace}\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n{block_props}    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n"
        )
    }

    /// The task types this test binary serves: the Docker set a deployment with
    /// containers serves (`tests/common` installs the same one).
    fn install_served() {
        let mut cfg = egret_config::Config::default().workflow;
        cfg.container_runner = egret_config::ContainerRunner::Docker;
        crate::runtime::install_served_for_tests(crate::runtime::served_for(&cfg));
    }

    /// The block's `working_dir_snapshot`, once it is set.
    async fn snapshot_of(pool: &PgPool, ex: uuid::Uuid) -> Option<String> {
        sqlx::query_scalar(
            "SELECT working_dir_snapshot FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'block'",
        )
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("the block's run")
    }

    /// Every regular file of the archive at `uri`, as `name -> contents`.
    async fn files_in(fs: &crate::storage::FileStore, uri: &str) -> BTreeMap<String, String> {
        let bytes = fs.store.get(uri).await.expect("the snapshot object");
        let mut src: &[u8] = &bytes;
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

    /// Tick until this execution's block is `PREPARED`. The lib tests share one
    /// database, so a tick may claim another test's run; the loop answers for
    /// this block alone.
    async fn tick_until_prepared(pool: &PgPool, reg: &Handlers, ex: uuid::Uuid) {
        for _ in 0..60 {
            tick_with(pool, 8, reg, WorkerTiming::default())
                .await
                .expect("a tick");
            let phase: Option<String> = sqlx::query_scalar(
                "SELECT working_dir FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
            )
            .bind(ex)
            .fetch_one(pool)
            .await
            .expect("the block");
            if phase.as_deref() == Some("PREPARED") {
                return;
            }
        }
        panic!("the block was not prepared in 60 ticks");
    }

    /// Tick until the block is terminal.
    async fn settle(pool: &PgPool, reg: &Handlers, ex: uuid::Uuid) {
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

    /// Spec §2.2 (`namespaceFiles` loads before the first child): the file
    /// the flow's own namespace holds is in the directory the first child
    /// starts from, and publish no longer refuses the property by name.
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

        let reg = Handlers::with_core_tasks(Some(fs.clone()));
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        // The prepare claim. The finish claim discards the snapshot after the
        // block ends, so the directory is read while it is PREPARED.
        tick_until_prepared(&pool, &reg, ex).await;
        let uri = snapshot_of(&pool, ex)
            .await
            .expect("the prepare claim's pointer");
        let files = files_in(&fs, &uri).await;
        assert_eq!(
            files.get("data/ref.txt").map(String::as_str),
            Some("from-namespace"),
            "the included namespace file is staged into the directory: {files:?}"
        );
        assert!(
            !files.contains_key("other.txt"),
            "the exclusion is the block's include: {files:?}"
        );
        settle(&pool, &reg, ex).await;
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

        let reg = Handlers::with_core_tasks(Some(fs.clone()));
        let ex = start_execution(&pool, ws, None, &src, json!({}))
            .await
            .expect("start");
        tick_until_prepared(&pool, &reg, ex).await;
        let uri = match snapshot_of(&pool, ex).await {
            Some(uri) => uri,
            None => {
                let row: (String, Option<String>) = sqlx::query_as(
                    "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
                )
                .bind(ex)
                .fetch_one(&pool)
                .await
                .expect("the block");
                panic!("the prepare claim wrote no pointer: block {row:?}");
            }
        };
        let files = files_in(&fs, &uri).await;
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
        settle(&pool, &reg, ex).await;
    }
}
