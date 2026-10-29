//! W9a Task 4: `read`, `fileSize`, `fileExists` and `isFileEmpty` through the
//! real worker -- the snapshot taken before every render (spec §7.2, the
//! user's ruling of 2026-10-01). Requires DATABASE_URL.
//!
//! # Why `multi_thread`, and why the tests take turns
//!
//! `read()` blocks the render thread on the runtime handle while the tokio
//! worker that spawned it waits in `join`; that needs the multi-thread runtime
//! (global constraints), which `#[sqlx::test]` does not give (sqlx-core's
//! `test_block_on` is `new_current_thread`). So these use `common::db()` --
//! one database for this test binary -- and, because a claim is global (a
//! worker serves every workspace), they take turns on [`SERIAL`] so one test
//! never runs another's handlers. The object store is
//! `storage::install_mem_for_tests`'s `MemStore`: nothing here touches RustFS.
//!
//! # How a file gets written
//!
//! [`finish_writing`] stands in for a handler that wrote a file: it claims
//! the run, puts the bytes, and hands them to `commit_outcome` as
//! `Outcome::files` -- which, since W9a Task 6, indexes them **in the
//! transaction it then settles in**, and commits once (W9c Task 9: that
//! transaction is `commit_outcome`'s own, so a read in `settle` is fetched
//! with it rolled back, and the outcome applied again). The script handler's
//! own path (a collected `outputFiles` URI staged by a later task) is
//! [`a_later_task_stages_an_earlier_tasks_output_uri_end_to_end`].

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use egret_storage::{ObjectStore, StorageError};

use egret_metastore::Db;
use egret_workflow::render_str;
use egret_workflow::storage::{self, uri, FileStore, StoredFile};
use egret_workflow::worker::{
    claim, commit_outcome, execution_render_context, reap, start_execution, start_execution_in,
    Handlers, NewExecution, Outcome,
};
use serde_json::json;
use uuid::Uuid;

/// Async, so a turn can be held across `.await` (a std guard cannot), and
/// runtime-agnostic, so each test's own runtime can wait on it.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NS: &str = "w9a.t4";

/// While set, every read from the store fails as an unreachable S3 does
/// (spec §11's outage). Tests take turns on [`SERIAL`], so one test's outage
/// is nobody else's.
static OUTAGE: AtomicBool = AtomicBool::new(false);

/// `MemStore`, with [`OUTAGE`] in front of `get_stream`.
struct Flaky(egret_storage::MemStore);

#[async_trait::async_trait]
impl ObjectStore for Flaky {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        if OUTAGE.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )));
        }
        self.0.get_stream(uri).await
    }
    // W9a Task 6: collected output files are written by stream and sized by
    // `head`; both go straight to the `MemStore`.
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        if OUTAGE.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )));
        }
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
}

/// The process-wide store, installed once: [`Flaky`] over a `MemStore`.
fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = storage::install(Arc::new(FileStore {
            store: Arc::new(Flaky(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps::default(),
            retention_days: 30,
        }));
    });
    storage::current().expect("installed above")
}

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    let (ws, _) = common::seed_workspace(&db).await;
    OUTAGE.store(false, Ordering::SeqCst);
    Some((db, ws, installed()))
}

/// Tick until nothing is claimable.
async fn drain(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(egret_workflow::storage::current());
    for _ in 0..64 {
        if egret_workflow::worker::tick(pool, 16, &reg)
            .await
            .expect("tick")
            == 0
        {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

async fn run_of(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> (Uuid, String) {
    sqlx::query_as(
        "SELECT id, state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("run {task} of {execution}: {e}"))
}

/// `(state, outputs, message)` of the one run of `task` in any execution of `ws`.
async fn result_of(
    pool: &sqlx::PgPool,
    ws: Uuid,
    task: &str,
) -> (String, Option<serde_json::Value>, Option<String>) {
    sqlx::query_as(
        "SELECT tr.state, tr.outputs, tr.message FROM workflow.task_runs tr \
           JOIN workflow.executions e ON e.id = tr.execution_id \
          WHERE e.workspace_id = $1 AND tr.task_id = $2",
    )
    .bind(ws)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("run {task}: {e}"))
}

async fn put(fs: &FileStore, ws: Uuid, uri: &str, bytes: &[u8]) {
    fs.store
        .put(&fs.object_uri(ws, uri).unwrap(), bytes.to_vec())
        .await
        .expect("put");
}

/// Claim the one claimable run (`task` of some execution), write `bytes` as
/// its file `name`, index it and apply a SUCCESS whose outputs are
/// `{"uri": …}` -- the index rows and the settle in ONE transaction, as
/// Task 6's `apply_outcome` will have them. Returns the URI.
async fn finish_writing(
    pool: &sqlx::PgPool,
    fs: &FileStore,
    ws: Uuid,
    flow_id: &str,
    task: &str,
    name: &str,
    bytes: &[u8],
) -> String {
    let (file, applied) = write_and_apply(
        pool,
        fs,
        ws,
        flow_id,
        task,
        name,
        Some(bytes),
        |u| json!({ "uri": u }),
    )
    .await;
    applied.expect("apply");
    file
}

/// [`finish_writing`]'s body: `bytes: None` indexes a file whose object was
/// never stored (a prune race, an orphaned row); `outputs` builds the
/// outputs from the URI. Commits only when `commit_outcome` answered `Ok`,
/// and returns that answer.
#[allow(clippy::too_many_arguments)]
async fn write_and_apply(
    pool: &sqlx::PgPool,
    fs: &FileStore,
    ws: Uuid,
    flow_id: &str,
    task: &str,
    name: &str,
    bytes: Option<&[u8]>,
    outputs: impl Fn(&str) -> serde_json::Value,
) -> (String, egret_core::Result<()>) {
    let claimed = claim(pool, 1).await.expect("claim");
    assert_eq!(claimed.len(), 1, "exactly the writing run is claimable");
    let c = &claimed[0];
    assert_eq!(c.task_id, task);
    let file = format!(
        "{}{name}",
        uri::task_prefix(NS, flow_id, c.execution_id, task, c.id).unwrap()
    );
    if let Some(bytes) = bytes {
        put(fs, ws, &file, bytes).await;
    }
    let outcome = Outcome {
        files: vec![StoredFile {
            uri: file.clone(),
            size: bytes.map_or(3, |b| b.len() as u64),
        }],
        ..Outcome::succeeded_with(outputs(&file))
    };
    // W9c Task 9: in a transaction `commit_outcome` owns, as the worker's
    // tick does, so a successor's `read()` in `settle` is fetched with the
    // transaction rolled back and the outcome applied again.
    let applied = commit_outcome(pool, c, &outcome).await;
    (file, applied)
}

async fn attempts(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("attempts")
}

/// **The case a pool lookup gets WRONG.** `a`'s file is indexed in the
/// transaction that finishes `a`, and `b`'s `runIf` renders inside that same
/// transaction's `settle`; a lookup on any other connection would not see the
/// row (Missing → `fileSize` fails → `b` FAILED).
#[tokio::test(flavor = "multi_thread")]
async fn a_run_if_reads_a_file_the_previous_task_committed_in_the_same_transaction() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    for (flow, bytes, want) in [
        ("same_tx", &b"hello"[..], "RUNNING"),
        ("same_tx_empty", &b""[..], "SKIPPED"),
    ] {
        let source = format!(
            "id: {flow}\nnamespace: {NS}\ntasks:\n\
             \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: write\n\
             \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
             runIf: \"{{{{ fileSize(outputs.a.uri) > 0 }}}}\"\n    message: ran\n"
        );
        let ex = start_execution(&db.pool, ws, None, &source, json!({}))
            .await
            .unwrap();
        finish_writing(&db.pool, &fs, ws, flow, "a", "out.txt", bytes).await;
        // Read BEFORE anything else ticks: the state the same-transaction
        // settle left. RUNNING = created and started (runIf true), waiting
        // for a worker; SKIPPED = runIf false. The empty file is the control
        // that the runIf was evaluated at all.
        assert_eq!(run_of(&db.pool, ex, "b").await.1, want, "{flow}");
        drain(&db.pool).await;
    }
}

/// `w9a-storage.json` `file_functions.trigger_flow_source`: a Log and a Return
/// read their trigger execution's file by a literal URI.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_task_reads_its_trigger_executions_file() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source_flow = format!(
        "id: src\nnamespace: {NS}\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n"
    );
    let s = start_execution(&db.pool, ws, None, &source_flow, json!({}))
        .await
        .unwrap();
    let file = finish_writing(
        &db.pool,
        &fs,
        ws,
        "src",
        "w",
        "src.txt",
        b"flow trigger source content",
    )
    .await;
    drain(&db.pool).await;

    let listener = format!(
        "id: listener\nnamespace: {NS}\ntasks:\n\
         \x20 - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{{{ read('{file}') }}}}\"\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read('{file}') }}}} {{{{ fileSize('{file}') }}}} {{{{ fileExists('{file}') }}}} {{{{ isFileEmpty('{file}') }}}}\"\n"
    );
    let mut conn = db.pool.acquire().await.unwrap();
    let t = start_execution_in(
        &mut conn,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &listener,
            inputs: json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: Some(s),
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
            started_by: None,
        },
    )
    .await
    .unwrap();
    drop(conn);
    drain(&db.pool).await;
    assert_eq!(run_of(&db.pool, t, "l").await.1, "SUCCESS");
    let (state, outputs, message) = result_of(&db.pool, ws, "r").await;
    assert_eq!(
        (state.as_str(), outputs),
        (
            "SUCCESS",
            Some(json!({"value": "flow trigger source content 27 true false"}))
        ),
        "{message:?}"
    );
}

