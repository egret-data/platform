//! W9b Task 4: the five file functions over namespace files, through the
//! real worker (W9b spec §6). Requires DATABASE_URL.
//!
//! Namespace files are seeded with Task 3's `put` (and `create_directory`,
//! `soft_delete`) into `storage::install_mem_for_tests`'s `MemStore`: nothing
//! here touches MinIO. A render looks them up on a connection of its own,
//! from `nsfiles::install_db_for_tests`'s options (`storage::scope`'s module
//! doc).
//!
//! # Why `multi_thread`, and why the tests take turns
//!
//! As `tests/storage_render.rs`: a namespace lookup blocks the render thread
//! on the runtime, which needs the multi-thread runtime, and a claim is global
//! (a worker serves every workspace), so the tests take turns on [`SERIAL`].
//! Every test makes its own workspace, so their namespace files never meet.
//!
//! The outage test is in `tests/nsfiles_render_outage.rs`: it installs
//! connect options that reach nothing, and those are process-wide.

mod common;

use std::sync::Arc;

use egret_metastore::Db;
use egret_workflow::runners::files::{FileScope, NsFileArgs, RenderFiles};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{
    create_directory, put, soft_delete, Conflict, Written,
};
use egret_workflow::storage::nsfiles::{self, CreatedBy};
use egret_workflow::storage::scope::Snapshot;
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::{execution_render_context, start_execution, Handlers};
use egret_workflow::{render_str, RenderContext};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The Task 1 artifact (`oracle-exec/w9b-namespace-files.json`).
const ARTIFACT: &str = include_str!("../oracle-exec/w9b-namespace-files.json");

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    nsfiles::install_db_for_tests(&db.pool);
    let fs = storage::install_mem_for_tests();
    assert!(
        storage::current().is_some(),
        "the worker reads the installed store"
    );
    let (ws, _) = common::seed_workspace(&db).await;
    Some((db, ws, fs))
}

fn by() -> CreatedBy {
    CreatedBy::Execution(Uuid::nil())
}

/// One revision of `path` in `ns` of `ws`, as an API upload writes it.
async fn put_file(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, path: &str, bytes: &[u8]) {
    let grant = authorize(ws, Actor::Api, ns, Access::Write).unwrap();
    let mut body: &[u8] = bytes;
    match put(
        &db.pool,
        fs,
        &grant,
        &NsPath::parse_file(path).unwrap(),
        &mut body,
        Conflict::Overwrite,
        by(),
    )
    .await
    {
        Ok(Written::Written(_)) => {}
        other => panic!("put {ns} {path}: {other:?}"),
    }
}

async fn mkdir(db: &Db, ws: Uuid, ns: &str, path: &str) {
    let grant = authorize(ws, Actor::Api, ns, Access::Write).unwrap();
    create_directory(&db.pool, &grant, &NsPath::parse_dir(path).unwrap(), by())
        .await
        .unwrap();
}

async fn delete(db: &Db, ws: Uuid, ns: &str, path: &str) {
    let grant = authorize(ws, Actor::Api, ns, Access::Write).unwrap();
    let marked = soft_delete(&db.pool, &grant, &NsPath::parse_file(path).unwrap())
        .await
        .unwrap();
    assert_eq!(marked.len(), 1, "{path}");
}

