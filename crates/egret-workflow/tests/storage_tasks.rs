//! W9a Task 11: `core.storage.{Write, Size, Delete, PurgeCurrentExecutionFiles}`
//! and `Loop` over an ION file, and W9a Task 12: `core.storage.{Concat, Split,
//! Reverse, DeduplicateItems, FilterItems}`, through the real worker (spec
//! §9.3), against `oracle-exec/w9a-storage.json`. Requires DATABASE_URL.
//!
//! `multi_thread` and [`SERIAL`] for `tests/storage_render.rs`'s reasons: a
//! `Loop`'s values file is read on a render thread that blocks on the
//! runtime, and a claim is global. The store is [`Probe`] over a `MemStore`,
//! installed once for this binary with `max_read_bytes` = [`READ_CAP`]:
//! nothing here touches MinIO.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::{self, uri, FileStore, StoredFile};
use egret_workflow::worker::{
    apply_outcome, claim, reap, start_execution, start_execution_in, tick, Handlers, NewExecution,
    Outcome,
};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NS: &str = "w9a.t11";

/// This binary's `workflow.storage.max_read_bytes`: every file a test loops
/// over is under it, and the over-cap test's is over it.
const READ_CAP: u64 = 4096;

/// While set, `get_stream` fails as an unreachable S3 does (spec §11).
static OUTAGE: AtomicBool = AtomicBool::new(false);
/// While set, `delete` fails as an unreachable S3 does.
static DELETE_FAILS: AtomicBool = AtomicBool::new(false);
/// The pool [`Probe::delete`] asks, and what it saw: for each object deleted,
/// the URI and how many index rows named it AT THAT MOMENT.
static PROBE_POOL: Mutex<Option<sqlx::PgPool>> = Mutex::new(None);
static ROWS_AT_DELETE: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new());

/// `MemStore`, with the two faults and the order probe in front of it.
struct Probe(egret_storage::MemStore);

/// `s3://egret-test/workflow/{ws}/{path}` back to `kestra:///{path}`.
fn uri_of_object(object: &str) -> String {
    let rest = object
        .strip_prefix("s3://egret-test/workflow/")
        .expect("this binary's prefix");
    let (_ws, path) = rest.split_once('/').expect("a workspace segment");
    format!("kestra:///{path}")
}

#[async_trait::async_trait]
impl ObjectStore for Probe {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, object: &str) -> Result<(), StorageError> {
        let pool = PROBE_POOL.lock().unwrap().clone();
        if let Some(pool) = pool {
            let file = uri_of_object(object);
            let rows: i64 =
                sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
                    .bind(&file)
                    .fetch_one(&pool)
                    .await
                    .expect("probe");
            ROWS_AT_DELETE.lock().unwrap().push((file, rows));
        }
        if DELETE_FAILS.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!(
                "{object}: dispatch failure: connection refused"
            )));
        }
        self.0.delete(object).await
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
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = storage::install(Arc::new(FileStore {
            store: Arc::new(Probe(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps {
                max_read_bytes: READ_CAP,
                ..storage::Caps::default()
            },
            retention_days: 30,
        }));
    });
    storage::current().expect("installed above")
}

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    let (ws, _) = common::seed_workspace(&db).await;
    OUTAGE.store(false, Ordering::SeqCst);
    DELETE_FAILS.store(false, Ordering::SeqCst);
    *PROBE_POOL.lock().unwrap() = Some(db.pool.clone());
    ROWS_AT_DELETE.lock().unwrap().clear();
    let fs = installed();
    Some((db, ws, fs))
}

/// The shipping registry for `container_runner = none`: `with_core_tasks`
/// plus `storage::register`, as `runtime::handlers_for` builds it.
fn registry(pool: &sqlx::PgPool) -> Handlers {
    let mut r = Handlers::with_core_tasks(egret_workflow::storage::current());
    egret_workflow::worker::handlers::storage::register(&mut r, pool.clone(), storage::current());
    r
}

async fn drain(pool: &sqlx::PgPool) {
    let reg = registry(pool);
    for _ in 0..256 {
        if tick(pool, 16, &reg).await.expect("tick") == 0 {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    })
}

fn core_storage() -> &'static Value {
    &fixture()["egret_expected"]["core_storage"]
}

