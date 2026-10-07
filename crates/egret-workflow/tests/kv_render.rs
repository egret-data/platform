//! W9c Task 4: the `kv()` function over the KV store, through the real
//! worker (W9c spec §6). Requires DATABASE_URL.
//!
//! KV values are seeded as the artifact stored them -- a row in
//! `workflow.kv_entries` and its Ion object -- into a `MemStore` with an
//! outage switch in front of `get_stream` ([`Flaky`]): nothing here touches
//! MinIO. A render reads them on the scope's own namespace connection, from
//! the connect options this binary installs ([`setup`]).
//!
//! # Why `multi_thread`, and why the tests take turns
//!
//! As `tests/nsfiles_render.rs`: a lookup blocks the render thread on the
//! runtime, which needs the multi-thread runtime, and a claim is global (a
//! worker serves every workspace), so the tests take turns on [`SERIAL`].
//! Every test makes its own workspace, so their keys never meet.
//!
//! # The installed connect options carry a `lock_timeout`
//!
//! The database outage test holds `ACCESS EXCLUSIVE` on
//! `workflow.kv_entries` in a transaction of its own, so the scope's lookup
//! fails with a lock timeout on a live connection -- a query failure, which
//! is an outage (W9c spec §15: every database error of a `kv()` lookup is).
//! The options are process-wide (`nsfiles::install_db`, set once), so every
//! test here reads with that timeout; nothing else here takes a table lock.
//! The database is private to this process (`egret-test-db`).

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::runners::files::{FileScope, KvAnswer, KvArgs, RenderFiles};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::store::{self as kv_store, NewValue};
use egret_workflow::storage::kv::value::{KvData, KvNumber, KvType, KvValue};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::{self, CreatedBy};
use egret_workflow::storage::scope::Snapshot;
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::{execution_render_context, reap, start_execution, tick, Handlers};
use egret_workflow::{render_str, RenderContext};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The Task 1 artifact (`oracle-exec/w9c-kv.json`).
const ARTIFACT: &str = include_str!("../oracle-exec/w9c-kv.json");

/// While set, every read from the store fails as an unreachable S3 does
/// (spec §11's outage). Tests take turns on [`SERIAL`].
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

/// The process-wide store ([`Flaky`] over a `MemStore`) and namespace
/// connect options (the pool's, with a 200 ms `lock_timeout`), installed
/// once; a fresh workspace.
async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = storage::install(Arc::new(FileStore {
            store: Arc::new(Flaky(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps::default(),
            retention_days: 30,
        }));
        let _ = nsfiles::install_db(
            (*db.pool.connect_options())
                .clone()
                .options([("lock_timeout", "200ms")]),
        );
    });
    let fs = storage::current().expect("installed above");
    let (ws, _) = common::seed_workspace(&db).await;
    Some((db, ws, fs))
}

fn grant(ws: Uuid, ns: &str, access: Access) -> Grant {
    authorize(ws, Actor::Api, ns, Resource::Kv, access).unwrap()
}

/// One live revision `rev` of `key` in `ns` of `ws`, typed `kind`, whose
/// object is `ion` -- the bytes as the artifact stored them. `expires_in` is
/// seconds from the database's `now()` (negative: already expired).
#[allow(clippy::too_many_arguments)]
async fn seed(
    db: &Db,
    fs: &FileStore,
    ws: Uuid,
    ns: &str,
    key: &str,
    kind: KvType,
    ion: &[u8],
    expires_in: Option<f64>,
) {
    let rev: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(revision), 0) + 1 FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3",
    )
    .bind(ws)
    .bind(ns)
    .bind(key)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, \
                 CASE WHEN $7::float8 IS NULL THEN NULL \
                      ELSE now() + make_interval(secs => $7::float8) END)",
    )
    .bind(ws)
    .bind(ns)
    .bind(key)
    .bind(rev)
    .bind(kind.as_str())
    .bind(ion.len() as i64)
    .bind(expires_in)
    .execute(&db.pool)
    .await
    .unwrap_or_else(|e| panic!("seed {ns}:{key}: {e}"));
    let object = fs.namespace_kv_uri(
        &grant(ws, ns, Access::Read),
        &KvKey::parse(key).unwrap(),
        rev as u32,
    );
    fs.store.put(&object, ion.to_vec()).await.unwrap();
}

/// A STRING value.
async fn seed_str(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, key: &str, text: &str) {
    let ion = serde_json::to_string(text).unwrap();
    seed(db, fs, ws, ns, key, KvType::String, ion.as_bytes(), None).await;
}