fn loop_source(flow: &str, before: &str, body: &str, after: &str) -> String {
    format!(
        "id: {flow}\nnamespace: {NS}\ninputs:\n  - id: f\n    type: STRING\n    defaults: none\ntasks:\n{before}\
         \x20 - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"x\"]'\n    tasks:\n{body}{after}"
    )
}

/// An iteration reads the root's file through `outputs.w`, inherited from
/// the root (`Surface::inherited`); the root is in the iteration's loop family.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_iteration_reads_a_file_its_root_wrote() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = loop_source(
        "loop_reads_root",
        "  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n",
        "      - id: in_iteration\n        type: io.kestra.plugin.core.debug.Return\n        \
         format: \"{{ read(outputs.w.uri) }}\"\n",
        "",
    );
    start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    finish_writing(
        &db.pool,
        &fs,
        ws,
        "loop_reads_root",
        "w",
        "root.txt",
        b"own content",
    )
    .await;
    drain(&db.pool).await;
    let (state, outputs, message) = result_of(&db.pool, ws, "in_iteration").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"value": "own content"}))),
        "{message:?}"
    );
}

/// The iteration's file URI carries the ITERATION's id (spec §3, the user's
/// ruling); a task after the loop, in the root, still reads it -- the root's
/// loop family holds every iteration.
///
/// What this proves is the READABLE SET, not an authoring path: the root
/// learns the URI through its `inputs`, rewritten by SQL once the iteration
/// has written it, because egret does not surface an iteration's outputs to
/// the root. Task 11 (the `Loop` STORE path, `executions/{iteration}/{n}.ion`)
/// replaces the UPDATE with the real route a root reaches such a file by.
#[tokio::test(flavor = "multi_thread")]
async fn the_roots_readable_set_holds_a_file_an_iteration_wrote() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = loop_source(
        "root_reads_iteration",
        "",
        "      - id: iw\n        type: io.kestra.plugin.core.log.Log\n        message: w\n",
        "  - id: after\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ read(inputs.f) }}\"\n",
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    // The loop spawns its iteration on settles; tick until `iw` is claimable.
    let file = finish_writing(
        &db.pool,
        &fs,
        ws,
        "root_reads_iteration",
        "iw",
        "it.txt",
        b"written in iteration",
    )
    .await;
    let parsed = uri::parse(&file).unwrap();
    assert_ne!(
        parsed.execution, root,
        "the iteration writes under its own id"
    );
    sqlx::query("UPDATE workflow.executions SET inputs = $2 WHERE id = $1")
        .bind(root)
        .bind(json!({ "f": file }))
        .execute(&db.pool)
        .await
        .unwrap();
    drain(&db.pool).await;
    let (state, outputs, message) = result_of(&db.pool, ws, "after").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"value": "written in iteration"}))),
        "{message:?}"
    );
}

/// `file_functions.unrelated_same_namespace` (chosen: NotReadable). The file
/// exists, in the same workspace and namespace; the render fails naming it.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_of_an_unrelated_execution_fails_the_render_naming_the_uri() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let other = format!("id: other\nnamespace: {NS}\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n");
    start_execution(&db.pool, ws, None, &other, json!({}))
        .await
        .unwrap();
    let file = finish_writing(
        &db.pool,
        &fs,
        ws,
        "other",
        "w",
        "u.txt",
        b"unrelated content",
    )
    .await;
    drain(&db.pool).await;

    for (task, call) in [
        ("r1", "read"),
        ("r2", "fileSize"),
        ("r3", "fileExists"),
        ("r4", "isFileEmpty"),
    ] {
        let reader = format!(
            "id: reader_{task}\nnamespace: {NS}\ntasks:\n  - id: {task}\n    type: io.kestra.plugin.core.debug.Return\n    \
             format: \"{{{{ {call}('{file}') }}}}\"\n"
        );
        start_execution(&db.pool, ws, None, &reader, json!({}))
            .await
            .unwrap();
        drain(&db.pool).await;
        let (state, outputs, message) = result_of(&db.pool, ws, task).await;
        let message = message.unwrap_or_default();
        assert_eq!(
            (state.as_str(), outputs),
            ("FAILED", None),
            "{call}: {message}"
        );
        assert!(
            message.contains(&file) && message.contains("may not read"),
            "{call}: {message}"
        );
    }
}

/// A Flow trigger's `conditions`/`inputs` render against
/// `execution_render_context`, which carries no file scope (§7.2).
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_trigger_condition_calling_read_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let src = format!("id: fired\nnamespace: {NS}\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n");
    let ex = start_execution(&db.pool, ws, None, &src, json!({}))
        .await
        .unwrap();
    let file = finish_writing(&db.pool, &fs, ws, "fired", "w", "f.txt", b"x").await;
    drain(&db.pool).await;

    let mut conn = db.pool.acquire().await.unwrap();
    let cx = execution_render_context(&mut conn, ex).await.unwrap();
    // The control: the same context renders an ordinary expression.
    assert_eq!(
        render_str("{{ execution.id }}", &cx).unwrap(),
        ex.to_string()
    );
    for call in ["read", "fileSize", "fileExists", "isFileEmpty"] {
        let err = render_str(&format!("{{{{ {call}('{file}') }}}}"), &cx)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&format!("The '{call}' function needs a running execution")),
            "{call}: {err}"
        );
    }
}