/// `(state, outputs, message)` of `task`'s run in `execution`.
async fn result_in(
    pool: &sqlx::PgPool,
    execution: Uuid,
    task: &str,
) -> (String, Option<Value>, Option<String>) {
    sqlx::query_as(
        "SELECT state, outputs, message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("run {task} of {execution}: {e}"))
}

async fn rows_named(pool: &sqlx::PgPool, file: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
        .bind(file)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn object_of(fs: &FileStore, ws: Uuid, file: &str) -> Option<Vec<u8>> {
    match fs.store.get(&fs.object_uri(ws, file).unwrap()).await {
        Ok(bytes) => Some(bytes),
        Err(StorageError::NotFound(_)) => None,
        Err(e) => panic!("{file}: {e}"),
    }
}

/// A flow whose tasks are the fixture's own task objects, verbatim. JSON is
/// YAML, so the flow source is a JSON document.
fn flow_of(id: &str, tasks: Vec<Value>) -> String {
    serde_json::to_string_pretty(&json!({ "id": id, "namespace": NS, "tasks": tasks })).unwrap()
}

fn run_task(class: &str, task_id: &str) -> Value {
    core_storage()[class]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["task"]["id"] == task_id)
        .unwrap_or_else(|| panic!("{class} has no run {task_id}"))
        .clone()
}

/// Replace every Kestra URI in `v` by egret's, as `map` pairs them.
fn renamed(v: &Value, map: &[(String, String)]) -> Value {
    let mut text = v.to_string();
    for (kestra, egret) in map {
        text = text.replace(kestra, egret);
    }
    serde_json::from_str(&text).unwrap()
}

const WRITE: &str = "io.kestra.plugin.core.storage.Write";
const SIZE: &str = "io.kestra.plugin.core.storage.Size";
const DELETE: &str = "io.kestra.plugin.core.storage.Delete";
const PURGE: &str = "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles";
const CONCAT: &str = "io.kestra.plugin.core.storage.Concat";
const SPLIT: &str = "io.kestra.plugin.core.storage.Split";
const REVERSE: &str = "io.kestra.plugin.core.storage.Reverse";
const DEDUP: &str = "io.kestra.plugin.core.storage.DeduplicateItems";
const FILTER: &str = "io.kestra.plugin.core.storage.FilterItems";

/// The handlers' declared properties and outputs are the fixture's, per class
/// (name, `required`, `default`; output name and JSON type).
#[test]
fn each_class_declares_kestras_properties_and_outputs() {
    use egret_workflow::worker::handlers::storage::DECLARED;
    for d in DECLARED {
        let class = &core_storage()[d.type_id];
        let props = class["properties"].as_object().unwrap();
        let ours: Vec<&str> = d.properties.iter().map(|p| p.name).collect();
        let theirs: Vec<&str> = props.keys().map(String::as_str).collect();
        assert_eq!(ours, theirs, "{}: property names", d.type_id);
        for p in d.properties {
            let k = &props[p.name];
            assert_eq!(k["required"], json!(p.required), "{}.{}", d.type_id, p.name);
            assert_eq!(
                k.get("default").and_then(Value::as_str),
                p.default,
                "{}.{} default",
                d.type_id,
                p.name
            );
        }
        let outs = class["outputs"].as_object().unwrap();
        let ours: Vec<(&str, &str)> = d.outputs.to_vec();
        let theirs: Vec<(&str, &str)> = outs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str().unwrap()))
            .collect();
        assert_eq!(ours, theirs, "{}: outputs", d.type_id);
    }
    assert_eq!(DECLARED.len(), 9);
}