/// Tick until nothing is claimable. A flow here can hold ~70 sequential
/// tasks, so the bound is wider than `storage_render`'s.
async fn drain(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(storage::current());
    for _ in 0..4096 {
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

/// `(state, outputs, message)` of `task` in execution `ex`.
async fn result_of(
    pool: &sqlx::PgPool,
    ex: Uuid,
    task: &str,
) -> (String, Option<Value>, Option<String>) {
    sqlx::query_as(
        "SELECT state, outputs, message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("run {task} of {ex}: {e}"))
}

/// A `Return` task whose `format` is `{{ expr }}`; it may fail without
/// stopping the flow, as the artifact's probes did.
fn ret(id: &str, expr: &str) -> String {
    let format = serde_json::to_string(&format!("{{{{ {expr} }}}}")).unwrap();
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.debug.Return\n    allowFailure: true\n    \
         format: {format}\n"
    )
}

fn flow(id: &str, ns: &str, tasks: &[String]) -> String {
    format!("id: {id}\nnamespace: {ns}\ntasks:\n{}", tasks.concat())
}

async fn run(db: &Db, ws: Uuid, source: &str, inputs: Value) -> Uuid {
    let ex = start_execution(&db.pool, ws, None, source, inputs)
        .await
        .unwrap();
    drain(&db.pool).await;
    ex
}

/// The value the run returned, or panic with its message.
async fn value_of(db: &Db, ex: Uuid, task: &str) -> String {
    let (state, outputs, message) = result_of(&db.pool, ex, task).await;
    match (state.as_str(), outputs) {
        ("SUCCESS", Some(o)) => o["value"].as_str().expect("a string value").to_string(),
        (state, outputs) => panic!("{task}: {state} {outputs:?} {message:?}"),
    }
}

/// The message a run failed with, or panic if it did not fail.
async fn error_of(db: &Db, ex: Uuid, task: &str) -> String {
    let (state, outputs, message) = result_of(&db.pool, ex, task).await;
    assert!(
        state != "SUCCESS" && outputs.is_none(),
        "{task} answered: {state} {outputs:?}"
    );
    message.unwrap_or_else(|| panic!("{task} failed with no message"))
}

/// Task 1's `w9b.fn` and `w9b.other` (`file_uri._setup`), and
/// `io.kestra.tests`' `/a/b.txt` x3 and `/d` (`uri`).
async fn seed_the_artifacts_namespaces(db: &Db, fs: &FileStore, ws: Uuid) {
    let f = "w9b.fn";
    put_file(db, fs, ws, f, "/fu/one.txt", b"one").await;
    for r in ["r1", "r2", "r3"] {
        put_file(db, fs, ws, f, "/fu/three.txt", r.as_bytes()).await;
    }
    put_file(db, fs, ws, f, "/fu/gone.txt", b"gone").await;
    delete(db, ws, f, "/fu/gone.txt").await;
    put_file(db, fs, ws, f, "/fu/empty.txt", b"").await;
    put_file(db, fs, ws, f, "/fu/bin.dat", &[0xff, 0xfe, 0x00, 0x81]).await;
    put_file(db, fs, ws, "w9b.other", "/o.txt", b"other content").await;
    let u = "io.kestra.tests";
    for r in ["rev1", "rev2", "rev3"] {
        put_file(db, fs, ws, u, "/a/b.txt", r.as_bytes()).await;
    }
    mkdir(db, ws, u, "/d").await;
}

/// The artifact's probes (`w9b_m2_flows.py`'s `fu`, `rd` and `targets`,
/// and the `uri` flow), each with the fixture cell it answers.
fn probes() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut p = vec![
        ("w9b.fn", "/file_uri/present_rev1", "fileURI('/fu/one.txt')"),
        (
            "w9b.fn",
            "/file_uri/present_rev3",
            "fileURI('/fu/three.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/with_revision_1",
            "fileURI('/fu/three.txt', revision=1)",
        ),
        (
            "w9b.fn",
            "/file_uri/with_revision_2",
            "fileURI('/fu/three.txt', revision=2)",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/with_revision_3",
            "fileURI('/fu/three.txt', revision=3)",
        ),
        (
            "w9b.fn",
            "/file_uri/with_revision_missing",
            "fileURI('/fu/three.txt', revision=9)",
        ),
        (
            "w9b.fn",
            "/file_uri/revision_non_integer",
            "fileURI('/fu/three.txt', revision='x')",
        ),
        ("w9b.fn", "/file_uri/missing", "fileURI('/fu/nope.txt')"),
        (
            "w9b.fn",
            "/file_uri/soft_deleted",
            "fileURI('/fu/gone.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/soft_deleted_revision_1",
            "fileURI('/fu/gone.txt', revision=1)",
        ),
        ("w9b.fn", "/file_uri/dotdot", "fileURI('/fu/../fu/one.txt')"),
        (
            "w9b.fn",
            "/file_uri/_extra/dotdot_nsfile",
            "fileURI('nsfile:///fu/../fu/one.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/kestra_arg",
            "fileURI('kestra:///w9b/fn/_files/fu/one.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/kestra_arg_other_ns",
            "fileURI('kestra:///w9b/other/_files/o.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/other_namespace_authority",
            "fileURI('nsfile://w9b.other/o.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/other_namespace_arg",
            "fileURI('o.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/authority_and_namespace_arg",
            "fileURI('nsfile://w9b.other/o.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/bare_no_slash",
            "fileURI('fu/one.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/nsfile_own",
            "fileURI('nsfile:///fu/three.txt')",
        ),
        ("w9b.fn", "/file_uri/_extra/directory", "fileURI('/fu')"),
        (
            "w9b.fn",
            "/file_uri/_extra/unsupported_scheme",
            "fileURI('s3://bucket/x')",
        ),
        ("w9b.fn", "/read/bare", "read('fu/one.txt')"),
        ("w9b.fn", "/read/bare_leading_slash", "read('/fu/one.txt')"),
        ("w9b.fn", "/read/nsfile_own", "read('nsfile:///fu/one.txt')"),
        (
            "w9b.fn",
            "/read/nsfile_other",
            "read('nsfile://w9b.other/o.txt')",
        ),
        (
            "w9b.fn",
            "/read/nsfile_other_with_namespace_arg",
            "read('nsfile://w9b.other/o.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/read/_extra/nsfile_own_with_namespace_arg",
            "read('nsfile:///o.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/read/_extra/bare_with_namespace_arg",
            "read('o.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/read/kestra_own_no_vN_latest_is_3",
            "read('kestra:///w9b/fn/_files/fu/three.txt')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_own_v2",
            "read('kestra:///w9b/fn/_files/fu/three.txt.v2')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_own_v3",
            "read('kestra:///w9b/fn/_files/fu/three.txt.v3')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_own_soft_deleted",
            "read('kestra:///w9b/fn/_files/fu/gone.txt')",
        ),
        (
            "w9b.fn",
            "/read/kestra_other",
            "read('kestra:///w9b/other/_files/o.txt')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_of_fileURI_latest",
            "read(fileURI('/fu/three.txt'))",
        ),
        (
            "w9b.fn",
            "/read/revision_2",
            "read('/fu/three.txt', revision=2)",
        ),
        (
            "w9b.fn",
            "/read/_extra/revision_missing",
            "read('/fu/three.txt', revision=9)",
        ),
        (
            "w9b.fn",
            "/read/_extra/revision_non_integer",
            "read('/fu/three.txt', revision='x')",
        ),
        (
            "w9b.fn",
            "/read/_extra/soft_deleted",
            "read('/fu/gone.txt')",
        ),
        ("w9b.fn", "/read/_extra/missing", "read('/fu/nope.txt')"),
        ("w9b.fn", "/read/_extra/directory", "read('/fu')"),
        ("w9b.fn", "/read/_extra/empty", "read('/fu/empty.txt')"),
        ("w9b.fn", "/read/binary", "read('/fu/bin.dat')"),
        ("w9b.fn", "/read/_extra/dotdot", "read('/fu/../fu/one.txt')"),
        // Fix round 1: the coordinator's rulings (M2, M3), as chosen cells.
        (
            "w9b.fn",
            "/file_uri/_extra/kestra_arg_with_revision",
            "fileURI('kestra:///w9b/fn/_files/fu/three.txt', revision=2)",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/kestra_arg_missing",
            "fileURI('kestra:///w9b/fn/_files/fu/nope.txt')",
        ),
        (
            "w9b.fn",
            "/file_uri/_extra/kestra_arg_soft_deleted",
            "fileURI('kestra:///w9b/fn/_files/fu/gone.txt')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_with_revision",
            "read('kestra:///w9b/fn/_files/fu/three.txt', revision=2)",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_with_other_namespace_arg",
            "read('kestra:///w9b/fn/_files/fu/one.txt', namespace='w9b.other')",
        ),
        (
            "w9b.fn",
            "/read/_extra/kestra_with_same_namespace_arg",
            "read('kestra:///w9b/fn/_files/fu/one.txt', namespace='w9b.fn')",
        ),
        (
            "io.kestra.tests",
            "/uri/fileURI/latest(3)",
            "fileURI('/a/b.txt')",
        ),
        (
            "io.kestra.tests",
            "/uri/fileURI/1",
            "fileURI('/a/b.txt', revision=1)",
        ),
        (
            "io.kestra.tests",
            "/uri/fileURI/2",
            "fileURI('/a/b.txt', revision=2)",
        ),
        (
            "io.kestra.tests",
            "/uri/fileURI/3",
            "fileURI('/a/b.txt', revision=3)",
        ),
        (
            "io.kestra.tests",
            "/uri/directory/fileURI('~1d')",
            "fileURI('/d')",
        ),
        (
            "io.kestra.tests",
            "/uri/directory/fileURI('~1d~1')",
            "fileURI('/d/')",
        ),
        (
            "io.kestra.tests",
            "/uri/directory/fileURI('~1d', revision=2)",
            "fileURI('/d', revision=2)",
        ),
        (
            "io.kestra.tests",
            "/uri/read_of_fileURI_latest",
            "read(fileURI('/a/b.txt'))",
        ),
    ];
    for (name, cells) in [
        ("fileSize", FOUR_TARGETS),
        ("fileExists", FOUR_TARGETS),
        ("isFileEmpty", FOUR_TARGETS),
    ] {
        for (cell, arg) in cells {
            let pointer: &'static str =
                Box::leak(format!("/four_functions_ns/{name}/{cell}").into_boxed_str());
            let expr: &'static str = Box::leak(format!("{name}({arg})").into_boxed_str());
            p.push(("w9b.fn", pointer, expr));
        }
    }
    p
}

/// `w9b_m2_flows.py`'s `targets`.
const FOUR_TARGETS: &[(&str, &str)] = &[
    ("present", "'/fu/one.txt'"),
    ("empty", "'/fu/empty.txt'"),
    ("missing", "'/fu/nope.txt'"),
    ("soft_deleted", "'/fu/gone.txt'"),
    ("directory", "'/fu'"),
    ("nsfile_other", "'nsfile://w9b.other/o.txt'"),
    ("kestra_own_no_vN", "'kestra:///w9b/fn/_files/fu/three.txt'"),
    (
        "kestra_own_soft_deleted",
        "'kestra:///w9b/fn/_files/fu/gone.txt'",
    ),
    (
        "kestra_own_missing",
        "'kestra:///w9b/fn/_files/fu/nope.txt'",
    ),
    ("binary", "'/fu/bin.dat'"),
];

/// What an error cell's message must name: the path its probe passed, in
/// the form egret names it (a leading '/' and no trailing one; no scheme or authority; a
/// `kestra:///` URI's path after `_files`). A scheme egret does not read is
/// named whole.
fn named_path(expr: &str) -> String {
    let arg = expr.split('\'').nth(1).expect("a quoted path");
    if let Some(rest) = arg.strip_prefix("nsfile://") {
        return rest[rest.find('/').unwrap()..].to_string();
    }
    if arg.starts_with("kestra:///") {
        return arg[arg.find("/_files").unwrap() + "/_files".len()..].to_string();
    }
    if arg.contains("://") {
        return arg.to_string();
    }
    // The stored form a message names: a leading '/', no trailing one (M7).
    format!("/{}", arg.trim_matches('/'))
}

/// Every cell of `file_uri`, `read`, `four_functions_ns` and `uri` that
/// holds a `value` or an `error`, by pointer.
fn cells(at: &Value, pointer: String, out: &mut Vec<String>) {
    let Some(map) = at.as_object() else { return };
    if map.contains_key("value") || map.contains_key("error") {
        out.push(pointer);
        return;
    }
    for (k, v) in map {
        let k = k.replace('~', "~0").replace('/', "~1");
        cells(v, format!("{pointer}/{k}"), out);
    }
}

/// Every `file_uri`, `read`, `four_functions_ns` and `uri` cell of the
/// artifact, through a real task run's render: egret's answer is the cell's
/// top level (a chosen cell's Kestra answer is under `kestra` and is not
/// asserted). A `value` is the Return's output; an `error` fails the run
/// with a message naming the path. And no render created anything.
#[tokio::test(flavor = "multi_thread")]
async fn the_functions_match_the_artifact() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let artifact: Value = serde_json::from_str(ARTIFACT).unwrap();
    let expected = &artifact["egret_expected"];
    let probes = probes();

    // The probes cover every cell, and nothing else.
    let mut every = Vec::new();
    for key in ["file_uri", "read", "four_functions_ns", "uri"] {
        cells(&expected[key], format!("/{key}"), &mut every);
    }
    let mut probed: Vec<String> = probes.iter().map(|(_, p, _)| p.to_string()).collect();
    every.sort();
    probed.sort();
    assert_eq!(probed, every, "the probes are the artifact's cells");

    seed_the_artifacts_namespaces(&db, &fs, ws).await;
    let rows_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.namespace_files WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let mut checked = 0;
    for ns in ["w9b.fn", "io.kestra.tests"] {
        let mine: Vec<_> = probes.iter().filter(|(n, _, _)| *n == ns).collect();
        let tasks: Vec<String> = mine
            .iter()
            .enumerate()
            .map(|(i, (_, _, expr))| ret(&format!("p{i:02}"), expr))
            .collect();
        let ex = run(&db, ws, &flow("fn", ns, &tasks), json!({})).await;
        for (i, (_, pointer, expr)) in mine.iter().enumerate() {
            let task = format!("p{i:02}");
            let cell = expected
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{pointer}"));
            if let Some(want) = cell.get("value") {
                let want = want.as_str().unwrap();
                assert_eq!(value_of(&db, ex, &task).await, want, "{pointer}: {expr}");
            } else {
                assert!(cell.get("error").is_some(), "{pointer}");
                let message = error_of(&db, ex, &task).await;
                let path = named_path(expr);
                assert!(
                    message.contains(&path),
                    "{pointer}: {expr}: the message names {path:?}: {message}"
                );
                assert!(!message.contains(&ws.to_string()), "{pointer}: {message}");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, probes.len());

    let rows_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.namespace_files WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows_after, rows_before, "a render created nothing");
}

/// The settle path, which has no pool: `b`'s `runIf` renders inside the
/// transaction that finishes `a`, and the lookup reads a namespace file on
/// the scope's own connection. The control (`c`, a file that is not there)
/// shows the `runIf` was evaluated.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_if_in_settle_reads_a_namespace_file() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.settle", "/cfg/on.txt", b"on").await;
    let source = "id: settle\nnamespace: w9b.settle\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ fileExists('cfg/on.txt') }}\"\n    message: b\n\
         \x20 - id: c\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ fileExists('cfg/off.txt') }}\"\n    message: c\n";
    let ex = run(&db, ws, source, json!({})).await;
    assert_eq!(result_of(&db.pool, ex, "b").await.0, "SUCCESS");
    assert_eq!(result_of(&db.pool, ex, "c").await.0, "SKIPPED");
}