/// The user's ruling: a URI assembled at render is not looked up -- an error
/// naming it and saying why. `file_functions.missing_in_own` and
/// `file_functions_extra.malformed_dotdot` were measured with `~ execution.id ~`
/// concatenation; egret is probed here with the WHOLE URI (an input value,
/// and a literal) for the artifact's answer, and the concatenated form for
/// the ruling's.
#[tokio::test(flavor = "multi_thread")]
async fn an_assembled_uri_not_in_the_snapshot_is_an_error_saying_why() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let dotdot = "kestra:///w9a/storage/ff-main/executions/2G1i2hGvCOdQ5cwWChXwwu/tasks/w/../../../../other/x.txt";
    let source = format!(
        "id: ff_main\nnamespace: {NS}\ninputs:\n  - id: f\n    type: STRING\ntasks:\n\
         \x20 - id: whole\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{{{ fileExists(inputs.f) }}}}\"\n\
         \x20 - id: dotdot\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{{{ fileExists('{dotdot}') }}}}\"\n"
    );
    // `f` is filled in once the id is known: the whole URI of a file of THIS
    // execution that was never written.
    let ex = start_execution(&db.pool, ws, None, &source, json!({"f": "pending"}))
        .await
        .unwrap();
    let own_missing = format!(
        "kestra:///w9a/t4/ff-main/executions/{ex}/tasks/w/{}/nope.txt",
        Uuid::nil()
    );
    sqlx::query("UPDATE workflow.executions SET inputs = $2 WHERE id = $1")
        .bind(ex)
        .bind(json!({ "f": own_missing }))
        .execute(&db.pool)
        .await
        .unwrap();
    drain(&db.pool).await;
    let (state, outputs, message) = result_of(&db.pool, ws, "whole").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"value": "false"}))),
        "missing_in_own, whole URI: the artifact's answer. {message:?}"
    );
    let (state, _, message) = result_of(&db.pool, ws, "dotdot").await;
    let message = message.unwrap_or_default();
    assert_eq!(state, "FAILED", "{message}");
    assert!(
        message.contains(dotdot) && message.contains("is not a storage URI egret can read"),
        "{message}"
    );

    // The same file, its URI assembled at render.
    let assembled = format!(
        "id: ff_assembled\nnamespace: {NS}\ntasks:\n  - id: built\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ fileExists('kestra:///w9a/t4/ff-assembled/executions/' ~ execution.id ~ '/tasks/w/{}/nope.txt') }}}}\"\n",
        Uuid::nil()
    );
    let ex2 = start_execution(&db.pool, ws, None, &assembled, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_of(&db.pool, ws, "built").await;
    let message = message.unwrap_or_default();
    let built = format!(
        "kestra:///w9a/t4/ff-assembled/executions/{ex2}/tasks/w/{}/nope.txt",
        Uuid::nil()
    );
    assert_eq!(state, "FAILED", "{message}");
    assert!(
        message.contains(&built) && message.contains("assembled while rendering"),
        "{message}"
    );
}

/// I3: a URI that holds `'` and a space, as a FILE input's or an output
/// file's can (`uri::parse` accepts both, `input_uri` does not encode), read
/// through the outputs map. The regex alone would cut it at the quote and the
/// read would fail as "assembled while rendering".
#[tokio::test(flavor = "multi_thread")]
async fn a_uri_with_a_quote_and_a_space_is_read_whole_from_the_outputs() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: quoted\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read(outputs.w.outputFiles[\\\"it's a.csv\\\"]) }}}}\"\n"
    );
    start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let (file, applied) = write_and_apply(
        &db.pool,
        &fs,
        ws,
        "quoted",
        "w",
        "it's a.csv",
        Some(b"a,b"),
        |u| json!({ "outputFiles": { "it's a.csv": u } }),
    )
    .await;
    applied.expect("apply");
    assert!(file.ends_with("/it's a.csv"), "{file}");
    drain(&db.pool).await;
    let (state, outputs, message) = result_of(&db.pool, ws, "r").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"value": "a,b"}))),
        "{message:?}"
    );
}

fn writer_then(flow: &str, next: &str) -> String {
    format!(
        "id: {flow}\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n{next}"
    )
}

/// I2, the worker half: a store outage while a HANDLER's properties render is
/// egret's outage (spec §11). The tick is an `Err`, the run keeps its lease
/// and records nothing, no attempt is spent; once the lease lapses and the
/// store is back, the same run is re-claimed and succeeds on its first
/// attempt.
#[tokio::test(flavor = "multi_thread")]
async fn an_outage_while_a_handler_renders_is_a_worker_error_and_spends_no_attempt() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = writer_then(
        "outage_handler",
        "  - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{ read(outputs.w.uri) }}\"\n",
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    finish_writing(&db.pool, &fs, ws, "outage_handler", "w", "o.txt", b"back").await;
    let before = attempts(&db.pool, ex, "r").await;

    OUTAGE.store(true, Ordering::SeqCst);
    let ticked = egret_workflow::worker::tick(
        &db.pool,
        16,
        &Handlers::with_core_tasks(egret_workflow::storage::current()),
    )
    .await;
    OUTAGE.store(false, Ordering::SeqCst);
    let err = ticked.expect_err("an outage is the worker's error, not the task's");
    assert!(err.to_string().contains("o.txt"), "{err}");
    let (state, outputs, message) = result_of(&db.pool, ws, "r").await;
    assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
    assert_eq!(
        attempts(&db.pool, ex, "r").await,
        before,
        "no attempt spent"
    );

    // The lease lapses; the run is re-claimed and finishes on that attempt.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
    drain(&db.pool).await;
    let (state, outputs, message) = result_of(&db.pool, ws, "r").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"value": "back"}))),
        "{message:?}"
    );
    assert_eq!(attempts(&db.pool, ex, "r").await, before);
}

/// I2, the settle half: an outage while `settle` renders a `runIf` is an `Err`
/// from `commit_outcome`, so the transaction -- the finished task's
/// outcome, its index rows, the successor -- rolls back whole.
#[tokio::test(flavor = "multi_thread")]
async fn an_outage_while_settle_renders_rolls_the_whole_transaction_back() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = writer_then(
        "outage_settle",
        "  - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ read(outputs.w.uri) == 'x' }}\"\n    message: b\n",
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let before = attempts(&db.pool, ex, "w").await;
    OUTAGE.store(true, Ordering::SeqCst);
    let (file, applied) = write_and_apply(
        &db.pool,
        &fs,
        ws,
        "outage_settle",
        "w",
        "x.txt",
        Some(b"x"),
        |u| json!({ "uri": u }),
    )
    .await;
    OUTAGE.store(false, Ordering::SeqCst);
    let err = applied.expect_err("an outage in settle is an Err");
    assert!(err.to_string().contains(&file), "{err}");
    let (state, outputs, _) = result_of(&db.pool, ws, "w").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("RUNNING", None),
        "w's outcome rolled back"
    );
    let indexed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
            .bind(&file)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(indexed, 0, "the index rows rolled back with it");
    let b: Option<String> = sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'b'",
    )
    .bind(ex)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(b, None, "and the successor was never created");
    assert_eq!(
        attempts(&db.pool, ex, "w").await,
        before,
        "no attempt spent"
    );

    // Store back, lease lapsed: the same run is finished again, and now `b` runs.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
    finish_writing(&db.pool, &fs, ws, "outage_settle", "w", "x.txt", b"x").await;
    assert_eq!(run_of(&db.pool, ex, "b").await.1, "RUNNING");
    drain(&db.pool).await;
}