/// `Write`, `Size` and `Delete`, each task taken verbatim from the fixture's
/// runs and in one flow: every state and output is Kestra's (URIs renamed to
/// egret's, which differ only in the ids and the file name), and the bytes
/// written are Kestra's. `Delete`'s object goes AFTER its index row.
#[tokio::test(flavor = "multi_thread")]
async fn write_size_and_delete_reproduce_kestras_runs() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let order = [
        (WRITE, "w_lines"),
        (WRITE, "w_other"),
        (WRITE, "w_ion"),
        (SIZE, "size"),
        (DELETE, "delete"),
        (DELETE, "delete_again"),
        (DELETE, "delete_missing_err"),
    ];
    let tasks = order
        .iter()
        .map(|(c, t)| run_task(c, t)["task"].clone())
        .collect();
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &flow_of("core-storage", tasks),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;

    // egret's URI for each Write, and the fixture's.
    let mut map = Vec::new();
    for t in ["w_lines", "w_other", "w_ion"] {
        let (state, outputs, message) = result_in(&db.pool, ex, t).await;
        assert_eq!(state, "SUCCESS", "{t}: {message:?}");
        let ours = outputs.unwrap()["uri"].as_str().unwrap().to_string();
        let theirs = run_task(WRITE, t)["outputs"]["uri"]
            .as_str()
            .unwrap()
            .to_string();
        let ext = run_task(WRITE, t)["task"]["extension"]
            .as_str()
            .unwrap_or(".tmp")
            .to_string();
        assert!(theirs.ends_with(&ext), "{theirs}");
        let parsed = uri::parse(&ours).unwrap();
        assert_eq!(parsed.execution, ex);
        let (run_id, _) = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(ex)
        .bind(t)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            ours,
            format!(
                "{}content{ext}",
                uri::task_prefix(NS, "core-storage", ex, t, run_id).unwrap()
            ),
            "{t}: the run's prefix, then the fixed stem and Kestra's extension"
        );
        map.push((theirs, ours));
    }
    for (t, run) in [("w_lines", &map[0]), ("w_ion", &map[2])] {
        let want = base64::engine::general_purpose::STANDARD
            .decode(
                run_task(WRITE, t)["files_written"][&run.0]["bytes_b64"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(object_of(&fs, ws, &run.1).await, Some(want.clone()), "{t}");
        let size: i64 =
            sqlx::query_scalar("SELECT size FROM workflow.execution_files WHERE uri = $1")
                .bind(&run.1)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(size as usize, want.len(), "{t}: the index holds the size");
    }

    for (class, t) in &order[3..] {
        let want = run_task(class, t);
        let (state, outputs, message) = result_in(&db.pool, ex, t).await;
        assert_eq!(state, want["state"].as_str().unwrap(), "{t}: {message:?}");
        if want["state"] == "SUCCESS" {
            assert_eq!(outputs, Some(renamed(&want["outputs"], &map)), "{t}");
        } else {
            assert_eq!(
                message.as_deref(),
                Some(renamed(&want["error"], &map).as_str().unwrap()),
                "{t}: Kestra's message"
            );
        }
    }

    // `w_other` is gone: no row, no object -- and the row went FIRST.
    let other = &map[1].1;
    assert_eq!(rows_named(&db.pool, other).await, 0);
    assert_eq!(object_of(&fs, ws, other).await, None);
    assert_eq!(
        *ROWS_AT_DELETE.lock().unwrap(),
        vec![(other.clone(), 0)],
        "the store deleted the object once, after the index row was gone"
    );
}

/// Kestra's bytes of `file` in `run`'s `files_written`.
fn kestra_bytes(run: &Value, file: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(run["files_written"][file]["bytes_b64"].as_str().unwrap())
        .unwrap()
}

/// Every Ion value of `bytes`, text or binary, as JSON: the comparison for
/// a file Kestra writes as BINARY Ion and egret as text.
fn ion_values(bytes: &[u8]) -> Vec<Value> {
    egret_workflow::storage::ion::values_to_json(std::io::Cursor::new(bytes.to_vec()), 100_000)
        .unwrap()
        .0
}

/// **W9a Task 12.** `Concat`, `Split`, `Reverse`, `DeduplicateItems` and
/// `FilterItems`, each run taken verbatim from the fixture and in one flow
/// after its three `Write`s: every state and output is Kestra's (URIs
/// renamed), the text files are Kestra's bytes, and the two item files --
/// BINARY Ion in Kestra, text in egret -- hold Kestra's values. Each file is
/// indexed with its size, and the URI is the run's prefix and egret's fixed
/// name.
#[tokio::test(flavor = "multi_thread")]
async fn the_whole_file_tasks_reproduce_kestras_runs() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let order = [
        (WRITE, "w_lines"),
        (WRITE, "w_other"),
        (WRITE, "w_ion"),
        (CONCAT, "concat"),
        (CONCAT, "concat_sep"),
        (SPLIT, "split_rows"),
        (SPLIT, "split_parts"),
        (SPLIT, "split_bytes"),
        (REVERSE, "reverse"),
        (DEDUP, "dedup"),
        (FILTER, "filter"),
        (FILTER, "filter_exclude"),
    ];
    let tasks = order
        .iter()
        .map(|(c, t)| run_task(c, t)["task"].clone())
        .collect();
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &flow_of("core-storage", tasks),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;

    let mut map = Vec::new();
    for t in ["w_lines", "w_other", "w_ion"] {
        let theirs = run_task(WRITE, t)["outputs"]["uri"]
            .as_str()
            .unwrap()
            .to_string();
        map.push((theirs, uri_of(&db.pool, ex, t).await));
    }
    let run_id = |t: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
            )
            .bind(ex)
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    for (class, t) in &order[3..] {
        let want = run_task(class, t);
        let (state, outputs, message) = result_in(&db.pool, ex, t).await;
        assert_eq!(state, want["state"].as_str().unwrap(), "{t}: {message:?}");
        let outputs = outputs.unwrap();
        let prefix = uri::task_prefix(NS, "core-storage", ex, t, run_id(t).await).unwrap();
        // Kestra's files, in order, and egret's.
        let (theirs, ours): (Vec<String>, Vec<String>) = if *class == SPLIT {
            let list = |v: &Value| -> Vec<String> {
                v["uris"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|u| u.as_str().unwrap().to_string())
                    .collect()
            };
            (list(&want["outputs"]), list(&outputs))
        } else {
            (
                vec![want["outputs"]["uri"].as_str().unwrap().to_string()],
                vec![outputs["uri"].as_str().unwrap().to_string()],
            )
        };
        assert_eq!(theirs.len(), ours.len(), "{t}: {ours:?}");
        let mut renames = map.clone();
        for (i, (k, e)) in theirs.iter().zip(&ours).enumerate() {
            let ext = &k[k.rfind('.').unwrap()..];
            let name = if *class == SPLIT {
                format!("part-{i:05}{ext}")
            } else {
                format!("content{ext}")
            };
            assert_eq!(
                e,
                &format!("{prefix}{name}"),
                "{t}: prefix, fixed name, Kestra's extension"
            );
            let bytes = object_of(&fs, ws, e)
                .await
                .unwrap_or_else(|| panic!("{t}: {e}"));
            let kestra = kestra_bytes(&want, k);
            if [DEDUP, FILTER].contains(class) {
                assert_eq!(
                    &kestra[..4],
                    [0xE0, 0x01, 0x00, 0xEA],
                    "{t}: Kestra's is binary"
                );
                assert_eq!(
                    ion_values(&bytes),
                    ion_values(&kestra),
                    "{t}: Kestra's values"
                );
            } else {
                assert_eq!(bytes, kestra, "{t}: Kestra's bytes");
            }
            assert_eq!(rows_named(&db.pool, e).await, 1, "{t}: indexed");
            let size: i64 =
                sqlx::query_scalar("SELECT size FROM workflow.execution_files WHERE uri = $1")
                    .bind(e)
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
            assert_eq!(size as usize, bytes.len(), "{t}: the index holds the size");
            renames.push((k.clone(), e.clone()));
        }
        assert_eq!(
            outputs,
            renamed(&want["outputs"], &renames),
            "{t}: Kestra's outputs"
        );
    }
}

/// A `filterCondition` reads through the RUN's own snapshot: `fileSize` of a
/// file the run may read answers inside the per-item render, with the
/// item's fields beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_filter_condition_reads_the_runs_files_beside_the_items_fields() {
    let _turn = SERIAL.lock().await;
    let Some((db, _ws, _fs)) = setup().await else {
        return;
    };
    let w_ion = run_task(WRITE, "w_ion")["task"].clone();
    let w_lines = run_task(WRITE, "w_lines")["task"].clone();
    let filter = json!({
        "id": "f",
        "type": FILTER,
        "from": "{{ outputs.w_ion.uri }}",
        "filterCondition": "{{ fileSize(outputs.w_lines.uri) == 15 and v > 2 }}",
    });
    let ex = start_execution(
        &db.pool,
        _ws,
        None,
        &flow_of("filter-reads", vec![w_lines, w_ion, filter]),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, outputs, message) = result_in(&db.pool, ex, "f").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let outputs = outputs.unwrap();
    assert_eq!(
        (
            &outputs["processedItemsTotal"],
            &outputs["droppedItemsTotal"]
        ),
        (&json!(4), &json!(2))
    );
}