/// The snapshot rule does not apply to namespace files: a path assembled at
/// render is looked up (spec §6).
#[tokio::test(flavor = "multi_thread")]
async fn a_path_assembled_at_render_is_read() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.asm", "/a/b.txt", b"assembled").await;
    let source = format!(
        "id: asm\nnamespace: w9b.asm\ninputs:\n  - id: x\n    type: STRING\ntasks:\n{}",
        ret("r", "read('a/' ~ inputs.x)")
    );
    let ex = run(&db, ws, &source, json!({"x": "b.txt"})).await;
    assert_eq!(value_of(&db, ex, "r").await, "assembled");
}

/// R1: an explicit revision of a soft-deleted path answers; its latest is
/// not-found, naming the path (w9b `file_uri._extra.soft_deleted_revision_1`,
/// `read._extra.kestra_own_soft_deleted`, `four_functions_ns.*.kestra_own_soft_deleted`).
#[tokio::test(flavor = "multi_thread")]
async fn a_revision_of_a_soft_deleted_path_is_read_and_its_latest_is_not() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.fn", "/fu/gone.txt", b"gone").await;
    delete(&db, ws, "w9b.fn", "/fu/gone.txt").await;
    let k = "'kestra:///w9b/fn/_files/fu/gone.txt'";
    let answers = [
        ("r1", "read('/fu/gone.txt', revision=1)".to_string(), "gone"),
        (
            "u1",
            "fileURI('/fu/gone.txt', revision=1)".to_string(),
            "kestra:///w9b/fn/_files/fu/gone.txt",
        ),
        ("kr", format!("read({k})"), "gone"),
        ("ks", format!("fileSize({k})"), "4"),
        ("ke", format!("fileExists({k})"), "true"),
        ("ki", format!("isFileEmpty({k})"), "false"),
        ("le", "fileExists('/fu/gone.txt')".to_string(), "false"),
    ];
    let fails = [
        ("lr", "read('/fu/gone.txt')"),
        ("lu", "fileURI('/fu/gone.txt')"),
        ("ls", "fileSize('/fu/gone.txt')"),
        ("li", "isFileEmpty('/fu/gone.txt')"),
    ];
    let mut tasks: Vec<String> = answers.iter().map(|(id, e, _)| ret(id, e)).collect();
    tasks.extend(fails.iter().map(|(id, e)| ret(id, e)));
    let ex = run(&db, ws, &flow("r1", "w9b.fn", &tasks), json!({})).await;
    for (id, expr, want) in answers {
        assert_eq!(value_of(&db, ex, id).await, want, "{expr}");
    }
    for (id, expr) in fails {
        let message = error_of(&db, ex, id).await;
        assert!(
            message.contains("'/fu/gone.txt'") && message.contains("does not exist"),
            "{expr}: {message}"
        );
    }
}