/// An index row whose object is gone stays the AUTHOR's error (spec §11), on
/// both paths: the handler's run FAILS naming the URI, and a `runIf` that
/// reads it fails its run -- neither is an `Err`.
#[tokio::test(flavor = "multi_thread")]
async fn an_indexed_file_whose_object_is_gone_fails_the_run_naming_the_uri() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = writer_then(
        "gone_object",
        "  - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ read(outputs.w.uri) == 'x' }}\"\n    message: b\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{ read(outputs.w.uri) }}\"\n",
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let (file, applied) = write_and_apply(
        &db.pool,
        &fs,
        ws,
        "gone_object",
        "w",
        "gone.txt",
        None,
        |u| json!({ "uri": u }),
    )
    .await;
    applied.expect("a missing object is not an outage: settle answers Ok");
    assert_eq!(run_of(&db.pool, ex, "b").await.1, "FAILED");
    let _ = file;
    // `b` failed, so the execution fails and `r` never runs. A second
    // execution reaches the handler path.
    let source = writer_then(
        "gone_object_handler",
        "  - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{ read(outputs.w.uri) }}\"\n",
    );
    start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let (file, applied) = write_and_apply(
        &db.pool,
        &fs,
        ws,
        "gone_object_handler",
        "w",
        "gone.txt",
        None,
        |u| json!({ "uri": u }),
    )
    .await;
    applied.expect("apply");
    drain(&db.pool).await;
    let (state, _, message) = result_of(&db.pool, ws, "r").await;
    let message = message.unwrap_or_default();
    assert_eq!(state, "FAILED", "{message}");
    assert!(
        message.contains(&file) && message.contains("missing from storage"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------
// W9a Task 6: one task's collected output, staged by the next
// ---------------------------------------------------------------------------

/// A [`TaskRunner`] standing in for a container: `produce` writes `out.txt`
/// by streaming an archive of it through the shipping collector into the
/// commands' `collect_to`; `consume` writes the shipping input archive and
/// reports what `in.txt` held.
#[derive(Debug)]
struct ArchiveRunner(Arc<FileStore>);

#[async_trait::async_trait]
impl egret_workflow::worker::commands::TaskRunner for ArchiveRunner {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
        commands: &egret_workflow::worker::commands::TaskCommands,
        _spec: &egret_workflow::scripts::RunnerSpec,
        _observer: &dyn egret_workflow::worker::commands::LineObserver,
    ) -> egret_core::Result<egret_workflow::worker::commands::CommandsResult> {
        use egret_workflow::storage::transfer::{write_tar, EntryKind, TarReader};
        let mut result = egret_workflow::worker::commands::CommandsResult::default();
        if commands.commands.iter().any(|c| c == "produce") {
            let to = commands
                .collect_to
                .as_ref()
                .expect("the handler set collect_to");
            let mut archive = Vec::new();
            write_tar(
                &mut archive,
                &BTreeMap::from([(
                    "out.txt".to_string(),
                    egret_workflow::storage::transfer::TarSource::Contents("made by w"),
                )]),
                None,
            )
            .await
            .unwrap();
            let mut src = &archive[..];
            let mut reader = TarReader::new(&mut src);
            let file = egret_workflow::storage::collect::collect_declared(
                &self.0,
                to,
                "out.txt",
                false,
                &mut reader,
                &mut 0,
            )
            .await
            .expect("collected")
            .expect("a file");
            result.output_files.insert("out.txt".into(), file);
        } else {
            let mut archive = Vec::new();
            write_tar(&mut archive, &commands.tar_sources()?, Some(&self.0))
                .await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e}")))?;
            let mut src = &archive[..];
            let mut reader = TarReader::new(&mut src);
            while let Some(e) = reader.next_entry().await.unwrap() {
                if e.kind == EntryKind::File && e.name == "in.txt" {
                    let mut body = String::new();
                    tokio::io::AsyncReadExt::read_to_string(&mut reader.body(), &mut body)
                        .await
                        .unwrap();
                    result.vars.insert("staged".into(), json!(body));
                }
            }
        }
        Ok(result)
    }
    async fn adopt(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: &egret_workflow::worker::commands::TaskCommands,
        _: &egret_workflow::scripts::RunnerSpec,
        _: &dyn egret_workflow::worker::commands::LineObserver,
    ) -> egret_core::Result<Option<egret_workflow::worker::commands::CommandsResult>> {
        Ok(None)
    }
    async fn cancel(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: std::time::Duration,
        _: &egret_workflow::scripts::RunnerSpec,
    ) -> egret_core::Result<()> {
        Ok(())
    }
    async fn release(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: &egret_workflow::scripts::RunnerSpec,
    ) -> egret_core::Result<()> {
        Ok(())
    }
}

/// **A later task stages an earlier task's output URI, end to end** (Task 5's
/// reviewer's carry): `w`'s `outputFiles` is collected into the store and
/// indexed with its outcome; `r`'s `inputFiles` names
/// `{{ outputs.w.outputFiles['out.txt'] }}`, which the real claim's
/// `Snapshot::take` resolves readable, and the script handler stages it from
/// the store under `in.txt`.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_task_stages_an_earlier_tasks_output_uri_end_to_end() {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    use egret_workflow::worker::handlers::script::{self, FixedFeatureState, ScriptRuntime};
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let mut reg = Handlers::with_core_tasks(egret_workflow::storage::current());
    script::register(
        &mut reg,
        Arc::new(ScriptRuntime {
            runner_kind: egret_workflow::scripts::RunnerKind::Docker,
            runner: Arc::new(ArchiveRunner(fs.clone())),
            plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
                r#type: egret_workflow::scripts::RunnerKind::Docker
                    .fqcn()
                    .to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["*".into(), "*/*".into()]),
                    allowed_types: None,
                },
            }]),
            features: Arc::new(FixedFeatureState(true)),
            files: Some(fs.clone()),
            pool: None,
        }),
    );
    let source = format!(
        "id: chain\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: io.kestra.plugin.scripts.shell.Commands\n    \
         containerImage: alpine:3.20\n    commands: [produce]\n    outputFiles: [out.txt]\n\
         \x20 - id: r\n    type: io.kestra.plugin.scripts.shell.Commands\n    \
         containerImage: alpine:3.20\n    commands: [consume]\n    inputFiles:\n      \
         in.txt: \"{{{{ outputs.w.outputFiles['out.txt'] }}}}\"\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    for _ in 0..16 {
        if egret_workflow::worker::tick(&db.pool, 4, &reg)
            .await
            .expect("tick")
            == 0
        {
            break;
        }
    }
    let (w_state, w_out, w_msg) = result_of(&db.pool, ws, "w").await;
    assert_eq!(w_state, "SUCCESS", "{w_msg:?}");
    let uri = w_out.unwrap()["outputFiles"]["out.txt"]
        .as_str()
        .expect("a URI")
        .to_string();
    let (w_run, _) = run_of(&db.pool, ex, "w").await;
    assert_eq!(
        uri,
        format!(
            "{}{}-out.txt",
            uri::task_prefix(NS, "chain", ex, "w", w_run).unwrap(),
            uri::id_utils_from("/egret/working-dir/out.txt")
        )
    );
    let row: (Option<Uuid>, i64) = sqlx::query_as(
        "SELECT task_run_id, size FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
    )
    .bind(ws)
    .bind(&uri)
    .fetch_one(&db.pool)
    .await
    .expect("indexed with w's outcome");
    assert_eq!(row, (Some(w_run), 9));
    let (r_state, r_out, r_msg) = result_of(&db.pool, ws, "r").await;
    assert_eq!(r_state, "SUCCESS", "{r_msg:?}");
    assert_eq!(
        r_out.unwrap()["vars"]["staged"],
        json!("made by w"),
        "r staged w's file from the store"
    );
}

// ---------------------------------------------------------------------------
// W9a Task 6 fix round 1: what a collection result does to the commit
// ---------------------------------------------------------------------------

/// What [`Collecting`] makes of `w`'s run.
#[derive(Debug, Clone, Copy)]
enum Collect {
    /// Collect `a.txt` (the shipping `collect_declared`) and report it under
    /// TWO keys -- its own and the verbatim output-directory name that has
    /// its URI -- as a runner without I1's guard would have.
    OneUriTwice,
    /// Collect `a.txt`; a store failure is `Error::Internal`, as both
    /// runners' `CollectError::Store` arms answer.
    Declared,
}

/// A [`TaskRunner`] for `w`: collects as [`Collect`] says.
#[derive(Debug)]
struct Collecting(Arc<FileStore>, Collect);