/// The index row first: a store that cannot delete the object leaves an
/// ORPHAN (an object no row names), never a row naming an object that is
/// gone, and the file is deleted for the flow.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_cannot_delete_leaves_an_orphan_never_a_dangling_row() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: orphan\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: {WRITE}\n    content: x\n\
         \x20 - id: d\n    type: {DELETE}\n    uri: \"{{{{ outputs.w.uri }}}}\"\n\
         \x20 - id: e\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ fileExists(outputs.w.uri) }}}}\"\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    DELETE_FAILS.store(true, Ordering::SeqCst);
    drain(&db.pool).await;
    DELETE_FAILS.store(false, Ordering::SeqCst);
    let (_, w, _) = result_in(&db.pool, ex, "w").await;
    let file = w.unwrap()["uri"].as_str().unwrap().to_string();
    let (state, outputs, message) = result_in(&db.pool, ex, "d").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"uri": file, "deleted": true}))),
        "{message:?}"
    );
    assert_eq!(rows_named(&db.pool, &file).await, 0, "no row");
    assert_eq!(
        object_of(&fs, ws, &file).await,
        Some(b"x".to_vec()),
        "the object is left, an orphan for the pruner"
    );
    assert_eq!(
        result_in(&db.pool, ex, "e").await.1,
        Some(json!({"value": "false"})),
        "the file no longer exists for the flow"
    );
}

/// A file of the trigger execution is READABLE (`Size` reads it: the
/// control) and is NOT deletable (spec §9.3; `core_storage.other_execution`,
/// chosen): `Delete` fails naming it, and the row and the object stay.
#[tokio::test(flavor = "multi_thread")]
async fn delete_of_its_trigger_executions_file_is_refused_and_the_file_stays() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source_flow = format!(
        "id: src\nnamespace: {NS}\ntasks:\n  - id: w\n    type: {WRITE}\n    content: kept\n"
    );
    let s = start_execution(&db.pool, ws, None, &source_flow, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let file = result_in(&db.pool, s, "w").await.1.unwrap()["uri"]
        .as_str()
        .unwrap()
        .to_string();

    let listener = format!(
        "id: listener\nnamespace: {NS}\ntasks:\n\
         \x20 - id: size\n    type: {SIZE}\n    uri: \"{file}\"\n\
         \x20 - id: del\n    type: {DELETE}\n    uri: \"{file}\"\n"
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

    assert_eq!(
        result_in(&db.pool, t, "size").await.1,
        Some(json!({"size": 4})),
        "the trigger's file is readable"
    );
    let (state, outputs, message) = result_in(&db.pool, t, "del").await;
    assert_eq!((state.as_str(), outputs), ("FAILED", None));
    let message = message.unwrap();
    assert!(
        message.contains(&file) && message.contains("loop family"),
        "{message}"
    );
    assert_eq!(rows_named(&db.pool, &file).await, 1, "the row stays");
    assert_eq!(object_of(&fs, ws, &file).await, Some(b"kept".to_vec()));
    assert!(ROWS_AT_DELETE.lock().unwrap().is_empty(), "nothing deleted");
}

/// An iteration deletes a file its loop ROOT wrote: the loop family is the
/// root and every iteration under it, not the running execution alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_iteration_deletes_a_file_its_loop_root_wrote() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: family\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: {WRITE}\n    content: root\n\
         \x20 - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"x\"]'\n    tasks:\n\
         \x20     - id: d\n        type: {DELETE}\n        uri: \"{{{{ outputs.w.uri }}}}\"\n"
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let file = result_in(&db.pool, root, "w").await.1.unwrap()["uri"]
        .as_str()
        .unwrap()
        .to_string();
    let iteration: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE parent_execution_id = $1")
            .bind(root)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let (state, outputs, message) = result_in(&db.pool, iteration, "d").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"uri": file, "deleted": true}))),
        "{message:?}"
    );
    assert_eq!(rows_named(&db.pool, &file).await, 0);
    assert_eq!(object_of(&fs, ws, &file).await, None);
}

/// Every URI under `kestra:///{ns}/{flow}/executions/{exec}`, with the ids
/// and the file name made placeholders: `{exec}/tasks/{task}/{run}/{file}`.
fn shape(uris: &Value) -> Vec<String> {
    let id = regex::Regex::new(r"^kestra:///.*?/executions/[^/]+").unwrap();
    uris.as_array()
        .unwrap()
        .iter()
        .map(|u| {
            let u = id.replace(u.as_str().unwrap(), "{exec}").to_string();
            let mut parts: Vec<String> = u.split('/').map(str::to_string).collect();
            if parts.len() >= 4 {
                parts[3] = "{run}".into();
            }
            if parts.len() == 5 {
                parts[4] = "{file}".into();
            }
            parts.join("/")
        })
        .collect()
}