/// §5: a flow reads any namespace of its workspace -- by authority, by the
/// `namespace` argument and by a `kestra:///other/ns/_files/…` URI.
#[tokio::test(flavor = "multi_thread")]
async fn another_namespace_of_the_workspace_is_read_by_authority_and_by_kestra_uri() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.shared.cfg", "/x.txt", b"shared").await;
    let tasks = [
        ret("auth", "read('nsfile://w9b.shared.cfg/x.txt')"),
        ret("arg", "read('x.txt', namespace='w9b.shared.cfg')"),
        ret("uri", "read('kestra:///w9b/shared/cfg/_files/x.txt')"),
        ret("furi", "fileURI('nsfile://w9b.shared.cfg/x.txt')"),
        ret("own", "fileExists('x.txt')"),
    ];
    let ex = run(&db, ws, &flow("reader", "w9b.reader", &tasks), json!({})).await;
    for task in ["auth", "arg", "uri"] {
        assert_eq!(value_of(&db, ex, task).await, "shared", "{task}");
    }
    assert_eq!(
        value_of(&db, ex, "furi").await,
        "kestra:///w9b/shared/cfg/_files/x.txt"
    );
    // The control: the same path is not the reader's own namespace's file.
    assert_eq!(value_of(&db, ex, "own").await, "false");
}