#[async_trait::async_trait]
impl egret_workflow::worker::commands::TaskRunner for Collecting {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
        commands: &egret_workflow::worker::commands::TaskCommands,
        _spec: &egret_workflow::scripts::RunnerSpec,
        _observer: &dyn egret_workflow::worker::commands::LineObserver,
    ) -> egret_core::Result<egret_workflow::worker::commands::CommandsResult> {
        use egret_workflow::storage::collect::{collect_declared, CollectError};
        use egret_workflow::storage::transfer::{write_tar, TarReader, TarSource};
        let to = commands
            .collect_to
            .as_ref()
            .expect("the handler set collect_to");
        let mut archive = Vec::new();
        write_tar(
            &mut archive,
            &BTreeMap::from([("a.txt".to_string(), TarSource::Contents("made by w"))]),
            None,
        )
        .await
        .unwrap();
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let file = match collect_declared(&self.0, to, "a.txt", false, &mut reader, &mut 0).await {
            Ok(f) => f.expect("a file"),
            Err(CollectError::Store(m)) => {
                return Err(egret_core::Error::Internal(anyhow::anyhow!("{m}")))
            }
            Err(e) => panic!("{e}"),
        };
        let mut result = egret_workflow::worker::commands::CommandsResult::default();
        if let Collect::OneUriTwice = self.1 {
            let verbatim = file.uri.strip_prefix(&to.task_prefix).unwrap().to_string();
            result.output_files.insert(verbatim, file.clone());
        }
        result.output_files.insert("a.txt".into(), file);
        Ok(result)
    }
    async fn adopt(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: &egret_workflow::worker::commands::TaskCommands,
        _: &egret_workflow::scripts::RunnerSpec,
        _: &dyn egret_workflow::worker::commands::LineObserver,
    ) -> egret_core::Result<Option<egret_workflow::worker::commands::CommandsResult>> {
        Ok(None)
    }
    async fn cancel(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: std::time::Duration,
        _: &egret_workflow::scripts::RunnerSpec,
    ) -> egret_core::Result<()> {
        Ok(())
    }
    async fn release(
        &self,
        _: &egret_workflow::worker::TaskInput,
        _: &egret_workflow::scripts::RunnerSpec,
    ) -> egret_core::Result<()> {
        Ok(())
    }
}

/// The core tasks plus the script handler over `runner`, with storage `fs`.
fn collecting_registry(
    fs: &Arc<FileStore>,
    runner: Arc<dyn egret_workflow::worker::commands::TaskRunner>,
) -> Handlers {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    use egret_workflow::worker::handlers::script::{self, FixedFeatureState, ScriptRuntime};
    let mut reg = Handlers::with_core_tasks(egret_workflow::storage::current());
    script::register(
        &mut reg,
        Arc::new(ScriptRuntime {
            runner_kind: egret_workflow::scripts::RunnerKind::Docker,
            runner,
            plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
                r#type: egret_workflow::scripts::RunnerKind::Docker
                    .fqcn()
                    .to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["*".into(), "*/*".into()]),
                    allowed_types: None,
                },
            }]),
            features: Arc::new(FixedFeatureState(true)),
            files: Some(fs.clone()),
            pool: None,
        }),
    );
    reg
}

fn one_collecting_task(flow: &str) -> String {
    format!(
        "id: {flow}\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: io.kestra.plugin.scripts.shell.Commands\n    \
         containerImage: alpine:3.20\n    commands: [produce]\n    outputFiles: [a.txt]\n    \
         outputDirectory: true\n"
    )
}

async fn indexed_uris(pool: &sqlx::PgPool, execution: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT uri FROM workflow.execution_files WHERE execution_id = $1 ORDER BY uri",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// **Fix round 1, I1, end to end: two outputs naming one URI are the
/// author's FAILED task, committed once** -- not an `Internal` error from
/// the commit's `INSERT … ON CONFLICT DO UPDATE` ("cannot affect row a
/// second time"), which would leave the run to be re-claimed, re-collected
/// and refused again for ever, spending no attempt.
#[tokio::test(flavor = "multi_thread")]
async fn two_outputs_naming_one_uri_fail_the_task_and_commit_once() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let reg = collecting_registry(&fs, Arc::new(Collecting(fs.clone(), Collect::OneUriTwice)));
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &one_collecting_task("one_uri_twice"),
        json!({}),
    )
    .await
    .unwrap();
    let ticked = egret_workflow::worker::tick(&db.pool, 4, &reg).await;
    let (state, _, message) = result_of(&db.pool, ws, "w").await;
    assert!(ticked.is_ok(), "the author's, not the worker's: {ticked:?}");
    assert_eq!(state, "FAILED", "{message:?}");
    let message = message.unwrap();
    let uris = indexed_uris(&db.pool, ex).await;
    assert_eq!(uris.len(), 1, "the URI is indexed once: {uris:?}");
    assert!(
        message.contains(&uris[0]) && message.contains("a.txt"),
        "{message}"
    );
}

/// **Fix round 1, I3, end to end: the store unreachable while `w`'s output
/// is collected is a worker error and spends no attempt** (spec §11). The
/// runner's arm turns `CollectError::Store` into `Error::Internal` (pinned
/// for Docker by `commands::tests::an_unreachable_store_while_collecting_*`);
/// here the handler passes it on, the tick is an `Err`, the run keeps its
/// lease with no outcome and no index row, and once the store is back the
/// re-claimed run succeeds on its first attempt.
#[tokio::test(flavor = "multi_thread")]
async fn an_outage_while_collecting_is_a_worker_error_and_spends_no_attempt() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let reg = collecting_registry(&fs, Arc::new(Collecting(fs.clone(), Collect::Declared)));
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &one_collecting_task("collect_outage"),
        json!({}),
    )
    .await
    .unwrap();
    let before = attempts(&db.pool, ex, "w").await;
    OUTAGE.store(true, Ordering::SeqCst);
    let ticked = egret_workflow::worker::tick(&db.pool, 4, &reg).await;
    OUTAGE.store(false, Ordering::SeqCst);
    let err = ticked.expect_err("an outage is the worker's error, not the task's");
    assert!(err.to_string().contains("a.txt"), "{err}");
    let (state, outputs, message) = result_of(&db.pool, ws, "w").await;
    assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
    assert_eq!(
        attempts(&db.pool, ex, "w").await,
        before,
        "no attempt spent"
    );
    assert!(indexed_uris(&db.pool, ex).await.is_empty());

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // At least `w` (a run another failed test left claimed is reaped too).
    assert!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap() >= 1);
    for _ in 0..8 {
        if egret_workflow::worker::tick(&db.pool, 4, &reg)
            .await
            .expect("tick")
            == 0
        {
            break;
        }
    }
    let (state, _, message) = result_of(&db.pool, ws, "w").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(attempts(&db.pool, ex, "w").await, before);
    assert_eq!(indexed_uris(&db.pool, ex).await.len(), 1);
}

// ---------------------------------------------------------------------------
// Package #6 Task A4 (spec D10, user decision Q4; brought forward from Task
// B4): a `Loop`'s FILE output publishes only a file of its execution's loop
// family -- the loop root and its iterations -- once Task A4 stores it.
// ---------------------------------------------------------------------------

/// The `Loop` run `each`'s stored outputs in `execution`.
async fn each_outputs(pool: &sqlx::PgPool, execution: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'each'",
    )
    .bind(execution)
    .fetch_one(pool)
    .await
    .expect("the loop run")
}

/// The one iteration of `root`: `(history, its log lines)`.
async fn the_iteration(pool: &sqlx::PgPool, root: Uuid) -> (serde_json::Value, Vec<String>) {
    let (id, history): (Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT id, history FROM workflow.executions WHERE parent_execution_id = $1",
    )
    .bind(root)
    .fetch_one(pool)
    .await
    .expect("the iteration");
    let lines = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .expect("its lines");
    (history, lines)
}