async fn soft_delete_latest(db: &Db, ws: Uuid, ns: &str, key: &str) {
    sqlx::query(
        "UPDATE workflow.kv_entries SET deleted = true, deleted_at = now() \
         WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND revision = \
           (SELECT max(revision) FROM workflow.kv_entries \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3)",
    )
    .bind(ws)
    .bind(ns)
    .bind(key)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// Tick until nothing is claimable.
async fn drain(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(storage::current());
    for _ in 0..4096 {
        if tick(pool, 16, &reg).await.expect("tick") == 0 {
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

async fn attempts(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("attempts")
}

/// A `Return` task whose `format` is `format` (a whole template); it may
/// fail without stopping the flow.
fn ret(id: &str, format: &str) -> String {
    let format = serde_json::to_string(format).unwrap();
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.debug.Return\n    allowFailure: true\n    \
         format: {format}\n"
    )
}

fn flow(id: &str, ns: &str, tasks: &[String]) -> String {
    format!("id: {id}\nnamespace: {ns}\ntasks:\n{}", tasks.concat())
}

async fn run(db: &Db, ws: Uuid, source: &str) -> Uuid {
    let ex = start_execution(&db.pool, ws, None, source, json!({}))
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

/// A cell name as a task id.
fn task_id(name: &str) -> String {
    name.replace(['.', '-'], "_")
}

/// One fixture cell checked against its run: `outcome` exact; an `ok` cell's
/// `outputs.value`; an error cell's `message` (contained: egret's task
/// message wraps the render error) or each of `message_contains`.
async fn check_cell(db: &Db, ex: Uuid, name: &str, cell: &Value) {
    let id = task_id(name);
    match cell["outcome"].as_str().unwrap() {
        "ok" => assert_eq!(
            value_of(db, ex, &id).await,
            cell["outputs"]["value"].as_str().unwrap(),
            "{name}"
        ),
        "error" => {
            let message = error_of(db, ex, &id).await;
            if let Some(want) = cell["message"].as_str() {
                assert!(message.contains(want), "{name}: {message}\n  want: {want}");
            }
            for fragment in cell["message_contains"].as_array().into_iter().flatten() {
                let fragment = fragment.as_str().unwrap();
                assert!(
                    message.contains(fragment),
                    "{name}: {message} lacks {fragment}"
                );
            }
        }
        other => panic!("{name}: outcome {other}"),
    }
}

/// w9c `kv_function._setup`: `k` = `{a:1}` in `w9c.fn`, `k` = `'other'` in
/// `w9c.fn.other`.
async fn seed_kv_function(db: &Db, fs: &FileStore, ws: Uuid) {
    seed(db, fs, ws, "w9c.fn", "k", KvType::Json, b"{a:1}", None).await;
    seed_str(db, fs, ws, "w9c.fn.other", "k", "other").await;
}

/// w9c `inheritance._setup`, each value `{key}@{level}` as the artifact's.
async fn seed_inheritance(db: &Db, fs: &FileStore, ws: Uuid) {
    let label = |ns: &str| -> String {
        match ns.strip_prefix("w9c.") {
            Some(rest) => rest.strip_prefix("inh.").unwrap_or(rest).to_string(),
            None => ns.to_string(),
        }
    };
    let mut live = vec![
        ("w9c.inh", "top"),
        ("w9c.inh.a", "mid"),
        ("w9c.inh", "mid"),
        ("w9c.inh.a.b", "own"),
        ("w9c.inh", "own"),
        ("w9c.inh.a.b.c", "child"),
        ("w9c", "root"),
        ("w9c.inh", "delmid"),
        ("w9c.inh", "midexp"),
    ];
    for k in ["deep1", "deep2"] {
        live.push(("w9c.inh", k));
    }
    for (ns, k) in live {
        seed_str(db, fs, ws, ns, k, &format!("{k}@{}", label(ns))).await;
    }
    seed_str(db, fs, ws, "w9c.inh.a", "delmid", "delmid@a").await;
    soft_delete_latest(db, ws, "w9c.inh.a", "delmid").await;
    let expired = |k: &str, level: &str| format!("\"{k}@{level}\"");
    seed(
        db,
        fs,
        ws,
        "w9c.inh.a",
        "midexp",
        KvType::String,
        expired("midexp", "a").as_bytes(),
        Some(-1.0),
    )
    .await;
    for k in ["deep1", "deep2"] {
        seed(
            db,
            fs,
            ws,
            "w9c.inh.a.b",
            k,
            KvType::String,
            expired(k, "a.b").as_bytes(),
            Some(-1.0),
        )
        .await;
    }
}

/// Every w9c `kv_function`, `inheritance` (its `kv()` cells) and `types`
/// cell, each through a real task run, compared as the cell's `compare`
/// says.
///
/// Two cells are not run here, by name:
/// - `kv_function.no_execution` is a render with no execution:
///   `a_flow_trigger_condition_calling_kv_is_refused_by_name` below;
/// - `kv_function.missing_eom_false_is_null` writes Pebble's `is null`, a
///   test egret's renderer does not bind (`runners::env`'s fence keeps it
///   unknown, a renderer gap that is not the KV store's); its value is
///   asserted with `== null` here and in `kv_fn::kv_error_on_missing_defaults_to_true`.
#[tokio::test(flavor = "multi_thread")]
async fn kv_matches_the_artifact() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let artifact: Value = serde_json::from_str(ARTIFACT).unwrap();
    let expected = &artifact["egret_expected"];

    // kv_function, as a flow of w9c.fn.
    seed_kv_function(&db, &fs, ws).await;
    let cells = expected["kv_function"].as_object().unwrap();
    const NOT_RUN: [&str; 2] = ["no_execution", "missing_eom_false_is_null"];
    let mut tasks = Vec::new();
    let mut run_cells = Vec::new();
    for (name, cell) in cells {
        if name.starts_with('_') || NOT_RUN.contains(&name.as_str()) {
            continue;
        }
        tasks.push(ret(&task_id(name), cell["expr"].as_str().unwrap()));
        run_cells.push((name.clone(), cell.clone()));
    }
    tasks.push(ret(
        "is_null",
        "{{ kv('missing', errorOnMissing=false) == null }}",
    ));
    let ex = run(&db, ws, &flow("kvfn", "w9c.fn", &tasks)).await;
    for (name, cell) in &run_cells {
        check_cell(&db, ex, name, cell).await;
    }
    assert_eq!(
        value_of(&db, ex, "is_null").await,
        cells["missing_eom_false_is_null"]["outputs"]["value"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        run_cells.len() + NOT_RUN.len(),
        25,
        "every kv_function cell"
    );

    // inheritance, as a flow of w9c.inh.a.b.
    seed_inheritance(&db, &fs, ws).await;
    let mut tasks = Vec::new();
    let mut inh = Vec::new();
    for (name, cell) in expected["inheritance"].as_object().unwrap() {
        let Some(template) = cell["task"].as_str() else {
            continue; // the Get cells: Task 5
        };
        tasks.push(ret(&task_id(name), template));
        inh.push((name.clone(), cell.clone()));
    }
    let ex = run(&db, ws, &flow("inh", "w9c.inh.a.b", &tasks)).await;
    for (name, cell) in &inh {
        check_cell(&db, ex, name, cell).await;
    }
    assert_eq!(inh.len(), 15, "every kv() inheritance cell");

    // types: the artifact's bytes, each read back by `kv()`.
    let mut tasks = Vec::new();
    let mut typed = Vec::new();
    for (name, cell) in expected["types"].as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let kind = KvType::parse(cell["api"]["type"].as_str().unwrap()).unwrap();
        let key = task_id(name);
        let ion = cell["ion_bytes"].as_str().unwrap();
        seed(&db, &fs, ws, "w9c.types", &key, kind, ion.as_bytes(), None).await;
        tasks.push(ret(&key, &format!("{{{{ kv('{key}') }}}}")));
        if cell.get("kv_tojson").and_then(Value::as_str).is_some() {
            tasks.push(ret(
                &format!("{key}_json"),
                &format!("{{{{ kv('{key}') | toJson }}}}"),
            ));
        }
        typed.push((key, cell.clone()));
    }
    let ex = run(&db, ws, &flow("types", "w9c.types", &tasks)).await;
    for (key, cell) in &typed {
        assert_eq!(
            value_of(&db, ex, key).await,
            cell["kv_prints"].as_str().unwrap(),
            "{key}"
        );
        if let Some(want) = cell.get("kv_tojson").and_then(Value::as_str) {
            assert_eq!(
                value_of(&db, ex, &format!("{key}_json")).await,
                want,
                "{key}"
            );
        }
    }
    assert_eq!(typed.len(), 39, "every types cell");
}

/// Spec §6, "where": every render with an execution -- a `runIf`, an `If`'s
/// condition, a `Switch`'s value, a task property.
#[tokio::test(flavor = "multi_thread")]
async fn kv_in_run_if_if_switch_and_a_task_property() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.where";
    seed(&db, &fs, ws, ns, "on", KvType::Boolean, b"true", None).await;
    seed(&db, &fs, ws, ns, "off", KvType::Boolean, b"false", None).await;
    seed_str(&db, &fs, ws, ns, "choice", "SECOND").await;
    let source = format!(
        "id: where\nnamespace: {ns}\ntasks:\n\
         \x20 - id: run_on\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ kv('on') }}}}\"\n    message: on\n\
         \x20 - id: run_off\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ kv('off') }}}}\"\n    message: off\n\
         \x20 - id: branch\n    type: io.kestra.plugin.core.flow.If\n    \
         condition: \"{{{{ kv('on') }}}}\"\n    then:\n\
         \x20     - id: then_task\n        type: io.kestra.plugin.core.log.Log\n        message: then\n    \
         else:\n\
         \x20     - id: else_task\n        type: io.kestra.plugin.core.log.Log\n        message: else\n\
         \x20 - id: pick\n    type: io.kestra.plugin.core.flow.Switch\n    \
         value: \"{{{{ kv('choice') }}}}\"\n    cases:\n\
         \x20     FIRST:\n        - id: first\n          type: io.kestra.plugin.core.log.Log\n          message: first\n\
         \x20     SECOND:\n        - id: second\n          type: io.kestra.plugin.core.log.Log\n          message: second\n\
         \x20 - id: prop\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ kv('choice') }}}}-{{{{ kv('on') }}}}\"\n"
    );
    let ex = run(&db, ws, &source).await;
    assert_eq!(result_of(&db.pool, ex, "run_on").await.0, "SUCCESS");
    assert_eq!(result_of(&db.pool, ex, "run_off").await.0, "SKIPPED");
    assert_eq!(result_of(&db.pool, ex, "then_task").await.0, "SUCCESS");
    let created = |task: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
            )
            .bind(ex)
            .bind(task)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(created("else_task").await, 0, "the If took `then`");
    assert_eq!(result_of(&db.pool, ex, "second").await.0, "SUCCESS");
    assert_eq!(created("first").await, 0, "the Switch took SECOND");
    assert_eq!(value_of(&db, ex, "prop").await, "SECOND-true");
}

/// Spec §6, "no snapshot": a `runIf` rendered inside `settle` (the
/// transaction that finishes `a`) reads a key committed AFTER the execution
/// started, by Task 3's own writer. The control (`c`) shows the `runIf` was
/// evaluated: a key that is not there, `errorOnMissing: false`, skips.
#[tokio::test(flavor = "multi_thread")]
async fn kv_in_settle_reads_a_key_committed_after_the_execution_started() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.settle";
    let source = format!(
        "id: settle\nnamespace: {ns}\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ kv('late') }}}}\"\n    message: b\n\
         \x20 - id: c\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ kv('never', errorOnMissing=false) }}}}\"\n    message: c\n"
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let written = kv_store::set(
        &db.pool,
        &fs,
        &grant(ws, ns, Access::Write),
        &KvKey::parse("late").unwrap(),
        NewValue {
            value: KvValue {
                kind: KvType::Boolean,
                data: KvData::Bool(true),
            },
            description: None,
            ttl: None,
            overwrite: true,
        },
        CreatedBy::Execution(Uuid::nil()),
    )
    .await;
    assert!(written.is_ok(), "{written:?}");
    drain(&db.pool).await;
    assert_eq!(result_of(&db.pool, ex, "b").await.0, "SUCCESS");
    assert_eq!(result_of(&db.pool, ex, "c").await.0, "SKIPPED");
}

/// `KvFunction.java:61-66`, spec §4.6: with no namespace the walk answers a
/// parent's key; the flow's own namespace written explicitly does not walk,
/// and neither does any other.
#[tokio::test(flavor = "multi_thread")]
async fn kv_with_no_namespace_walks_and_with_the_flows_own_does_not() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_str(&db, &fs, ws, "w9c.walk", "top", "top@walk").await;
    seed_str(&db, &fs, ws, "w9c.walk.a", "mid", "mid@a").await;
    let ex = run(
        &db,
        ws,
        &flow(
            "walk",
            "w9c.walk.a.b",
            &[
                ret("walked", "{{ kv('top') }}"),
                ret("own", "{{ kv('top', flow.namespace) }}"),
                ret("own_named", "{{ kv(key='top', namespace='w9c.walk.a.b') }}"),
                ret("mid_explicit", "{{ kv('top', 'w9c.walk.a') }}"),
                ret("parent", "{{ kv('top', 'w9c.walk') }}"),
                ret("nearest", "{{ kv('mid') }}"),
            ],
        ),
    )
    .await;
    assert_eq!(value_of(&db, ex, "walked").await, "top@walk");
    for task in ["own", "own_named"] {
        assert!(
            error_of(&db, ex, task)
                .await
                .contains("The key 'top' does not exist in the namespace 'w9c.walk.a.b'."),
            "{task}"
        );
    }
    assert!(error_of(&db, ex, "mid_explicit")
        .await
        .contains("The key 'top' does not exist in the namespace 'w9c.walk.a'."));
    assert_eq!(value_of(&db, ex, "parent").await, "top@walk");
    assert_eq!(value_of(&db, ex, "nearest").await, "mid@a");
}