/// The workspace is the hard boundary: workspace B holds the same namespace
/// and path; workspace A's render does not find it and never sees its bytes,
/// and the message names the path and nothing of B.
#[tokio::test(flavor = "multi_thread")]
async fn another_workspaces_namespace_file_is_not_found_and_never_its_bytes() {
    let _turn = SERIAL.lock().await;
    let Some((db, a, fs)) = setup().await else {
        return;
    };
    let (b, _) = common::seed_workspace(&db).await;
    put_file(&db, &fs, b, "w9b.ws", "/secret.txt", b"B-SECRET").await;
    put_file(&db, &fs, a, "w9b.ws", "/mine.txt", b"A-OWN").await;
    let tasks = [
        ret("read", "read('/secret.txt')"),
        ret("exists", "fileExists('/secret.txt')"),
        ret("uri", "fileURI('kestra:///w9b/ws/_files/secret.txt')"),
        ret("kread", "read('kestra:///w9b/ws/_files/secret.txt')"),
        ret("mine", "read('/mine.txt')"),
    ];
    let ex = run(&db, a, &flow("ws", "w9b.ws", &tasks), json!({})).await;
    assert_eq!(value_of(&db, ex, "exists").await, "false");
    assert_eq!(value_of(&db, ex, "mine").await, "A-OWN", "the control");
    for task in ["read", "uri", "kread"] {
        let message = error_of(&db, ex, task).await;
        assert!(
            message.contains("'/secret.txt'") && message.contains("does not exist"),
            "{task}: {message}"
        );
        assert!(
            !message.contains("B-SECRET") && !message.contains(&b.to_string()),
            "{task}: {message}"
        );
    }
}

/// The `_files` rule (`nsfiles::access::check_namespace`, through
/// `authorize`): an authority naming a `_files` namespace is refused by name,
/// even with a file of that path in the reader's own namespace (what reading
/// the wrong namespace would answer). And a flow whose own namespace has an
/// empty segment cannot read namespace files at all (M3).
#[tokio::test(flavor = "multi_thread")]
async fn a_files_segment_namespace_authority_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.own", "/x.txt", b"own x").await;
    put_file(&db, &fs, ws, "w9b.other", "/o.txt", b"other").await;
    let tasks = [
        ret("read", "read('nsfile://w9b._files/x.txt')"),
        ret("exists", "fileExists('nsfile://w9b._files/x.txt')"),
        ret("arg", "fileSize('x.txt', namespace='w9b._files')"),
        ret("control", "read('x.txt')"),
    ];
    let ex = run(&db, ws, &flow("fs", "w9b.own", &tasks), json!({})).await;
    assert_eq!(value_of(&db, ex, "control").await, "own x");
    for task in ["read", "exists", "arg"] {
        let message = error_of(&db, ex, task).await;
        assert!(
            message.contains("'w9b._files'") && message.contains("segment '_files'"),
            "{task}: {message}"
        );
    }

    let empty = [ret("other", "read('nsfile://w9b.other/o.txt')")];
    let ex = run(&db, ws, &flow("e", "w9b..e", &empty), json!({})).await;
    let message = error_of(&db, ex, "other").await;
    assert!(
        message.contains("running flow's namespace 'w9b..e'") && message.contains("empty segment"),
        "{message}"
    );
}

/// `workflow.storage.max_read_bytes` (10 MiB by default): one byte over is
/// an error naming the cap; at the cap is read (its length, so the output
/// stays small).
#[tokio::test(flavor = "multi_thread")]
async fn read_over_max_read_bytes_fails_naming_the_cap() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let cap = fs.caps.max_read_bytes;
    let at = vec![b'a'; usize::try_from(cap).unwrap()];
    let mut over = at.clone();
    over.push(b'a');
    put_file(&db, &fs, ws, "w9b.cap", "/at.txt", &at).await;
    put_file(&db, &fs, ws, "w9b.cap", "/over.txt", &over).await;
    let tasks = [
        ret("at", "read('/at.txt') | length"),
        ret("over", "read('/over.txt')"),
        ret("size", "fileSize('/over.txt')"),
    ];
    let ex = run(&db, ws, &flow("cap", "w9b.cap", &tasks), json!({})).await;
    assert_eq!(value_of(&db, ex, "at").await, cap.to_string());
    let message = error_of(&db, ex, "over").await;
    assert!(
        message.contains("'/over.txt'")
            && message.contains(&format!("workflow.storage.max_read_bytes ({cap} bytes)")),
        "{message}"
    );
    // fileSize is not capped: only read() holds the bytes.
    assert_eq!(value_of(&db, ex, "size").await, (cap + 1).to_string());
}