/// **Accepted: the iteration's own file, and its loop root's.** Both are in
/// the iteration's loop family and indexed, so the entry carries both URIs.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_file_output_of_its_own_loop_family_is_kept() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = loop_source(
        "file_out_own",
        "  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n",
        "      - id: iw\n        type: io.kestra.plugin.core.log.Log\n        message: w\n",
        "    outputs:\n      - id: own\n        type: FILE\n        value: \"{{ outputs.iw.uri }}\"\n      \
         - id: root\n        type: FILE\n        value: \"{{ outputs.w.uri }}\"\n",
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let root_file = finish_writing(&db.pool, &fs, ws, "file_out_own", "w", "r.txt", b"r").await;
    let own_file = finish_writing(&db.pool, &fs, ws, "file_out_own", "iw", "i.txt", b"i").await;
    drain(&db.pool).await;

    let (history, lines) = the_iteration(&db.pool, root).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS"]),
        "{lines:?}"
    );
    assert_eq!(
        each_outputs(&db.pool, root)
            .await
            .map(|o| o["outputs"].clone()),
        Some(json!([
            {"item": {"value": "x", "iteration": 0},
             "outputs": {"own": own_file, "root": root_file}}
        ]))
    );
    assert_eq!(run_of(&db.pool, root, "each").await.1, "SUCCESS");
}

/// **Refused: anything else**, each failing the iteration after its terminal
/// (`CREATED RUNNING SUCCESS FAILED`) with `egret refuses output f: <uri> is
/// not a file of this execution`, and nothing appended:
///
/// - a file of an unrelated execution of the same namespace;
/// - a path that is not `kestra://`;
/// - a file of the execution's TRIGGER execution -- readable by `read()`
///   (the readability rule admits the trigger family) but not publishable as
///   an output, which is the narrower rule of spec D10.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_file_output_outside_its_loop_family_fails_the_iteration() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let other = format!(
        "id: file_out_src\nnamespace: {NS}\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n"
    );
    let src = start_execution(&db.pool, ws, None, &other, json!({}))
        .await
        .unwrap();
    let foreign = finish_writing(&db.pool, &fs, ws, "file_out_src", "w", "u.txt", b"u").await;
    drain(&db.pool).await;

    let looping = |flow: &str, uri: &str| {
        loop_source(
            flow,
            "",
            "      - id: r\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n",
            &format!("    outputs:\n      - id: f\n        type: FILE\n        value: \"{uri}\"\n"),
        )
    };
    let mut roots = Vec::new();
    for (flow, uri) in [
        ("file_out_foreign", foreign.as_str()),
        ("file_out_path", "file:///etc/passwd"),
    ] {
        let root = start_execution(&db.pool, ws, None, &looping(flow, uri), json!({}))
            .await
            .unwrap();
        roots.push((root, uri.to_string()));
    }
    // Started by `src` as its trigger: `read('<foreign>')` works here.
    let listener = looping("file_out_trigger", &foreign);
    let mut conn = db.pool.acquire().await.unwrap();
    let triggered = start_execution_in(
        &mut conn,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &listener,
            inputs: json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: Some(src),
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
            started_by: None,
        },
    )
    .await
    .unwrap();
    drop(conn);
    roots.push((triggered, foreign.clone()));
    drain(&db.pool).await;

    for (root, uri) in roots {
        let (history, lines) = the_iteration(&db.pool, root).await;
        assert_eq!(
            history,
            json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
            "{uri}: {lines:?}"
        );
        let want = format!("egret refuses output f: {uri} is not a file of this execution");
        assert!(lines.contains(&want), "{uri}: {lines:?}");
        assert_eq!(run_of(&db.pool, root, "each").await.1, "FAILED", "{uri}");
        assert!(
            each_outputs(&db.pool, root)
                .await
                .is_some_and(|o| o.get("outputs").is_none()),
            "{uri}: nothing appended"
        );
    }
}

/// **No master key: a SECRET `Loop` output fails the iteration, naming the
/// output and the setting** (spec D9, brought forward to Task A4), rather
/// than storing the value in clear. Here because this binary never installs
/// a cipher (`secret_inputs::install_for_tests` is process-wide, and the
/// worker suite installs it); the first assertion says so if that changes.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_loop_output_without_a_master_key_fails_the_iteration() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    assert!(
        !egret_workflow::secret_inputs::can_seal(),
        "this binary must run without a master key for this test to mean anything"
    );
    let source = loop_source(
        "secret_out_nokey",
        "",
        "      - id: r\n        type: io.kestra.plugin.core.debug.Return\n        format: hush-a4\n",
        "    outputs:\n      - id: s\n        type: SECRET\n        value: \"{{ outputs.r.value }}\"\n",
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;

    let (history, lines) = the_iteration(&db.pool, root).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("egret refuses output s:")
                && l.contains("secrets.master_key")
                && !l.contains("hush-a4")),
        "{lines:?}"
    );
    assert_eq!(run_of(&db.pool, root, "each").await.1, "FAILED");
    let stored = each_outputs(&db.pool, root).await.unwrap_or_default();
    assert!(
        !stored.to_string().contains("hush-a4"),
        "nothing stored in clear: {stored}"
    );
}

// ---------------------------------------------------------------------------
// Package #6 Task A5: `fetchType: STORE` and `AUTO` (spec D5)
// ---------------------------------------------------------------------------