/// The rows and objects of `ns`, as a reader could change them.
async fn state_of(db: &Db, fs: &FileStore, ws: Uuid, ns: &str) -> (Vec<Value>, Vec<String>) {
    let rows: Vec<(i64, i32, bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, revision, deleted, expires_at FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 ORDER BY id",
    )
    .bind(ws)
    .bind(ns)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let rows = rows
        .into_iter()
        .map(|(id, rev, deleted, expires)| json!([id, rev, deleted, expires]))
        .collect();
    let listing = fs
        .store
        .list(&format!("workflow/{ws}/"), None)
        .await
        .unwrap();
    let mut objects: Vec<String> = listing.objects.into_iter().map(|o| o.key).collect();
    objects.sort();
    (rows, objects)
}

/// The user's ruling (2026-10-07, spec §4.5): an expired key reads exactly
/// as a missing one and honours `errorOnMissing` both ways; reads never
/// write -- the rows and objects are unchanged (w9c `expiry.single.kv_*`).
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_key_is_missing_and_honours_error_on_missing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.exp";
    seed(
        &db,
        &fs,
        ws,
        ns,
        "gone",
        KvType::String,
        b"\"x\"",
        Some(-1.0),
    )
    .await;
    seed(
        &db,
        &fs,
        ws,
        ns,
        "live",
        KvType::String,
        b"\"y\"",
        Some(3600.0),
    )
    .await;
    let before = state_of(&db, &fs, ws, ns).await;
    assert_eq!((before.0.len(), before.1.len()), (2, 2), "{before:?}");
    let ex = run(
        &db,
        ws,
        &flow(
            "exp",
            ns,
            &[
                ret("default", "{{ kv('gone') }}"),
                ret("eom_true", "{{ kv('gone', errorOnMissing=true) }}"),
                ret("eom_false", "{{ kv('gone', errorOnMissing=false) }}"),
                ret("live", "{{ kv('live') }}"),
            ],
        ),
    )
    .await;
    for task in ["default", "eom_true"] {
        assert!(
            error_of(&db, ex, task)
                .await
                .contains("The key 'gone' does not exist in the namespace 'w9c.exp'."),
            "{task}"
        );
    }
    assert_eq!(value_of(&db, ex, "eom_false").await, "");
    assert_eq!(value_of(&db, ex, "live").await, "y");
    assert_eq!(state_of(&db, &fs, ws, ns).await, before, "a read wrote");
}