/// `read.binary` (chosen): non-UTF-8 is an error naming the path; the other
/// three answer, NotUtf8 being a property of the bytes.
#[tokio::test(flavor = "multi_thread")]
async fn read_of_a_binary_file_fails_naming_the_path() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(
        &db,
        &fs,
        ws,
        "w9b.bin",
        "/fu/bin.dat",
        &[0xff, 0xfe, 0x00, 0x81],
    )
    .await;
    let tasks = [
        ret("read", "read('/fu/bin.dat')"),
        ret("size", "fileSize('/fu/bin.dat')"),
    ];
    let ex = run(&db, ws, &flow("bin", "w9b.bin", &tasks), json!({})).await;
    let message = error_of(&db, ex, "read").await;
    assert!(
        message.contains("kestra:///w9b/bin/_files/fu/bin.dat") && message.contains("is not UTF-8"),
        "{message}"
    );
    assert_eq!(value_of(&db, ex, "size").await, "4");
}

/// A Flow trigger's `conditions`/`inputs` render against
/// `execution_render_context`, which carries no file scope: all five refuse
/// by name, namespace paths included.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_trigger_condition_calling_file_uri_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.trig", "/x.txt", b"x").await;
    let src = "id: fired\nnamespace: w9b.trig\ntasks:\n  - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n";
    let ex = run(&db, ws, src, json!({})).await;
    let mut conn = db.pool.acquire().await.unwrap();
    let cx = execution_render_context(&mut conn, ex).await.unwrap();
    assert_eq!(
        render_str("{{ execution.id }}", &cx).unwrap(),
        ex.to_string()
    );
    for call in ["fileURI", "read", "fileSize", "fileExists", "isFileEmpty"] {
        for arg in ["x.txt", "nsfile:///x.txt"] {
            let err = render_str(&format!("{{{{ {call}('{arg}') }}}}"), &cx)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&format!("The '{call}' function needs a running execution")),
                "{call}({arg}): {err}"
            );
        }
    }
}

/// W9a's C1, for namespace lookups: each lookup blocks its render thread on
/// the runtime while the worker that spawned it waits in `join`. One worker,
/// four concurrent renders, each opening its own connection; a std-thread
/// watchdog so a wedge FAILS instead of hanging the suite.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_namespace_reads_on_one_worker_do_not_wedge_the_runtime() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.wedge", "/cfg/x.txt", b"unwedged").await;
    let opts = Arc::new((*db.pool.connect_options()).clone());
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let out = rt.block_on(async {
            let mut renders = Vec::new();
            for _ in 0..4 {
                let (fs, opts) = (fs.clone(), opts.clone());
                renders.push(tokio::spawn(async move {
                    let snap = Snapshot::from_parts(Default::default(), fs, ws, None)
                        .with_namespace_files(Some("w9b.wedge".into()), Some(opts));
                    let cx = RenderContext::new().with_files(snap.into_scope());
                    render_str("{{ read('cfg/x.txt') }}", &cx).map_err(|e| e.to_string())
                }));
            }
            let mut all = Vec::new();
            for r in renders {
                all.push(r.await.unwrap());
            }
            all
        });
        let _ = done.send(out);
    });
    let out = tokio::task::spawn_blocking(move || {
        finished.recv_timeout(std::time::Duration::from_secs(30))
    })
    .await
    .unwrap()
    .expect("the runtime wedged: four namespace reads on one worker did not finish in 30s");
    assert_eq!(out, vec![Ok("unwedged".to_string()); 4]);
}

/// The brief's measurement: one connect per render scope that calls a
/// namespace function. Prints the median of 50 connects to `DATABASE_URL`.
/// Ignored: a measurement, not a check (`--ignored --nocapture` to run).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: cargo test --test nsfiles_render -- --ignored --nocapture"]
async fn the_cost_of_one_connect_per_render_scope() {
    use sqlx::Connection as _;
    let Some(db) = common::db().await else {
        return;
    };
    let opts = (*db.pool.connect_options()).clone();
    let mut took = Vec::new();
    for _ in 0..50 {
        let t = std::time::Instant::now();
        let conn = sqlx::PgConnection::connect_with(&opts).await.unwrap();
        took.push(t.elapsed());
        conn.close().await.unwrap();
    }
    took.sort();
    println!(
        "connect x50: median {:?}, min {:?}, max {:?}",
        took[25], took[0], took[49]
    );
}

// ─── Fix round 1 ─────────────────────────────────────────────────────────────