/// `PurgeCurrentExecutionFiles` lists what it deleted as Kestra does: each
/// file, then every directory above it, post-order, down to the execution
/// (the fixture's two runs; its `sh` task is a shell task this binary cannot
/// run, and its entries are left out of the comparison).
#[tokio::test(flavor = "multi_thread")]
async fn purge_lists_what_it_deleted_as_kestra_does() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let source = format!(
        "id: core-purge\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w_a\n    type: {WRITE}\n    content: a\n    extension: .txt\n\
         \x20 - id: purge\n    type: {PURGE}\n\
         \x20 - id: w_after\n    type: {WRITE}\n    content: b\n\
         \x20 - id: purge_again\n    type: {PURGE}\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    for t in ["purge", "purge_again"] {
        let (state, outputs, message) = result_in(&db.pool, ex, t).await;
        assert_eq!(state, "SUCCESS", "{t}: {message:?}");
        let want: Vec<String> = shape(&run_task(PURGE, t)["outputs"]["uris"])
            .into_iter()
            .filter(|u| !u.contains("/tasks/sh"))
            .collect();
        assert_eq!(shape(&outputs.unwrap()["uris"]), want, "{t}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.execution_files WHERE execution_id = $1"
        )
        .bind(ex)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
}

/// The root's FILE input, indexed at start as the start route indexes an
/// upload: its URI, with the object put first.
async fn root_input(fs: &FileStore, ws: Uuid, flow: &str, root: Uuid) -> StoredFile {
    let file = uri::input_uri(NS, flow, root, "f", "in.txt").unwrap();
    fs.store
        .put(&fs.object_uri(ws, &file).unwrap(), b"input".to_vec())
        .await
        .unwrap();
    StoredFile { uri: file, size: 5 }
}

/// Start `source` with the id `id`, the FILE input `input` and, optionally,
/// a trigger execution.
async fn start_with(
    db: &Db,
    ws: Uuid,
    source: &str,
    id: Uuid,
    input: Option<StoredFile>,
    trigger: Option<Uuid>,
) -> Uuid {
    let mut conn = db.pool.acquire().await.unwrap();
    start_execution_in(
        &mut conn,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source,
            inputs: json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: trigger,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: Some(id),
            input_files: input.into_iter().collect(),
        },
    )
    .await
    .unwrap()
}

/// The `uri` output of `task` in `execution`.
async fn uri_of(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> String {
    result_in(pool, execution, task).await.1.unwrap()["uri"]
        .as_str()
        .unwrap()
        .to_string()
}

/// **`PurgeCurrentExecutionFiles` from the ROOT deletes only the root's own
/// files** -- its FILE input and what its tasks wrote -- and not one file of
/// its iterations or of its trigger execution (the controller's ruling of
/// 2026-10-03: "current execution" is the running one; spec §9.3). Every
/// object after its row.
#[tokio::test(flavor = "multi_thread")]
async fn purge_from_the_root_deletes_only_the_roots_own_files() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let source_flow = format!(
        "id: src\nnamespace: {NS}\ntasks:\n  - id: w\n    type: {WRITE}\n    content: kept\n"
    );
    let s = start_execution(&db.pool, ws, None, &source_flow, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    let kept = uri_of(&db.pool, s, "w").await;

    let main = format!(
        "id: purging\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w_root\n    type: {WRITE}\n    content: root\n\
         \x20 - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\", \"b\"]'\n    tasks:\n\
         \x20     - id: w_it\n        type: {WRITE}\n        content: \"{{{{ item.value }}}}\"\n\
         \x20 - id: purge\n    type: {PURGE}\n"
    );
    let id = Uuid::new_v4();
    let input = root_input(&fs, ws, "purging", id).await;
    let root = start_with(&db, ws, &main, id, Some(input.clone()), Some(s)).await;
    drain(&db.pool).await;

    let iterations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 ORDER BY loop_index",
    )
    .bind(root)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(iterations.len(), 2);
    let own = [input.uri.clone(), uri_of(&db.pool, root, "w_root").await];
    let mut theirs = vec![kept.clone()];
    for it in &iterations {
        let f = uri_of(&db.pool, *it, "w_it").await;
        assert_eq!(uri::parse(&f).unwrap().execution, *it, "its own id");
        theirs.push(f);
    }

    let (state, outputs, message) = result_in(&db.pool, root, "purge").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let listed: Vec<String> = outputs.unwrap()["uris"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect();
    for f in &own {
        assert!(listed.contains(f), "{f} listed");
        assert_eq!(rows_named(&db.pool, f).await, 0, "{f}: row");
        assert_eq!(object_of(&fs, ws, f).await, None, "{f}: object");
    }
    for f in &theirs {
        assert!(!listed.contains(f), "{f} not listed");
        assert_eq!(rows_named(&db.pool, f).await, 1, "{f}: its row stays");
        assert!(
            object_of(&fs, ws, f).await.is_some(),
            "{f}: its object stays"
        );
    }
    assert!(
        listed
            .iter()
            .all(|u| uri::parse(u).map_or(true, |p| p.execution == root)),
        "only the root's own prefix is listed: {listed:?}"
    );
    let mut seen = ROWS_AT_DELETE.lock().unwrap().clone();
    seen.sort();
    let mut want: Vec<(String, i64)> = own.iter().map(|f| (f.clone(), 0)).collect();
    want.sort();
    assert_eq!(seen, want, "each object deleted once, after its row");
}

/// **`PurgeCurrentExecutionFiles` inside an iteration deletes only that
/// ITERATION's files** (the controller's ruling of 2026-10-03; a file written
/// inside an iteration carries the iteration's id, spec §3): the root's FILE
/// input and its task's file stay, and so does a sibling's file. Each
/// iteration writes, purges, then writes again, so iteration 1's purge runs
/// while iteration 0's second file exists.
#[tokio::test(flavor = "multi_thread")]
async fn purge_inside_an_iteration_leaves_the_root_and_its_siblings_files() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let main = format!(
        "id: purge-it\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w_root\n    type: {WRITE}\n    content: root\n\
         \x20 - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\", \"b\"]'\n    tasks:\n\
         \x20     - id: w_before\n        type: {WRITE}\n        content: \"{{{{ item.value }}}}\"\n\
         \x20     - id: purge\n        type: {PURGE}\n\
         \x20     - id: w_after\n        type: {WRITE}\n        content: after\n"
    );
    let id = Uuid::new_v4();
    let input = root_input(&fs, ws, "purge-it", id).await;
    let root = start_with(&db, ws, &main, id, Some(input.clone()), None).await;
    drain(&db.pool).await;

    let iterations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 ORDER BY loop_index",
    )
    .bind(root)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(iterations.len(), 2);
    let root_file = uri_of(&db.pool, root, "w_root").await;
    let mut kept = vec![input.uri.clone(), root_file];
    let mut purged = Vec::new();
    for it in &iterations {
        purged.push(uri_of(&db.pool, *it, "w_before").await);
        kept.push(uri_of(&db.pool, *it, "w_after").await);
        let (state, outputs, message) = result_in(&db.pool, *it, "purge").await;
        assert_eq!(state, "SUCCESS", "{message:?}");
        let listed = outputs.unwrap()["uris"].clone();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|u| uri::parse(u.as_str().unwrap()).map_or(true, |p| p.execution == *it)),
            "iteration {it} lists only its own: {listed}"
        );
    }
    // Iteration 1's purge ran after iteration 0's `w_after` existed.
    for f in &kept {
        assert_eq!(rows_named(&db.pool, f).await, 1, "{f}: its row stays");
        assert!(
            object_of(&fs, ws, f).await.is_some(),
            "{f}: its object stays"
        );
    }
    for f in &purged {
        assert_eq!(rows_named(&db.pool, f).await, 0, "{f}: row");
        assert_eq!(object_of(&fs, ws, f).await, None, "{f}: object");
    }
}