/// Spec §5: the workspace is always the execution's. The same namespace and
/// key in another workspace is missing here, and its value never appears.
#[tokio::test(flavor = "multi_thread")]
async fn another_workspaces_key_is_missing_and_never_its_value() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let (other, _) = common::seed_workspace(&db).await;
    seed_str(&db, &fs, other, "w9c.ws", "k", "OTHER-WORKSPACE-VALUE").await;
    let ex = run(
        &db,
        ws,
        &flow(
            "ws",
            "w9c.ws",
            &[
                ret("own", "{{ kv('k') }}"),
                ret("named", "{{ kv('k', 'w9c.ws') }}"),
                ret("quiet", "{{ kv('k', errorOnMissing=false) }}"),
            ],
        ),
    )
    .await;
    for task in ["own", "named"] {
        let e = error_of(&db, ex, task).await;
        assert!(
            e.contains("The key 'k' does not exist in the namespace 'w9c.ws'."),
            "{task}: {e}"
        );
        assert!(!e.contains("OTHER-WORKSPACE-VALUE") && !e.contains(&other.to_string()));
    }
    assert_eq!(value_of(&db, ex, "quiet").await, "");
    // The control: the other workspace's own flow reads it.
    let ex = run(
        &db,
        other,
        &flow("ws", "w9c.ws", &[ret("own", "{{ kv('k') }}")]),
    )
    .await;
    assert_eq!(value_of(&db, ex, "own").await, "OTHER-WORKSPACE-VALUE");
}