/// The coordinator's ruling on concern 1 (M2): beside a `kestra:///…/_files/…`
/// URI, a `revision` argument, or a `namespace` argument that differs from
/// the URI's, is an error naming both the argument and the URI; the same
/// namespace is not. And M3: `fileURI` of such a URI checks that it names a
/// file -- a missing one fails naming the path, a soft-deleted path's
/// revision 1 is returned (R1).
#[tokio::test(flavor = "multi_thread")]
async fn a_kestra_uri_beside_a_revision_or_another_namespace_is_an_error_naming_both() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_the_artifacts_namespaces(&db, &fs, ws).await;
    let three = "kestra:///w9b/fn/_files/fu/three.txt";
    let one = "kestra:///w9b/fn/_files/fu/one.txt";
    let tasks = [
        ret("rrev", &format!("read('{three}', revision=2)")),
        ret("urev", &format!("fileURI('{three}', revision=2)")),
        ret("rns", &format!("read('{one}', namespace='w9b.other')")),
        ret("sns", &format!("fileSize('{one}', namespace='w9b.other')")),
        ret("same", &format!("read('{one}', namespace='w9b.fn')")),
        ret("missing", "fileURI('kestra:///w9b/fn/_files/fu/nope.txt')"),
        ret("gone", "fileURI('kestra:///w9b/fn/_files/fu/gone.txt')"),
        ret("plain", &format!("read('{three}')")),
    ];
    let ex = run(&db, ws, &flow("m2", "w9b.fn", &tasks), json!({})).await;
    for task in ["rrev", "urev"] {
        let m = error_of(&db, ex, task).await;
        assert!(
            m.contains("'revision' argument (2)") && m.contains(three),
            "{task}: {m}"
        );
    }
    for task in ["rns", "sns"] {
        let m = error_of(&db, ex, task).await;
        assert!(
            m.contains("'namespace' argument 'w9b.other'") && m.contains(one),
            "{task}: {m}"
        );
    }
    assert_eq!(value_of(&db, ex, "same").await, "one");
    assert_eq!(
        value_of(&db, ex, "plain").await,
        "r1",
        "the control: revision 1"
    );
    let m = error_of(&db, ex, "missing").await;
    assert!(
        m.contains("'/fu/nope.txt'") && m.contains("does not exist"),
        "{m}"
    );
    assert_eq!(
        value_of(&db, ex, "gone").await,
        "kestra:///w9b/fn/_files/fu/gone.txt"
    );
}

/// M8: a directory is always revision 1, so a `kestra:///…/_files/d` URI
/// names it as its path does -- `fileExists` true, `fileSize` 0,
/// `isFileEmpty` true -- and `read` and `fileURI` of it fail naming it (R3).
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_named_by_a_kestra_uri_answers_as_by_its_path() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_the_artifacts_namespaces(&db, &fs, ws).await;
    let d = "'kestra:///w9b/fn/_files/fu'";
    let tasks = [
        ret("e", &format!("fileExists({d})")),
        ret("s", &format!("fileSize({d})")),
        ret("i", &format!("isFileEmpty({d})")),
        ret("r", &format!("read({d})")),
        ret("u", &format!("fileURI({d})")),
        ret("pe", "fileExists('/fu')"),
        ret("v2", "fileExists('kestra:///w9b/fn/_files/fu.v2')"),
    ];
    let ex = run(&db, ws, &flow("m8", "w9b.fn", &tasks), json!({})).await;
    for (task, want) in [("e", "true"), ("s", "0"), ("i", "true"), ("pe", "true")] {
        assert_eq!(value_of(&db, ex, task).await, want, "{task}");
    }
    // A `.vN` URI is a file's revision N; a directory has none.
    assert_eq!(value_of(&db, ex, "v2").await, "false");
    let m = error_of(&db, ex, "r").await;
    assert!(m.contains("'/fu' is a directory"), "{m}");
    let m = error_of(&db, ex, "u").await;
    assert!(
        m.contains("'/fu' is a directory") && m.contains("no file URI"),
        "{m}"
    );
}

/// Connect options for one test's namespace lookups, told apart from every
/// other connection in `pg_stat_activity` by their application name.
fn tagged(db: &Db, tag: &str) -> Arc<sqlx::postgres::PgConnectOptions> {
    Arc::new((*db.pool.connect_options()).clone().application_name(tag))
}

async fn backends(db: &Db, tag: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name = $1")
        .bind(tag)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// `backends(tag)` reaches `want` within 5 s (a closed connection's backend
/// exits a moment after the goodbye), or the last count.
async fn settles_at(db: &Db, tag: &str, want: i64) -> i64 {
    let mut n = backends(db, tag).await;
    for _ in 0..250 {
        if n == want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        n = backends(db, tag).await;
    }
    n
}

/// A snapshot over the installed store, reaching namespace files as `ns`'s
/// flow with `opts`, kept as itself so the test can ask what it holds.
fn snapshot(
    fs: &Arc<FileStore>,
    ws: Uuid,
    ns: &str,
    opts: Arc<sqlx::postgres::PgConnectOptions>,
) -> Arc<Snapshot> {
    Arc::new(
        Snapshot::from_parts(Default::default(), fs.clone(), ws, None)
            .with_namespace_files(Some(ns.into()), Some(opts)),
    )
}

/// I1: a render's namespace connection does not outlive the render. Two
/// lookups in one render share it (one backend while the render runs is
/// not observable from here, so the test checks the end): once `render_str`
/// returns, the scope holds nothing and the server has no backend of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_render_closes_its_namespace_connection_when_it_ends() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.life", "/a.txt", b"A").await;
    let tag = format!("w9b-t4-life-{}", Uuid::new_v4().simple());
    let snap = snapshot(&fs, ws, "w9b.life", tagged(&db, &tag));
    let cx = RenderContext::new().with_files(FileScope(snap.clone()));
    for _ in 0..2 {
        assert_eq!(
            render_str("{{ read('a.txt') }}{{ fileSize('a.txt') }}", &cx).unwrap(),
            "A1"
        );
        assert!(
            !snap.holds_a_namespace_connection(),
            "the render ended holding it"
        );
        assert_eq!(
            settles_at(&db, &tag, 0).await,
            0,
            "a backend outlived the render"
        );
    }
    assert_eq!(snap.infrastructure_failure(), None);
}