/// **`Delete`'s statement is scoped to the loop family by itself**, not only
/// by the check before it: an index row whose URI names this execution but
/// whose `execution_id` is ANOTHER execution's (a row no writer makes, put
/// here directly) passes the URI check and is still not deleted -- the
/// statement's `execution_id = ANY(family)` is what keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn deletes_statement_cannot_reach_a_row_of_another_execution() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let other = start_execution(
        &db.pool,
        ws,
        None,
        &format!("id: other\nnamespace: {NS}\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n"),
        json!({}),
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let file = format!(
        "{}odd.txt",
        uri::task_prefix(NS, "scoped", id, "t", Uuid::new_v4()).unwrap()
    );
    let source = format!(
        "id: scoped\nnamespace: {NS}\ntasks:\n\
         \x20 - id: del\n    type: {DELETE}\n    uri: \"{file}\"\n"
    );
    let root = start_with(&db, ws, &source, id, None, None).await;
    sqlx::query(
        "INSERT INTO workflow.execution_files (workspace_id, execution_id, uri, size) \
         VALUES ($1, $2, $3, 3)",
    )
    .bind(ws)
    .bind(other)
    .bind(&file)
    .execute(&db.pool)
    .await
    .unwrap();
    fs.store
        .put(&fs.object_uri(ws, &file).unwrap(), b"odd".to_vec())
        .await
        .unwrap();
    drain(&db.pool).await;

    let (state, outputs, message) = result_in(&db.pool, root, "del").await;
    assert_eq!(
        (state.as_str(), outputs),
        ("SUCCESS", Some(json!({"uri": file, "deleted": false}))),
        "{message:?}"
    );
    assert_eq!(
        rows_named(&db.pool, &file).await,
        1,
        "the other's row stays"
    );
    assert_eq!(object_of(&fs, ws, &file).await, Some(b"odd".to_vec()));
}

/// **A `Loop` over a file keeps the file's values out of its task run**
/// (fix round 1, I1): the run's `rendered` -- what every load of the
/// execution reads and the task-run API returns -- is the URI and the
/// count, `"{uri}\n{count}"`, and the values themselves are read by index
/// from `workflow.loop_file_values` when an iteration is spawned.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_over_a_file_keeps_its_values_out_of_the_task_run() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("compact", "\"needle-0\"\n\"needle-1\"\n\"needle-0\""),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let file = uri_of(&db.pool, root, "write").await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let rendered: Option<String> = sqlx::query_scalar(
        "SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'loop'",
    )
    .bind(root)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rendered, Some(format!("{file}\n3")));
    let values: Vec<String> = iterations_of(&db.pool, root)
        .await
        .into_iter()
        .map(|(_, _, v)| v)
        .collect();
    assert_eq!(values, ["needle-0", "needle-1", "needle-0"]);
}

fn loop_over_ion() -> &'static Value {
    &fixture()["egret_expected"]["loop_over_ion"]
}