/// W9c spec §3: a namespace with a `_kv` segment is refused by name, through
/// `authorize`'s `check_namespace`, whatever `errorOnMissing` says; so is a
/// malformed one (w9c `kv_function.bad_ns`). The control one character away
/// reads as an empty namespace.
#[tokio::test(flavor = "multi_thread")]
async fn a_kv_segment_namespace_argument_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let ex = run(
        &db,
        ws,
        &flow(
            "seg",
            "w9c.seg",
            &[
                ret("kv_seg", "{{ kv('k', 'a._kv.b', false) }}"),
                ret("bad", "{{ kv('k', 'no..pe', false) }}"),
                ret("control", "{{ kv('k', 'a.kv.b', false) }}"),
            ],
        ),
    )
    .await;
    let e = error_of(&db, ex, "kv_seg").await;
    assert!(e.contains("'a._kv.b'") && e.contains("'_kv'"), "{e}");
    assert!(error_of(&db, ex, "bad").await.contains("no..pe"));
    assert_eq!(value_of(&db, ex, "control").await, "");
}

/// Spec §11: a database outage during `kv()` is egret's, not the author's.
/// The lookup's query fails (a lock timeout on `workflow.kv_entries`, held by
/// this test): the tick is an `Err`, the run keeps its lease and records
/// nothing, no attempt is spent -- not `null`, which `errorOnMissing: false`
/// would answer for a missing key. Then the settle half: a `runIf` in the
/// transaction that finishes `a` rolls it back whole. Once the lock is gone
/// and the lease lapsed, both finish on their first attempt.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_outage_during_kv_re_claims_with_no_attempt_spent() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.dbout";
    seed_str(&db, &fs, ws, ns, "k", "back").await;
    let handler = flow(
        "dbout",
        ns,
        &[ret("r", "{{ kv('k', errorOnMissing=false) }}")],
    );
    let ex = start_execution(&db.pool, ws, None, &handler, json!({}))
        .await
        .unwrap();
    let before = attempts(&db.pool, ex, "r").await;
    let reg = Handlers::with_core_tasks(storage::current());

    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE workflow.kv_entries IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let ticked = tick(&db.pool, 16, &reg).await;
    let err = ticked.expect_err("an outage is the worker's error, not the task's");
    assert!(
        err.to_string().contains("key 'k' in namespace 'w9c.dbout'")
            && err.to_string().contains("database is unavailable"),
        "{err}"
    );
    let (state, outputs, message) = result_of(&db.pool, ex, "r").await;
    assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
    assert_eq!(
        attempts(&db.pool, ex, "r").await,
        before,
        "no attempt spent"
    );

    // The settle half, under the same lock.
    let settle = format!(
        "id: dbout_settle\nnamespace: {ns}\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ kv('k') }}}}\"\n    message: b\n"
    );
    let ex2 = start_execution(&db.pool, ws, None, &settle, json!({}))
        .await
        .unwrap();
    // `r`'s lease is still live, so only `a` is claimable.
    let ticked = tick(&db.pool, 16, &reg).await;
    let err = ticked.expect_err("an outage in settle is the worker's error");
    assert!(err.to_string().contains("key 'k'"), "{err}");
    let (state, _, _) = result_of(&db.pool, ex2, "a").await;
    assert_eq!(state, "RUNNING", "a's outcome rolled back");
    let b: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'b'",
    )
    .bind(ex2)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(b, 0, "b was never created");
    lock.rollback().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 2);
    drain(&db.pool).await;
    assert_eq!(value_of(&db, ex, "r").await, "back");
    assert_eq!(attempts(&db.pool, ex, "r").await, before);
    assert_eq!(result_of(&db.pool, ex2, "b").await.0, "SUCCESS");
}