/// I1: a lookup that fails on a connection an earlier lookup of the same
/// render opened -- here the server ended it (`pg_terminate_backend`) --
/// reconnects once and answers; it is not an outage. Driven through the
/// trait from a plain thread (a render thread's situation), with no render
/// end between the two lookups.
#[tokio::test(flavor = "multi_thread")]
async fn a_lookup_on_a_connection_the_server_ended_reconnects_once() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.rc", "/a.txt", b"A").await;
    let tag = format!("w9b-t4-rc-{}", Uuid::new_v4().simple());
    let snap = snapshot(&fs, ws, "w9b.rc", tagged(&db, &tag));
    let args = NsFileArgs {
        path: "a.txt",
        namespace: None,
        revision: None,
    };
    let (s, handle, pool, t) = (
        snap.clone(),
        tokio::runtime::Handle::current(),
        db.pool.clone(),
        tag.clone(),
    );
    let out = std::thread::spawn(move || {
        let first = s.namespace_file(&args).map(|m| m.size);
        assert!(s.holds_a_namespace_connection(), "the lookup keeps it for the render");
        let killed: Vec<bool> = handle.block_on(
            sqlx::query_scalar(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1",
            )
            .bind(&t)
            .fetch_all(&pool),
        )
        .unwrap();
        let second = s.namespace_file(&args).map(|m| m.size);
        let third = s.read_namespace_file(&args);
        s.end_render();
        (first, killed, second, third)
    })
    .join()
    .unwrap();
    assert_eq!(out.0, Ok(1));
    assert_eq!(
        out.1,
        vec![true],
        "the server ended exactly the scope's backend"
    );
    assert_eq!(out.2, Ok(1), "reconnected once and answered");
    assert_eq!(out.3, Ok("A".to_string()));
    assert_eq!(snap.infrastructure_failure(), None, "not an outage");
    assert!(!snap.holds_a_namespace_connection());
    assert_eq!(settles_at(&db, &tag, 0).await, 0);
}

/// M6: a QUERY that fails (not the connect) is an outage too, and the
/// connection it failed on is not kept. The role connects but may not read
/// `workflow` (M4, deferred: every sqlx error is an outage today).
#[tokio::test(flavor = "multi_thread")]
async fn a_query_failure_is_an_outage_and_its_connection_is_not_kept() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    // The dedicated test database's own role, created once; it may connect
    // and may not use the `workflow` schema.
    sqlx::query(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'w9b_t4_noread') THEN \
             CREATE ROLE w9b_t4_noread LOGIN PASSWORD 'w9b_t4_noread'; \
           END IF; \
         END $$",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let opts = Arc::new(
        (*db.pool.connect_options())
            .clone()
            .username("w9b_t4_noread")
            .password("w9b_t4_noread"),
    );
    let snap = snapshot(&fs, ws, "w9b.q", opts);
    let cx = RenderContext::new().with_files(FileScope(snap.clone()));
    let err = render_str("{{ fileExists('a.txt') }}", &cx)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("'/a.txt'") && err.contains("database is unavailable"),
        "{err}"
    );
    let failure = snap
        .infrastructure_failure()
        .expect("an outage is recorded");
    assert!(
        failure.contains("permission denied"),
        "a query, not a connect: {failure}"
    );
    // The render's end closes whatever it held, so ask the scope directly,
    // from a plain thread and with no render end: the connection the query
    // failed on was let go by the lookup itself.
    let s = snap.clone();
    let kept = std::thread::spawn(move || {
        let args = NsFileArgs {
            path: "a.txt",
            namespace: None,
            revision: None,
        };
        let failed = s.namespace_file(&args).is_err();
        let kept = s.holds_a_namespace_connection();
        s.end_render();
        (failed, kept)
    })
    .join()
    .unwrap();
    assert_eq!(kept, (true, false), "the lookup failed and kept nothing");
}

/// M6: the stream guard. An object larger than its row says (the row says
/// 4 bytes, the object holds 5) is refused naming the cap, though the row's
/// own size passed the check before the download.
#[tokio::test(flavor = "multi_thread")]
async fn the_stream_guard_refuses_an_object_larger_than_its_row() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    put_file(&db, &fs, ws, "w9b.lies", "/l.txt", b"abcd").await;
    let grant = authorize(ws, Actor::Api, "w9b.lies", Access::Read).unwrap();
    let key = fs.namespace_object_uri(&grant, &NsPath::parse_file("/l.txt").unwrap(), 1);
    fs.store.put(&key, b"abcde".to_vec()).await.unwrap();
    let small = Arc::new(FileStore {
        store: fs.store.clone(),
        bucket: fs.bucket.clone(),
        prefix: fs.prefix.clone(),
        caps: storage::Caps {
            max_read_bytes: 4,
            ..storage::Caps::default()
        },
        retention_days: 30,
    });
    let snap = snapshot(
        &small,
        ws,
        "w9b.lies",
        Arc::new((*db.pool.connect_options()).clone()),
    );
    let cx = RenderContext::new().with_files(FileScope(snap.clone()));
    let err = render_str("{{ read('l.txt') }}", &cx)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("'/l.txt'") && err.contains("max_read_bytes (4 bytes)"),
        "{err}"
    );
    assert_eq!(render_str("{{ fileSize('l.txt') }}", &cx).unwrap(), "4");
}
