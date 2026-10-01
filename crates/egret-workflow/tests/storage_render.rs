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
//! `storage::install_mem_for_tests`'s `MemStore`: nothing here touches MinIO.
//!
//! # How a file gets written
//!
//! [`finish_writing`] stands in for a handler that wrote a file: it claims
//! the run, puts the bytes, and hands them to `apply_outcome` as
//! `Outcome::files` -- which, since W9a Task 6, indexes them **in the
//! transaction it then settles in**, and commits once. The script handler's
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
    apply_outcome, claim, execution_render_context, reap, start_execution, start_execution_in,
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
    let reg = Handlers::with_core_tasks();
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
/// outputs from the URI. Commits only when `apply_outcome` answered `Ok`,
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
    let mut tx = pool.begin().await.unwrap();
    let outcome = Outcome {
        files: vec![StoredFile {
            uri: file.clone(),
            size: bytes.map_or(3, |b| b.len() as u64),
        }],
        ..Outcome::succeeded_with(outputs(&file))
    };
    let applied = apply_outcome(&mut tx, c, &outcome).await;
    if applied.is_ok() {
        tx.commit().await.unwrap();
    }
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
    let ticked = egret_workflow::worker::tick(&db.pool, 16, &Handlers::with_core_tasks()).await;
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
/// from `apply_outcome`, so the caller's transaction -- the finished task's
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
    let mut reg = Handlers::with_core_tasks();
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
    let mut reg = Handlers::with_core_tasks();
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