/// A corpus flow's source, verbatim.
fn corpus(name: &str) -> String {
    let path = format!(
        "{}/oracle-exec/corpus/valids/{name}.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The artifact's measured `Loop` run of `entry` (`w9a-storage.json`
/// `egret_expected.loop_over_ion`).
fn loop_over_ion(entry: &str) -> serde_json::Value {
    let path = format!(
        "{}/oracle-exec/w9a-storage.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let all: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    all["egret_expected"]["loop_over_ion"][entry].clone()
}

/// The `Loop` run `task`'s stored outputs in `execution`.
async fn loop_run_outputs(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("the loop run")
}

/// The iteration `index` of `root`.
async fn iteration_of(pool: &sqlx::PgPool, root: Uuid, index: i32) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index = $2",
    )
    .bind(root)
    .bind(index)
    .fetch_one(pool)
    .await
    .expect("the iteration")
}

/// A stored object's text.
async fn object_text(fs: &FileStore, ws: Uuid, uri: &str) -> String {
    String::from_utf8(
        fs.store
            .get(&fs.object_uri(ws, uri).unwrap())
            .await
            .expect("the object"),
    )
    .expect("utf-8")
}

/// The `value` output of the one run of `task` in `execution`.
async fn value_of(
    pool: &sqlx::PgPool,
    execution: Uuid,
    task: &str,
) -> (String, Option<serde_json::Value>) {
    sqlx::query_as("SELECT state, outputs->'value' FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(execution)
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("the run")
}

/// **`valids/loop-outputs-auto`, field by field against the artifact's
/// measured run** (`w9a-storage.json` `egret_expected.loop_over_ion`): the
/// executor oracle compares states only, so A4's state bank of this entry
/// did not see that its list held FETCH maps where Kestra's AUTO over a URI
/// writes `{uri}`. Every field the artifact recorded is compared:
///
/// - `iterationCount`, `runningIterations`, `terminatedIterations` equal;
/// - `nextOffset` absent -- spec D4, egret reads file values by index
///   (recorded divergence, §5 follow-up);
/// - each entry's `item` equal to the artifact's (the record as JSON text,
///   the index, no `key`), in the same order;
/// - each entry's `outputs` is `{uri}` and nothing else, the URI the
///   artifact's layout `kestra:///{ns}/{flow}/executions/{ITERATION id}/…ion`
///   with the iteration's own id (Kestra's file name is random, egret's is
///   `outputs.ion`: the one recorded difference);
/// - the file's text is the artifact's, `{value:"some output"}`;
/// - `end`'s value is the artifact's `end_value` with those URIs.
#[tokio::test(flavor = "multi_thread")]
async fn loop_outputs_auto_matches_the_artifacts_run_field_by_field() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let golden = loop_over_ion("valids/loop-outputs-auto");
    assert_eq!(golden["state"], "SUCCESS", "the artifact's entry is there");
    let root = start_execution(&db.pool, ws, None, &corpus("loop-outputs-auto"), json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(root)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(state, golden["state"]);

    let ours = loop_run_outputs(&db.pool, root, "loop").await;
    let want = &golden["loop_outputs"];
    for counter in [
        "iterationCount",
        "runningIterations",
        "terminatedIterations",
    ] {
        assert_eq!(ours[counter], want[counter], "{counter}");
    }
    assert!(
        want.get("nextOffset").is_some() && ours.get("nextOffset").is_none(),
        "spec D4"
    );
    let (ours_list, want_list) = (
        ours["outputs"].as_array().expect("a list"),
        want["outputs"].as_array().unwrap(),
    );
    assert_eq!(ours_list.len(), want_list.len());
    let files = &golden["iteration_output_files"];
    let mut end = want_list.clone();
    for (i, (o, w)) in ours_list.iter().zip(want_list).enumerate() {
        assert_eq!(o["item"], w["item"], "entry {i}'s item");
        let keys: Vec<&String> = o["outputs"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["uri"], "entry {i}: {{uri}} only, as the artifact's");
        let uri = o["outputs"]["uri"].as_str().unwrap();
        let theirs = w["outputs"]["uri"].as_str().unwrap();
        let iteration = iteration_of(&db.pool, root, i as i32).await;
        let layout = |u: &str| u.rsplitn(3, '/').nth(2).unwrap().to_string();
        assert_eq!(layout(uri), layout(theirs), "entry {i}: the same prefix");
        assert_eq!(
            uri,
            format!(
                "kestra:///io/kestra/tests/loop-outputs-auto/executions/{}/outputs.ion",
                iteration.hyphenated()
            ),
            "entry {i}: under the ITERATION's own prefix"
        );
        assert_eq!(
            object_text(&fs, ws, uri).await,
            files[theirs]["text"].as_str().unwrap(),
            "entry {i}: the file's text"
        );
        end[i]["outputs"]["uri"] = json!(uri);
    }
    let (end_state, end_value) = value_of(&db.pool, root, "end").await;
    assert_eq!(end_state, "SUCCESS");
    let end_value: serde_json::Value =
        serde_json::from_str(end_value.unwrap().as_str().unwrap()).unwrap();
    let golden_end: serde_json::Value =
        serde_json::from_str(golden["end_value"].as_str().unwrap()).unwrap();
    assert_eq!(
        golden_end,
        json!(want_list),
        "the artifact's end renders its list"
    );
    assert_eq!(end_value, json!(end));
}

/// **`valids/loop-outputs-store`, verbatim**, field by field against what
/// survey §2 item 7 says Kestra writes (the artifact recorded this entry's
/// states only): three entries in index order, each `{item: {value,
/// iteration}, outputs: {uri}}`, the URI under that iteration's prefix, the
/// file `{value:"some output"}`, the counters, and `end` rendering the list.
#[tokio::test(flavor = "multi_thread")]
async fn loop_outputs_store_writes_one_file_per_iteration_and_lists_its_uri() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let root = start_execution(&db.pool, ws, None, &corpus("loop-outputs-store"), json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let mut want = Vec::new();
    for (i, value) in ["value 1", "value 2", "value 3"].into_iter().enumerate() {
        let iteration = iteration_of(&db.pool, root, i as i32).await;
        let uri = uri::execution_file_uri(
            "io.kestra.tests",
            "loop-outputs-store",
            iteration,
            "outputs.ion",
        )
        .unwrap();
        assert_eq!(object_text(&fs, ws, &uri).await, r#"{value:"some output"}"#);
        want.push(json!({"item": {"value": value, "iteration": i}, "outputs": {"uri": uri}}));
    }
    assert_eq!(
        loop_run_outputs(&db.pool, root, "loop").await,
        json!({
            "iterationCount": 3,
            "runningIterations": 0,
            "terminatedIterations": {"SUCCESS": 3},
            "outputs": want,
        })
    );
    let (end_state, end_value) = value_of(&db.pool, root, "end").await;
    assert_eq!(end_state, "SUCCESS");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(end_value.unwrap().as_str().unwrap()).unwrap(),
        json!(want)
    );
}

/// **A later task's `read()` of a STORE URI returns the typed map as Ion**
/// (spec D5): each output coerced by its `type` (an INT from text, a JSON
/// object, a BOOLEAN), in declared order; the file is indexed in the
/// iteration's loop family, so the ROOT reads it.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_task_reads_a_store_file_as_the_typed_map_in_ion() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: store_read\nnamespace: {NS}\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    \
         values: [a]\n    fetchType: STORE\n    outputs:\n      - id: s\n        type: STRING\n        \
         value: \"{{{{ outputs.r.value }}}}\"\n      - id: n\n        type: INT\n        value: \"3\"\n      \
         - id: j\n        type: JSON\n        value: '{{\"k\": [1, 2]}}'\n      - id: b\n        type: BOOL\n        \
         value: \"true\"\n    tasks:\n      - id: r\n        type: io.kestra.plugin.core.debug.Return\n        \
         format: \"{{{{ item.value }}}}-out\"\n  - id: later\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read(outputs.each.outputs[0].outputs.uri) }}}}\"\n"
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let (history, lines) = the_iteration(&db.pool, root).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS"]),
        "{lines:?}"
    );
    let (state, value) = value_of(&db.pool, root, "later").await;
    assert_eq!(
        (state.as_str(), value),
        (
            "SUCCESS",
            Some(json!(r#"{s:"a-out",n:3,j:{k:[1,2]},b:true}"#))
        )
    );
}

/// **`AUTO` resolves to FETCH for a list `values:` and for a string that is
/// not a URI** (Kestra's `guessOutputMode`): the entry holds the map itself
/// and no file is indexed. (`AUTO` over a URI is STORE: the corpus test above.)
#[tokio::test(flavor = "multi_thread")]
async fn auto_is_fetch_for_a_list_and_for_a_string_that_is_not_a_uri() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    for (flow, values) in [("auto_list", "[x]"), ("auto_text", "'[\"x\"]'")] {
        let source = format!(
            "id: {flow}\nnamespace: {NS}\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    \
             values: {values}\n    outputs:\n      - id: v\n        type: STRING\n        value: got\n    \
             tasks:\n      - id: r\n        type: io.kestra.plugin.core.log.Log\n        message: r\n"
        );
        let root = start_execution(&db.pool, ws, None, &source, json!({}))
            .await
            .unwrap();
        drain(&db.pool).await;
        assert_eq!(
            each_outputs(&db.pool, root)
                .await
                .map(|o| o["outputs"].clone()),
            Some(json!([{"item": {"value": "x", "iteration": 0}, "outputs": {"v": "got"}}])),
            "{flow}"
        );
        let iteration = iteration_of(&db.pool, root, 0).await;
        assert!(
            indexed_uris(&db.pool, iteration).await.is_empty(),
            "{flow}: no file"
        );
    }
}

/// **The 1 MiB per-iteration cap is FETCH's** (spec D4, D5): a map of about
/// 1.2 MB (two optional outputs, each rendered alone under the 1 MiB render
/// budget) fails its iteration under `FETCH`, naming `fetchType: STORE`, and
/// is written under `STORE`.
#[tokio::test(flavor = "multi_thread")]
async fn the_per_iteration_cap_does_not_apply_to_store() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let big = |flow: &str, fetch: &str| {
        format!(
            "id: {flow}\nnamespace: {NS}\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    \
             values: [x]\n    fetchType: {fetch}\n    outputs:\n      - id: a\n        type: STRING\n        \
             required: false\n        value: \"{{{{ range(0, 50000) | join('aaaaaaaaaaaa') }}}}\"\n      - id: b\n        type: STRING\n        \
             required: false\n        value: \"{{{{ range(0, 50000) | join('bbbbbbbbbbbb') }}}}\"\n    tasks:\n      - id: r\n        \
             type: io.kestra.plugin.core.log.Log\n        message: r\n"
        )
    };
    let fetched = start_execution(&db.pool, ws, None, &big("cap_fetch", "FETCH"), json!({}))
        .await
        .unwrap();
    let stored = start_execution(&db.pool, ws, None, &big("cap_store", "STORE"), json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;

    let (history, lines) = the_iteration(&db.pool, fetched).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
        "{lines:?} {:?}",
        each_outputs(&db.pool, fetched)
            .await
            .map(|o| o.to_string().len())
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Loop outputs of iteration 0 are ")
                && l.contains("Use fetchType: STORE")),
        "{lines:?}"
    );

    let (history, lines) = the_iteration(&db.pool, stored).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS"]),
        "{lines:?}"
    );
    let iteration = iteration_of(&db.pool, stored, 0).await;
    let uri = uri::execution_file_uri(NS, "cap_store", iteration, "outputs.ion").unwrap();
    assert_eq!(
        each_outputs(&db.pool, stored)
            .await
            .map(|o| o["outputs"].clone()),
        Some(json!([{"item": {"value": "x", "iteration": 0}, "outputs": {"uri": uri}}]))
    );
    assert_eq!(
        indexed_uris(&db.pool, iteration).await,
        std::slice::from_ref(&uri)
    );
    let text = object_text(&fs, ws, &uri).await;
    assert!(
        text.len() > 1_200_000 && text.starts_with("{a:\"0aaaaaaaaaaaa1aaa"),
        "{}",
        text.len()
    );
}