/// `(loop_index, loop_value)` of each iteration of `root`.
async fn iterations_of(pool: &sqlx::PgPool, root: Uuid) -> Vec<(Uuid, i32, String)> {
    sqlx::query_as(
        "SELECT id, loop_index, loop_value FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL ORDER BY loop_index",
    )
    .bind(root)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn corpus(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/oracle-exec/corpus/valids/{name}.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `valids/loop-values-from-uri`, the corpus flow verbatim: `Loop` with
/// `values: "{{ outputs.write.uri }}"` iterates the records of the ION file
/// `Write` wrote, each coalesced as Kestra's `ionValueToString`, and each
/// iteration's `item` is Kestra's (`loop_over_ion`, measured).
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_iterates_the_records_of_the_file_write_wrote() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let want = &loop_over_ion()["valids/loop-values-from-uri"];
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &corpus("loop-values-from-uri"),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, want["state"].as_str().unwrap(), "{message:?}");
    let its = iterations_of(&db.pool, root).await;
    assert_eq!(its.len() as u64, want["iterations"].as_u64().unwrap());
    let values: Vec<Value> = its.iter().map(|(_, _, v)| json!(v)).collect();
    assert_eq!(Value::Array(values), want["loopRun_values"]);
    let mut items = Vec::new();
    for (id, _, _) in &its {
        let (state, outputs, message) = result_in(&db.pool, *id, "item").await;
        assert_eq!(state, "SUCCESS", "{message:?}");
        items.push(outputs.unwrap()["value"].clone());
    }
    assert_eq!(Value::Array(items), want["item_outputs"]);
}

/// A loop over the file `content` holds, written by a `Write`, whose body
/// returns `{{ item.value }}`; the source of the loop's root.
fn loop_over_content(flow: &str, content: &str) -> String {
    format!(
        "id: {flow}\nnamespace: {NS}\ntasks:\n\
         \x20 - id: write\n    type: {WRITE}\n    extension: .ion\n    content: {}\n\
         \x20 - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
         values: \"{{{{ outputs.write.uri }}}}\"\n    concurrencyLimit: 0\n    tasks:\n\
         \x20     - id: item\n        type: io.kestra.plugin.core.debug.Return\n        \
         format: \"{{{{ item.value }}}}\"\n",
        serde_json::to_string(content).unwrap()
    )
}

/// `loop_over_ion.edge_file`, measured: a number, a nested struct, a list
/// holding a null, a string, a decimal (`12.50` keeps its scale), a
/// timestamp (JSON-quoted, as an instant with microseconds) and a boolean are
/// Kestra's values, all seven (the user's ruling of 2026-10-03 on W9a Task
/// 11's concern 2; until then the decimal and the timestamp were pinned as
/// divergences).
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_over_the_edge_file_reads_kestras_values() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let edge = &loop_over_ion()["edge_file"];
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("edge", edge["content"].as_str().unwrap()),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let ours: Vec<String> = iterations_of(&db.pool, root)
        .await
        .into_iter()
        .map(|(_, _, v)| v)
        .collect();
    let kestra: Vec<String> = edge["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kestra[4], "12.50");
    assert_eq!(kestra[5], "\"2025-12-03T12:00:00.000000Z\"");
    assert_eq!(ours, kestra);
}

/// **A record the file holds twice iterates twice** (the user's ruling of
/// 2026-10-03 on W9a Task 11's concern 2): Kestra's URI path has no
/// `distinct()` (`FlowableUtils.readLoopValuesFromUri`, `javap -c` of the
/// artifact's jar), so every record is an iteration, in file order, with its
/// own index. An inline `values:` array still deduplicates
/// (`executor::tests::a_files_values_are_iterated_as_read_and_an_inline_arrays_deduplicated`).
#[tokio::test(flavor = "multi_thread")]
async fn a_record_the_file_holds_twice_iterates_twice() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("twice", "1\n1\n{a:1}\n1\n{a:1}"),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let its = iterations_of(&db.pool, root).await;
    let want = ["1", "1", "{\"a\":1}", "1", "{\"a\":1}"];
    assert_eq!(
        its.iter()
            .map(|(_, i, v)| (*i, v.as_str()))
            .collect::<Vec<_>>(),
        want.iter()
            .enumerate()
            .map(|(i, v)| (i as i32, *v))
            .collect::<Vec<_>>()
    );
    for (id, _, value) in &its {
        let (state, outputs, message) = result_in(&db.pool, *id, "item").await;
        assert_eq!(state, "SUCCESS", "{message:?}");
        assert_eq!(outputs.unwrap()["value"], json!(value));
    }
}

/// `loop_over_ion.null_record`: a `null` record fails the loop with Kestra's
/// message. egret reads the whole file before the first iteration, so it
/// fails with NO iteration run, where Kestra ran the one before the null
/// (`iterations_run: ["0|1"]`) -- the difference the read-before-`advance()`
/// design costs (spec §9.3).
#[tokio::test(flavor = "multi_thread")]
async fn a_null_record_fails_the_loop_with_kestras_message() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let null = &loop_over_ion()["null_record"];
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("nulls", null["content"].as_str().unwrap()),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, null["loop_state"].as_str().unwrap());
    assert_eq!(message.as_deref(), null["error"].as_str());
    assert!(iterations_of(&db.pool, root).await.is_empty());
}

/// A values file over `workflow.storage.max_read_bytes` fails the loop
/// naming the cap (spec §9.3: there is no loop-width limit to reuse).
#[tokio::test(flavor = "multi_thread")]
async fn a_values_file_over_max_read_bytes_fails_the_loop_naming_the_cap() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    // One string record of READ_CAP + 1 bytes.
    let content = format!("\"{}\"", "a".repeat(READ_CAP as usize - 1));
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("too_big", &content),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (_, w, _) = result_in(&db.pool, root, "write").await;
    let file = w.unwrap()["uri"].as_str().unwrap().to_string();
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "FAILED");
    let message = message.unwrap();
    assert!(
        message.contains(&file)
            && message.contains(&format!(
                "workflow.storage.max_read_bytes ({READ_CAP} bytes)"
            )),
        "{message}"
    );
    assert!(iterations_of(&db.pool, root).await.is_empty());

    // The control: the same record one byte shorter is at the cap and loops.
    // (One record, not 2048 × `1`: a file's values are not deduplicated
    // since the user's ruling of 2026-10-03, and the repeat is
    // `a_record_the_file_holds_twice_iterates_twice`'s.)
    let at_cap = format!("\"{}\"", "a".repeat(READ_CAP as usize - 2));
    assert_eq!(at_cap.len() as u64, READ_CAP);
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("at_cap", &at_cap),
        json!({}),
    )
    .await
    .unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let its = iterations_of(&db.pool, root).await;
    assert_eq!(its.len(), 1);
    assert_eq!(its[0].2, "a".repeat(READ_CAP as usize - 2));
}