/// Spec §11: a store outage reading the value is egret's too: the tick is an
/// `Err` naming the key, no attempt is spent, and once the store is back the
/// re-claimed run reads the value on that attempt.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_outage_reading_the_value_re_claims_with_no_attempt_spent() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.sout";
    seed_str(&db, &fs, ws, ns, "k", "back").await;
    let source = flow(
        "sout",
        ns,
        &[ret("r", "{{ kv('k', errorOnMissing=false) }}")],
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let before = attempts(&db.pool, ex, "r").await;
    OUTAGE.store(true, Ordering::SeqCst);
    let ticked = tick(&db.pool, 16, &Handlers::with_core_tasks(storage::current())).await;
    OUTAGE.store(false, Ordering::SeqCst);
    let err = ticked.expect_err("an outage is the worker's error, not the task's");
    assert!(
        err.to_string().contains("key 'k' in namespace 'w9c.sout'")
            && err.to_string().contains("internal storage is unavailable"),
        "{err}"
    );
    assert!(
        !err.to_string().contains("workflow/"),
        "names no storage key: {err}"
    );
    let (state, outputs, message) = result_of(&db.pool, ex, "r").await;
    assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
    assert_eq!(
        attempts(&db.pool, ex, "r").await,
        before,
        "no attempt spent"
    );

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
    drain(&db.pool).await;
    assert_eq!(value_of(&db, ex, "r").await, "back");
    assert_eq!(attempts(&db.pool, ex, "r").await, before);
}