/// **A SECRET output under `AUTO` resolved to STORE is refused at
/// evaluation** (spec D9, §2.2 item 8): save refuses `AUTO` with a string
/// `values:`; a stored version that bypassed it (here: `start_execution`
/// takes the source as stored) fails its iteration before anything is
/// rendered, with a message naming the output and how STORE was reached,
/// and never the value. Checked BEFORE the no-master-key refusal this binary
/// would otherwise give.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_output_under_auto_over_a_file_is_refused_at_evaluation() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: secret_auto\nnamespace: {NS}\ntasks:\n  - id: write\n    type: io.kestra.plugin.core.storage.Write\n    \
         content: \"1\"\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: \"{{{{ outputs.write.uri }}}}\"\n    \
         outputs:\n      - id: s\n        type: SECRET\n        value: hush-a5\n    tasks:\n      - id: r\n        \
         type: io.kestra.plugin.core.log.Log\n        message: r\n"
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let (history, lines) = the_iteration(&db.pool, root).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
        "{lines:?}"
    );
    assert!(
        lines.contains(
            &"egret refuses output s: a SECRET output cannot be written to internal \
             storage (fetchType is AUTO and the Loop's values: rendered to a file, so its outputs \
             are stored): the iteration's outputs file would hold the value in clear. Use \
             fetchType: FETCH"
                .to_string()
        ),
        "{lines:?}"
    );
    assert!(lines.iter().all(|l| !l.contains("hush-a5")));
    assert_eq!(run_of(&db.pool, root, "each").await.1, "FAILED");
    let iteration = iteration_of(&db.pool, root, 0).await;
    assert!(
        indexed_uris(&db.pool, iteration).await.is_empty(),
        "no file written"
    );
}

// ---------------------------------------------------------------------------
// Package #6 Task B1: the flow-level `outputs:` (spec D6). SECRET (D9) and
// FILE (D10) rules brought forward from Task B4, so that B1 never stores a
// SECRET value in clear or publishes another execution's file.
// ---------------------------------------------------------------------------

/// The execution's history, stored `outputs`, and its log lines.
async fn execution_end(
    pool: &sqlx::PgPool,
    ex: Uuid,
) -> (serde_json::Value, Option<serde_json::Value>, Vec<String>) {
    let (history, outputs): (serde_json::Value, Option<serde_json::Value>) =
        sqlx::query_as("SELECT history, outputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(pool)
            .await
            .expect("the execution");
    let lines = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(ex)
    .fetch_all(pool)
    .await
    .expect("lines");
    (history, outputs, lines)
}

/// **Accepted: a FILE flow output naming a file of the execution's own task.**
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_file_output_of_its_own_execution_is_kept() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: flow_file_own\nnamespace: {NS}\ntasks:\n  - id: w\n    \
         type: io.kestra.plugin.core.log.Log\n    message: w\noutputs:\n  - id: f\n    \
         type: FILE\n    value: \"{{{{ outputs.w.uri }}}}\"\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let file = finish_writing(&db.pool, &fs, ws, "flow_file_own", "w", "o.txt", b"o").await;
    drain(&db.pool).await;

    let (history, outputs, lines) = execution_end(&db.pool, ex).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS"]),
        "{lines:?}"
    );
    assert_eq!(outputs, Some(json!({ "f": file })));
}

/// **Refused: a FILE flow output naming anything else** -- a file of an
/// unrelated execution, a `file://` path, a file of the execution's TRIGGER
/// execution (readable by `read()`, not publishable) -- fails the execution
/// after its terminal with `egret refuses output f: <uri> is not a file of
/// this execution`, and nothing is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_file_output_outside_its_execution_fails_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let other = format!(
        "id: flow_file_src\nnamespace: {NS}\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n"
    );
    let src = start_execution(&db.pool, ws, None, &other, json!({}))
        .await
        .unwrap();
    let foreign = finish_writing(&db.pool, &fs, ws, "flow_file_src", "w", "u.txt", b"u").await;
    drain(&db.pool).await;

    let publishing = |flow: &str, uri: &str| {
        format!(
            "id: {flow}\nnamespace: {NS}\ntasks:\n  - id: r\n    \
             type: io.kestra.plugin.core.debug.Return\n    format: x\noutputs:\n  - id: f\n    \
             type: FILE\n    value: \"{uri}\"\n"
        )
    };
    let mut roots = Vec::new();
    for (flow, uri) in [
        ("flow_file_foreign", foreign.as_str()),
        ("flow_file_path", "file:///etc/passwd"),
    ] {
        let ex = start_execution(&db.pool, ws, None, &publishing(flow, uri), json!({}))
            .await
            .unwrap();
        roots.push((ex, uri.to_string()));
    }
    let listener = publishing("flow_file_trigger", &foreign);
    let mut conn = db.pool.acquire().await.unwrap();
    let triggered = start_execution_in(
        &mut conn,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: &listener,
            inputs: json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: Some(src),
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
            started_by: None,
        },
    )
    .await
    .unwrap();
    drop(conn);
    roots.push((triggered, foreign.clone()));
    drain(&db.pool).await;

    for (ex, uri) in roots {
        let (history, outputs, lines) = execution_end(&db.pool, ex).await;
        assert_eq!(
            history,
            json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
            "{uri}: {lines:?}"
        );
        let want = format!("egret refuses output f: {uri} is not a file of this execution");
        assert!(lines.iter().any(|l| l == &want), "{want}: {lines:?}");
        assert_eq!(outputs, None, "{uri}: nothing stored");
    }
}

/// **A SECRET flow output on a deployment with no master key fails the
/// execution**, naming the output and the setting and never the value, and
/// nothing is stored (spec D9).
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_flow_output_without_a_master_key_fails_the_execution() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    assert!(
        !egret_workflow::secret_inputs::can_seal(),
        "this binary must run without a master key for this test to mean anything"
    );
    let source = format!(
        "id: flow_secret_nokey\nnamespace: {NS}\ntasks:\n  - id: r\n    \
         type: io.kestra.plugin.core.debug.Return\n    format: hush-b1\noutputs:\n  - id: s\n    \
         type: SECRET\n    value: \"{{{{ outputs.r.value }}}}\"\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;

    let (history, outputs, lines) = execution_end(&db.pool, ex).await;
    assert_eq!(
        history,
        json!(["CREATED", "RUNNING", "SUCCESS", "FAILED"]),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("egret refuses output s:")
                && l.contains("secrets.master_key")
                && !l.contains("hush-b1")),
        "{lines:?}"
    );
    assert_eq!(outputs, None, "nothing stored");
}