/// A BINARY Ion file -- what a JDBC plugin's STORE writes, each row with its
/// own version marker (`ion_rows`) -- is read as bytes and iterated record by
/// record.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_iterates_a_binary_ion_file() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let rows = fixture()["egret_expected"]["ion_rows"].as_array().unwrap();
    let mut bytes = Vec::new();
    for r in rows
        .iter()
        .filter(|r| r["sql_type"] == "int" || r["sql_type"] == "text_with_quote")
    {
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(r["bytes_b64"].as_str().unwrap())
                .unwrap(),
        );
    }
    assert!(String::from_utf8(bytes.clone()).is_err(), "not text");
    let source = format!(
        "id: binary\nnamespace: {NS}\ntasks:\n\
         \x20 - id: w\n    type: io.kestra.plugin.core.log.Log\n    message: w\n\
         \x20 - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
         values: \"{{{{ outputs.w.uri }}}}\"\n    concurrencyLimit: 0\n    tasks:\n\
         \x20     - id: item\n        type: io.kestra.plugin.core.debug.Return\n        \
         format: \"{{{{ item.value }}}}\"\n"
    );
    let root = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    // A run that wrote the binary file, indexed by `apply_outcome`.
    let claimed = claim(&db.pool, 1).await.unwrap();
    let c = &claimed[0];
    let file = format!(
        "{}rows.ion",
        uri::task_prefix(NS, "binary", root, "w", c.id).unwrap()
    );
    fs.store
        .put(&fs.object_uri(ws, &file).unwrap(), bytes.clone())
        .await
        .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    apply_outcome(
        &mut tx,
        c,
        &Outcome {
            files: vec![StoredFile {
                uri: file.clone(),
                size: bytes.len() as u64,
            }],
            ..Outcome::succeeded_with(json!({ "uri": file }))
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let values: Vec<Value> = iterations_of(&db.pool, root)
        .await
        .into_iter()
        .map(|(_, _, v)| serde_json::from_str(&v).unwrap())
        .collect();
    let want: Vec<Value> = rows
        .iter()
        .filter(|r| r["sql_type"] == "int" || r["sql_type"] == "text_with_quote")
        .map(|r| {
            egret_workflow::storage::ion::values_to_json(
                std::io::Cursor::new(r["ion_text"].as_str().unwrap().as_bytes().to_vec()),
                1,
            )
            .unwrap()
            .0[0]
                .clone()
        })
        .collect();
    assert_eq!(values, want);
}

/// A store OUTAGE while the values file is read is egret's, not the
/// author's (spec §11): the settle that would have started the loop rolls
/// back -- the `Write` is not finished, no attempt is spent, the loop is not
/// failed -- and once the store is back the same run finishes and the loop
/// iterates.
#[tokio::test(flavor = "multi_thread")]
async fn an_outage_reading_the_values_file_rolls_back_and_the_loop_waits() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let root = start_execution(
        &db.pool,
        ws,
        None,
        &loop_over_content("outage", "1\n2\n"),
        json!({}),
    )
    .await
    .unwrap();
    OUTAGE.store(true, Ordering::SeqCst);
    let reg = registry(&db.pool);
    let err = tick(&db.pool, 1, &reg)
        .await
        .expect_err("an outage is an Err");
    OUTAGE.store(false, Ordering::SeqCst);
    assert!(
        err.to_string().contains("internal storage is unavailable"),
        "{err}"
    );
    let (state, outputs, _) = result_in(&db.pool, root, "write").await;
    assert_eq!((state.as_str(), outputs), ("RUNNING", None), "rolled back");
    let attempts: i32 = sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'write'",
    )
    .bind(root)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1, "no attempt spent");
    let lp: Option<String> = sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'loop'",
    )
    .bind(root)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(lp, None, "the loop was never created");

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
    drain(&db.pool).await;
    let (state, _, message) = result_in(&db.pool, root, "loop").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(iterations_of(&db.pool, root).await.len(), 2);
}

/// A `values:` that renders to a `file://` or `nsfile://` URI -- Kestra's
/// other two `URIFetcher` schemes -- fails the loop naming why, instead of
/// being taken for JSON: the snapshot (the one readability decision) refuses
/// a URI that does not parse as `kestra:///…`, by name.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_or_nsfile_values_uri_fails_the_loop_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    for (flow, values) in [("local", "file:///etc/hosts"), ("nsf", "nsfile:///x.ion")] {
        let source = format!(
            "id: {flow}\nnamespace: {NS}\ntasks:\n\
             \x20 - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: \"{values}\"\n    tasks:\n\
             \x20     - id: item\n        type: io.kestra.plugin.core.debug.Return\n        format: x\n"
        );
        let root = start_execution(&db.pool, ws, None, &source, json!({}))
            .await
            .unwrap();
        drain(&db.pool).await;
        let (state, _, message) = result_in(&db.pool, root, "loop").await;
        assert_eq!(state, "FAILED");
        let message = message.unwrap_or_default();
        assert!(
            message.contains(values) && message.contains("kestra:///"),
            "{message}"
        );
    }
}