/// Options that tag the scope's connections, so the test can find them
/// in `pg_stat_activity` by application name.
fn tagged(db: &Db, tag: &str) -> Arc<sqlx::postgres::PgConnectOptions> {
    Arc::new((*db.pool.connect_options()).clone().application_name(tag))
}

/// `(backends, advisory locks held, backends inside a transaction)` of `tag`.
async fn held_by(db: &Db, tag: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT count(DISTINCT a.pid), \
                count(l.pid) FILTER (WHERE l.locktype = 'advisory'), \
                count(DISTINCT a.pid) FILTER (WHERE a.state LIKE 'idle in transaction%') \
           FROM pg_stat_activity a LEFT JOIN pg_locks l ON l.pid = a.pid \
          WHERE a.application_name = $1",
    )
    .bind(tag)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// `held_by(tag).0` reaches `want` within 5 s (a closed connection's backend
/// exits a moment after the goodbye), or the last count.
async fn settles_at(db: &Db, tag: &str, want: i64) -> i64 {
    let mut n = held_by(db, tag).await.0;
    for _ in 0..250 {
        if n == want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        n = held_by(db, tag).await.0;
    }
    n
}

/// The hold audit, measured (Task 4 step 5): between two `kv()` calls of one
/// render the scope holds exactly its one namespace connection, idle, in no
/// transaction and with no advisory lock; the object read's private pool
/// is closed when `kv()` returns, even when it was used (an object that is
/// not the size its row records takes it for one statement); and the
/// render's end closes the connection.
#[tokio::test(flavor = "multi_thread")]
async fn the_scope_holds_no_lock_and_releases_its_connection_at_end_render() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.hold";
    seed_str(&db, &fs, ws, ns, "a", "A").await;
    seed_str(&db, &fs, ws, ns, "liar", "abc").await;
    // The row says 5 bytes ("abc" quoted); the object now holds 6.
    let object = fs.namespace_kv_uri(
        &grant(ws, ns, Access::Read),
        &KvKey::parse("liar").unwrap(),
        1,
    );
    fs.store.put(&object, b"\"abcd\"".to_vec()).await.unwrap();

    let tag = format!("w9c-t4-hold-{}", Uuid::new_v4().simple());
    let snap = Arc::new(
        Snapshot::from_parts(Default::default(), fs.clone(), ws, None)
            .with_namespace_files(Some(ns.into()), Some(tagged(&db, &tag))),
    );
    let (s, handle, pool, t) = (
        snap.clone(),
        tokio::runtime::Handle::current(),
        db.pool.clone(),
        tag.clone(),
    );
    let observed = std::thread::spawn(move || {
        let held = |label: &str| {
            let d = Db { pool: pool.clone() };
            (label.to_string(), handle.block_on(held_by(&d, &t)))
        };
        let args = |key| KvArgs {
            key,
            namespace: None,
        };
        let first = s.kv(&args("a"));
        let after_first = held("after the first kv()");
        let liar = s.kv(&args("liar"));
        let after_liar = held("after the mismatch");
        let kept = s.holds_a_namespace_connection();
        s.end_render();
        (first, after_first, liar, after_liar, kept)
    })
    .join()
    .unwrap();
    assert_eq!(observed.0, Ok(KvAnswer::Value(KvData::String("A".into()))));
    assert_eq!(
        observed.1 .1,
        (1, 0, 0),
        "{}: one idle connection, no lock",
        observed.1 .0
    );
    let liar = observed.2.expect_err("a mismatch is refused");
    assert!(liar.contains("does not match its record"), "{liar}");
    assert_eq!(
        observed.3 .1,
        (1, 0, 0),
        "{}: the private pool was closed, only the render's connection is left",
        observed.3 .0
    );
    assert!(observed.4, "the render keeps its connection until it ends");
    assert!(!snap.holds_a_namespace_connection());
    assert_eq!(
        settles_at(&db, &tag, 0).await,
        0,
        "a backend outlived the render"
    );
    assert_eq!(snap.infrastructure_failure(), None);

    // Through a template render: the render's end closes it.
    let cx = RenderContext::new().with_files(FileScope(snap.clone()));
    assert_eq!(render_str("{{ kv('a') }}{{ kv('a') }}", &cx).unwrap(), "AA");
    assert!(!snap.holds_a_namespace_connection());
    assert_eq!(settles_at(&db, &tag, 0).await, 0);
}

