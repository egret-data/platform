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
//! # How a file gets written before Task 6
//!
//! No handler writes files yet (Task 6 indexes `outputFiles` in
//! `apply_outcome`'s transaction). [`finish_writing`] does what that will do,
//! in the same place: it claims the run, puts the bytes, indexes them **in the
//! transaction `apply_outcome` then settles in**, and commits once.

mod common;

use std::sync::Arc;

use egret_metastore::Db;
use egret_workflow::render_str;
use egret_workflow::storage::{self, index_task_files, uri, FileStore, StoredFile};
use egret_workflow::worker::{
    apply_outcome, claim, execution_render_context, start_execution, start_execution_in, Handlers,
    NewExecution, Outcome,
};
use serde_json::json;
use uuid::Uuid;

/// Async, so a turn can be held across `.await` (a std guard cannot), and
/// runtime-agnostic, so each test's own runtime can wait on it.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NS: &str = "w9a.t4";

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    let (ws, _) = common::seed_workspace(&db).await;
    Some((db, ws, storage::install_mem_for_tests()))
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
    let claimed = claim(pool, 1).await.expect("claim");
    assert_eq!(claimed.len(), 1, "exactly the writing run is claimable");
    let c = &claimed[0];
    assert_eq!(c.task_id, task);
    let file = format!(
        "{}{name}",
        uri::task_prefix(NS, flow_id, c.execution_id, task, c.id).unwrap()
    );
    put(fs, ws, &file, bytes).await;
    let mut tx = pool.begin().await.unwrap();
    index_task_files(
        &mut tx,
        ws,
        c.id,
        &[StoredFile {
            uri: file.clone(),
            size: bytes.len() as u64,
        }],
    )
    .await
    .expect("index");
    apply_outcome(&mut tx, c, &Outcome::succeeded_with(json!({ "uri": file })))
        .await
        .expect("apply");
    tx.commit().await.unwrap();
    file
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
/// The root learns the URI through its `inputs` (rewritten once the iteration
/// has written it): egret does not yet surface an iteration's outputs to the
/// root, and a value of the render context is one of the two places a URI
/// may come from.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_after_the_loop_reads_a_file_an_iteration_wrote() {
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