/// Spec §6: a render with no execution -- a Flow trigger's conditions --
/// refuses `kv()` by name (w9c `kv_function.no_execution`).
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_trigger_condition_calling_kv_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_str(&db, &fs, ws, "w9c.trig", "k", "v").await;
    let ex = run(
        &db,
        ws,
        &flow("fired", "w9c.trig", &[ret("w", "{{ kv('k') }}")]),
    )
    .await;
    assert_eq!(value_of(&db, ex, "w").await, "v", "the task run reads it");
    let mut conn = db.pool.acquire().await.unwrap();
    let cx = execution_render_context(&mut conn, ex).await.unwrap();
    assert_eq!(
        render_str("{{ execution.id }}", &cx).unwrap(),
        ex.to_string()
    );
    for call in ["{{ kv('k') }}", "{{ kv('k', 'w9c.trig', false) }}"] {
        let err = render_str(call, &cx).unwrap_err().to_string();
        assert!(
            err.contains("The 'kv' function needs a running execution"),
            "{call}: {err}"
        );
    }
}

/// The corpus's `valids/retry-with-flowable-errors` reads its counter with
/// `{{ kv(namespace=flow.namespace, key='retry_counter_1') }}` after its
/// `set_kv` wrote 1 (`kvType: NUMBER`). `Set` is Task 5's, so the value is
/// written here by Task 3's writer as `set_kv` would; the read renders 1 and
/// computes with it. (The `AllowFailure` part stays refused; the corpus
/// entry stays baselined.)
#[tokio::test(flavor = "multi_thread")]
async fn retry_with_flowable_errors_reads_its_counter() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "io.kestra.tests";
    kv_store::set(
        &db.pool,
        &fs,
        &grant(ws, ns, Access::Write),
        &KvKey::parse("retry_counter_1").unwrap(),
        NewValue {
            value: KvValue {
                kind: KvType::Number,
                data: KvData::Number(KvNumber::Int("1".into())),
            },
            description: None,
            ttl: None,
            overwrite: true,
        },
        CreatedBy::Execution(Uuid::nil()),
    )
    .await
    .unwrap();
    let ex = run(
        &db,
        ws,
        &flow(
            "retry-with-flowable-errors",
            ns,
            &[
                ret("read", "{{ kv(namespace=flow.namespace, key='retry_counter_1') }}"),
                ret(
                    "compare",
                    "{{ kv(namespace=flow.namespace, key='retry_counter_1') < 2 ? 'ko' : 'It works' }}",
                ),
                ret(
                    "incr",
                    "{{ kv(namespace=flow.namespace, key='retry_counter_1') + 1 }}",
                ),
            ],
        ),
    )
    .await;
    assert_eq!(value_of(&db, ex, "read").await, "1");
    assert_eq!(value_of(&db, ex, "compare").await, "ko");
    assert_eq!(value_of(&db, ex, "incr").await, "2");
}

/// Task 4 step 5's measurement: the median of 50 `kv()` renders on the
/// scope's connection (one connect, then 50 lookups and object reads), and
/// of 50 renders each opening its own scope (a connect each, as a task run
/// does). Ignored: a measurement, not a check (`--ignored --nocapture`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: cargo test --test kv_render -- --ignored --nocapture"]
async fn the_cost_of_a_kv_render() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.cost";
    seed_str(&db, &fs, ws, ns, "k", "v").await;
    let opts = Arc::new((*db.pool.connect_options()).clone());
    let median = |mut took: Vec<std::time::Duration>| {
        took.sort();
        (took[25], took[0], took[49])
    };
    // Each snapshot captures this runtime's handle, so it is made here.
    let scope = || {
        Snapshot::from_parts(Default::default(), fs.clone(), ws, None)
            .with_namespace_files(Some(ns.into()), Some(opts.clone()))
    };
    let snap = scope();
    let fresh: Vec<Snapshot> = (0..50).map(|_| scope()).collect();
    // A plain thread, as a render's: `kv()` refuses a runtime worker's. The
    // test body runs on `block_on`'s thread, so the workers stay free.
    let (warm, cold) = std::thread::spawn(move || {
        let args = KvArgs {
            key: "k",
            namespace: None,
        };
        snap.kv(&args).unwrap();
        let mut warm = Vec::new();
        for _ in 0..50 {
            let t = std::time::Instant::now();
            snap.kv(&args).unwrap();
            warm.push(t.elapsed());
        }
        snap.end_render();
        let mut cold = Vec::new();
        for snap in fresh {
            let t = std::time::Instant::now();
            snap.kv(&args).unwrap();
            snap.end_render();
            cold.push(t.elapsed());
        }
        (warm, cold)
    })
    .join()
    .unwrap();
    let (m, lo, hi) = median(warm);
    println!("kv() on an open scope x50: median {m:?}, min {lo:?}, max {hi:?}");
    let (m, lo, hi) = median(cold);
    println!("kv() with a fresh scope (connect + close) x50: median {m:?}, min {lo:?}, max {hi:?}");
}
