//! W9b Tasks 6 and 7: `io.kestra.plugin.core.namespace.{UploadFiles,
//! DownloadFiles, DeleteFiles, PurgeFiles}` through the real worker (W9b spec
//! §8.1-§8.4), against `oracle-exec/w9b-namespace-files.json`'s `tasks.*` and
//! `purge` cells. Requires DATABASE_URL.
//!
//! The store is [`Probe`] over a `MemStore`, installed once for this binary
//! with `max_namespace_files` = [`CAP`]: nothing here touches MinIO. Every
//! test makes its own workspace, so their files never meet; the tests take
//! turns on [`SERIAL`] because a claim is global and [`OUTAGE`] is too.
//! `multi_thread`: a render that looks a namespace file up blocks its thread
//! on the runtime (W9a's constraint).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{
    list_live, open, put, resolve, revisions, soft_delete, Conflict, Want, Written,
};
use egret_workflow::storage::nsfiles::NsError;
use egret_workflow::storage::nsfiles::{self, CreatedBy};
use egret_workflow::storage::{self, uri, FileStore};
use egret_workflow::worker::handlers::namespace::{self as nst, DownloadFiles, UploadFiles};
use egret_workflow::worker::{reap, start_execution, tick, Handlers};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// This binary's `workflow.storage.max_namespace_files`: the download
/// premise lists 10 rows (6 files, 4 directories), exactly this.
const CAP: u64 = 10;

/// The fixture's namespace for the task flows.
const NS: &str = "w9b.tasks";

/// While set, `get_stream` fails as an unreachable S3 does (spec §11).
static OUTAGE: AtomicBool = AtomicBool::new(false);

/// The next `put_stream` to a key ending with this fails once, as an
/// unreachable S3 does: the injectable fault of fix round 1's I1 test.
static FAIL_PUT_ONCE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

struct Probe(egret_storage::MemStore);

#[async_trait::async_trait]
impl ObjectStore for Probe {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, object: &str) -> Result<(), StorageError> {
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
        {
            let mut fault = FAIL_PUT_ONCE.lock().unwrap();
            if fault.as_deref().is_some_and(|suffix| uri.ends_with(suffix)) {
                *fault = None;
                return Err(StorageError::S3(format!(
                    "{uri}: dispatch failure: connection refused"
                )));
            }
        }
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
}

/// The one object store of this binary: the installed `FileStore` and every
/// small-capped one ([`capped`]) read and write the same objects.
fn probe() -> Arc<Probe> {
    static P: std::sync::OnceLock<Arc<Probe>> = std::sync::OnceLock::new();
    P.get_or_init(|| Arc::new(Probe(egret_storage::MemStore::new())))
        .clone()
}

/// A `FileStore` over the same objects as the installed one, with `caps`:
/// what a handler built directly with it sees (`storage::install` is
/// first-wins, so a cap under test cannot be installed a second time).
fn capped(caps: storage::Caps) -> Arc<FileStore> {
    Arc::new(FileStore {
        store: probe(),
        bucket: "egret-test".to_string(),
        prefix: "workflow/".to_string(),
        caps,
        retention_days: 30,
    })
}

/// The shipping registry, but with `UploadFiles` and `DownloadFiles` over
/// `fs`.
fn registry_over(pool: &sqlx::PgPool, fs: Arc<FileStore>) -> Handlers {
    let mut r = Handlers::with_core_tasks(storage::current());
    egret_workflow::worker::handlers::storage::register(&mut r, pool.clone(), storage::current());
    r.add_publishable(Arc::new(UploadFiles::new(pool.clone(), Some(fs.clone()))));
    r.add_publishable(Arc::new(DownloadFiles::new(pool.clone(), Some(fs))));
    r
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = storage::install(Arc::new(FileStore {
            store: probe(),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps {
                max_namespace_files: CAP,
                ..storage::Caps::default()
            },
            retention_days: 30,
        }));
    });
    storage::current().expect("installed above")
}

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    nsfiles::install_db_for_tests(&db.pool);
    OUTAGE.store(false, Ordering::SeqCst);
    *FAIL_PUT_ONCE.lock().unwrap() = None;
    let fs = installed();
    let (ws, _) = common::seed_workspace(&db).await;
    Some((db, ws, fs))
}

/// The shipping registry for `container_runner = none`, as
/// `runtime::handlers_for` builds it.
fn registry(pool: &sqlx::PgPool) -> Handlers {
    let mut r = Handlers::with_core_tasks(storage::current());
    egret_workflow::worker::handlers::storage::register(&mut r, pool.clone(), storage::current());
    nst::register(&mut r, pool.clone(), storage::current());
    r
}

/// Tick until nothing is claimable; a worker error fails the test HERE, by
/// assertion (a handler `Err` is how an infrastructure failure shows).
async fn drain_with(pool: &sqlx::PgPool, reg: &Handlers) {
    for _ in 0..512 {
        let ticked = tick(pool, 16, reg).await;
        assert!(ticked.is_ok(), "a worker error: {ticked:?}");
        if ticked.unwrap() == 0 {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

async fn drain(pool: &sqlx::PgPool) {
    drain_with(pool, &registry(pool)).await;
}

async fn run(db: &Db, ws: Uuid, source: &str) -> Uuid {
    let ex = start_execution(&db.pool, ws, None, source, json!({}))
        .await
        .unwrap();
    drain(&db.pool).await;
    ex
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

async fn succeeded(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Value {
    let (state, outputs, message) = result_of(pool, ex, task).await;
    assert_eq!(state, "SUCCESS", "{task}: {message:?}");
    outputs.unwrap_or(json!({}))
}

/// The message of a run in `state`.
async fn message_of(pool: &sqlx::PgPool, ex: Uuid, task: &str, state: &str) -> String {
    let (got, outputs, message) = result_of(pool, ex, task).await;
    assert_eq!(got, state, "{task}: {outputs:?} {message:?}");
    message.unwrap_or_else(|| panic!("{task} has no message"))
}

/// Every file row of `ns` in `ws`, path -> revisions (live or not).
async fn rows(pool: &sqlx::PgPool, ws: Uuid, ns: &str) -> BTreeMap<String, Vec<i32>> {
    let all: Vec<(String, i32)> = sqlx::query_as(
        "SELECT path, revision FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND NOT is_directory \
          ORDER BY path, revision",
    )
    .bind(ws)
    .bind(ns)
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for (path, rev) in all {
        out.entry(path).or_default().push(rev);
    }
    out
}

/// The bytes of revision `rev` of `path` in `ns`.
async fn ns_bytes(fs: &FileStore, ws: Uuid, ns: &str, path: &str, rev: u32) -> Vec<u8> {
    let grant = authorize(ws, Actor::Api, ns, Access::Read).unwrap();
    let key = fs.namespace_object_uri(&grant, &NsPath::parse_file(path).unwrap(), rev);
    fs.store.get(&key).await.unwrap()
}

/// Which execution wrote revision `rev` of `path`.
async fn writer_of(pool: &sqlx::PgPool, ws: Uuid, ns: &str, path: &str, rev: i32) -> Uuid {
    sqlx::query_scalar(
        "SELECT created_by_execution FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND path = $3 AND revision = $4",
    )
    .bind(ws)
    .bind(ns)
    .bind(path)
    .bind(rev)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One revision of `path` in `ns`, as an API upload writes it (the
/// fixture's premise: "seeded with store::put").
async fn seed(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, path: &str, bytes: &[u8]) {
    let grant = authorize(ws, Actor::Api, ns, Access::Write).unwrap();
    let mut body: &[u8] = bytes;
    match put(
        &db.pool,
        fs,
        &grant,
        &NsPath::parse_file(path).unwrap(),
        &mut body,
        Conflict::Overwrite,
        CreatedBy::User(Uuid::nil()),
    )
    .await
    {
        Ok(Written::Written(_)) => {}
        other => panic!("seed {ns} {path}: {other:?}"),
    }
}

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/oracle-exec/w9b-namespace-files.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    })
}

fn cell(class: &str) -> &'static Value {
    &fixture()["egret_expected"]["tasks"][class]
}

fn raw_flow(id: &str) -> String {
    fixture()["raw_records"]["flows"][id]["yaml"]
        .as_str()
        .unwrap_or_else(|| panic!("no raw flow {id}"))
        .to_string()
}

/// `text` without the lines from the one starting `from` up to (not
/// including) the one starting `to` (or the end), and what was cut.
fn cut(text: &str, from: &str, to: Option<&str>) -> (String, String) {
    let start = text.find(from).unwrap_or_else(|| panic!("no {from:?}"));
    let end = match to {
        Some(to) => {
            start
                + text[start..]
                    .find(to)
                    .unwrap_or_else(|| panic!("no {to:?}"))
        }
        None => text.len(),
    };
    (
        format!("{}{}", &text[..start], &text[end..]),
        text[start..end].to_string(),
    )
}

/// The fixture's upload flow, as `tasks.UploadFiles.premise` edits it:
/// `absent.txt` removed, `wd` removed, `up_other_ns` moved last.
fn upload_variant() -> String {
    let yaml = raw_flow("w9b.tasks/upload");
    let yaml: String = yaml
        .lines()
        .filter(|l| !l.contains("absent.txt:"))
        .map(|l| format!("{l}\n"))
        .collect();
    let (yaml, _) = cut(&yaml, "  - id: wd\n", Some("  - id: up_conflict_error\n"));
    let (yaml, other) = cut(&yaml, "  - id: up_other_ns\n", Some("  - id: up_nothing\n"));
    format!("{yaml}{other}")
}

/// One task of the fixture's upload flow, as written.
fn upload_task(id: &str) -> String {
    let yaml = raw_flow("w9b.tasks/upload");
    let start = format!("  - id: {id}\n");
    let from = yaml.find(&start).unwrap_or_else(|| panic!("no task {id}"));
    let rest = &yaml[from + start.len()..];
    let end = rest.find("\n  - id: ").map_or(rest.len(), |e| e + 1);
    format!("{start}{}", &rest[..end])
}

fn flow(id: &str, ns: &str, tasks: &[String]) -> String {
    let body: String = tasks
        .iter()
        .map(|t| {
            if t.ends_with('\n') {
                t.clone()
            } else {
                format!("{t}\n")
            }
        })
        .collect();
    format!("id: {id}\nnamespace: {ns}\ntasks:\n{body}")
}

/// The fixture's `w` (Write, "map content", `.txt`).
fn write_task() -> String {
    upload_task("w")
}

/// Mask an execution URI as the fixture's `compare` says: the execution id,
/// the task-run id and (with `name`) the generated file name.
fn masked(file: &str, name: bool) -> String {
    let mut segs: Vec<String> = file.split('/').map(str::to_string).collect();
    let i = segs
        .iter()
        .position(|s| s == "executions")
        .unwrap_or_else(|| panic!("{file}: no executions segment"));
    segs[i + 1] = "<exec>".into();
    if segs.get(i + 2).map(String::as_str) == Some("tasks") {
        segs[i + 4] = "<taskrun>".into();
    }
    if name {
        let last = segs.last_mut().unwrap();
        let ext = last
            .rfind('.')
            .map_or(String::new(), |d| last[d..].to_string());
        *last = format!("<name>{ext}");
    }
    segs.join("/")
}

fn masked_values(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), masked_values(v)))
                .collect(),
        ),
        Value::String(s) if s.starts_with("kestra:///") => Value::String(masked(s, false)),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// the shapes
// ---------------------------------------------------------------------------

/// DECLARED equals each fixture schema cell's TOP-LEVEL `required`/`default`
/// (a chosen cell's `kestra` is never read); `UploadFiles.namespace` is
/// optional (ruling D), so publish accepts an `UploadFiles` without one
/// (`tasks.UploadFiles.publish_without_namespace`).
#[tokio::test(flavor = "multi_thread")]
async fn declared_properties_and_outputs_match_the_artifact() {
    assert_eq!(nst::DECLARED.len(), 4);
    for (type_id, props, outputs) in nst::DECLARED {
        let class = type_id.rsplit('.').next().unwrap();
        let schema = &cell(class)["schema"];
        let theirs = schema["properties"].as_object().unwrap();
        let names: Vec<&str> = props.iter().map(|p| p.0).collect();
        let their_names: Vec<&str> = theirs.keys().map(String::as_str).collect();
        assert_eq!(names, their_names, "{class}: property names");
        for (name, required, default) in *props {
            let p = &theirs[*name];
            assert_eq!(p["required"], json!(required), "{class}.{name} required");
            assert_eq!(p["default"].as_str(), *default, "{class}.{name} default");
        }
        let outs: Vec<&str> = schema["outputs"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(*outputs, outs.as_slice(), "{class}: outputs");
    }
    for class in ["UploadFiles", "DeleteFiles"] {
        assert_eq!(
            cell(class)["schema"]["properties"]["namespace"]["kestra"]["required"],
            json!(true),
            "the control: Kestra's own answer differs for {class}, and DECLARED follows \
             the top level"
        );
    }
    // The four classes, each once.
    let classes: BTreeSet<&str> = nst::DECLARED.iter().map(|c| c.0).collect();
    assert_eq!(
        classes,
        BTreeSet::from([
            nst::UPLOAD_FILES,
            nst::DOWNLOAD_FILES,
            nst::DELETE_FILES,
            nst::PURGE_FILES
        ])
    );
    // PurgeFiles' default behavior is the `purge` cell's.
    assert_eq!(
        cell("PurgeFiles")["schema"]["properties"]["behavior"]["default"],
        fixture()["egret_expected"]["purge"]["default_behavior"]["schema_default"]
    );

    let _turn = SERIAL.lock().await;
    let Some(db) = common::db().await else {
        return;
    };
    let (ws, user) = common::seed_workspace(&db).await;
    let want = &cell("UploadFiles")["publish_without_namespace"];
    assert_eq!(want["accepted"], json!(true));
    let source = raw_flow("w9b.tasks/upload-nons");
    assert!(
        !source.contains("namespace: w9b.tasks\n    "),
        "no task namespace"
    );
    let id = common::seed_flow(&db, ws, user, &source, "draft").await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let published =
        egret_workflow::publish(&repo, &common::workflow_refs(&db), ws, user, id, "").await;
    assert!(published.is_ok(), "{published:?}");
}

// ---------------------------------------------------------------------------
// UploadFiles
// ---------------------------------------------------------------------------

/// `Write` -> `UploadFiles` `files: ["{{ outputs.w.uri }}", "**.txt"]`,
/// `destination: /in/`: the URI and the glob name the same file, written
/// once, at `/in/` + its name, with Write's bytes, by this execution; the
/// output is `{}` (Kestra's run returns an empty Output).
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_from_a_write_uri_and_a_glob_lands_as_the_artifact_lays_it_out() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              files: [\"{{ outputs.w.uri }}\", \"**.txt\"]\n    destination: /in/\n";
    let ex = run(&db, ws, &flow("up-glob", NS, &[write_task(), up.into()])).await;
    let w = succeeded(&db.pool, ex, "w").await["uri"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(w.ends_with("/content.txt"), "{w}");
    assert_eq!(succeeded(&db.pool, ex, "up").await, json!({}));
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::from([("/in/content.txt".to_string(), vec![1])])
    );
    assert_eq!(
        ns_bytes(&fs, ws, NS, "/in/content.txt", 1).await,
        b"map content"
    );
    assert_eq!(writer_of(&db.pool, ws, NS, "/in/content.txt", 1).await, ex);
    // The parent directory has its row (spec §4.2).
    let dir: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.namespace_files \
          WHERE workspace_id = $1 AND namespace = $2 AND path = '/in/' AND is_directory",
    )
    .bind(ws)
    .bind(NS)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(dir, 1);
}

/// `filesMap` values: a `kestra://` URI (its bytes), or anything else (the
/// file's contents, rendered); and the map as a string holding JSON.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_map_takes_a_uri_or_literal_contents() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              filesMap:\n      a.txt: \"{{ outputs.w.uri }}\"\n      lit/b.txt: \"hello {{ 1 + 1 }}\"\n    \
              destination: /m\n";
    let up_string =
        "  - id: up_string\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
                     filesMap: '{\"c.txt\": \"contents c\"}'\n";
    let ex = run(
        &db,
        ws,
        &flow("up-map", NS, &[write_task(), up.into(), up_string.into()]),
    )
    .await;
    assert_eq!(succeeded(&db.pool, ex, "up").await, json!({}));
    assert_eq!(succeeded(&db.pool, ex, "up_string").await, json!({}));
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::from([
            ("/c.txt".to_string(), vec![1]),
            ("/m/a.txt".to_string(), vec![1]),
            ("/m/lit/b.txt".to_string(), vec![1]),
        ])
    );
    assert_eq!(ns_bytes(&fs, ws, NS, "/m/a.txt", 1).await, b"map content");
    assert_eq!(ns_bytes(&fs, ws, NS, "/m/lit/b.txt", 1).await, b"hello 2");
    assert_eq!(ns_bytes(&fs, ws, NS, "/c.txt", 1).await, b"contents c");
}

/// `tasks.UploadFiles.runs.up_map` as recorded (chosen; Kestra skips the
/// missing URI silently): the task fails naming the URI, and the other
/// entries are not written either.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_with_a_uri_that_does_not_resolve_fails_naming_it_and_writes_nothing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let want = &cell("UploadFiles")["runs"]["up_map"];
    let ex = run(
        &db,
        ws,
        &flow("upload", NS, &[write_task(), upload_task("up_map")]),
    )
    .await;
    let message = message_of(&db.pool, ex, "up_map", want["state"].as_str().unwrap()).await;
    let absent = want["error_names"].as_str().unwrap();
    assert!(message.contains(absent), "{message}");
    assert_eq!(want["written"], json!([]));
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::new(),
        "nothing written"
    );
}

/// `runs.up_map_dest_no_slash` (chosen): `up2` + `m.txt` -> `/up2/m.txt`
/// (Kestra: `/up2m.txt`); `/up/` + `m.txt` -> `/up/m.txt`; the default `/`
/// + `rootm.txt` -> `/rootm.txt` (`runs.up_map_default_dest`).
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_destination_is_a_directory_joined_with_one_slash() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let up_slash = "  - id: up_slash\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
                    filesMap:\n      m.txt: \"{{ outputs.w.uri }}\"\n    destination: /up/\n";
    let ex = run(
        &db,
        ws,
        &flow(
            "upload",
            NS,
            &[
                write_task(),
                upload_task("up_map_dest_no_slash"),
                upload_task("up_map_default_dest"),
                up_slash.into(),
            ],
        ),
    )
    .await;
    let runs = &cell("UploadFiles")["runs"];
    let mut want = BTreeMap::new();
    for t in ["up_map_dest_no_slash", "up_map_default_dest"] {
        assert_eq!(runs[t]["state"], "SUCCESS");
        assert_eq!(succeeded(&db.pool, ex, t).await, runs[t]["outputs"], "{t}");
        for p in runs[t]["written"].as_array().unwrap() {
            want.insert(p.as_str().unwrap().to_string(), vec![1]);
        }
    }
    succeeded(&db.pool, ex, "up_slash").await;
    want.insert("/up/m.txt".to_string(), vec![1]);
    assert_eq!(rows(&db.pool, ws, NS).await, want);
    assert!(
        !want.contains_key("/up2m.txt"),
        "Kestra's join is not egret's"
    );
}

/// `runs.up_other_ns` (chosen): a flow writes only its own namespace; the
/// refusal names the namespace, and nothing is written anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_to_another_namespace_is_refused_naming_it_and_writes_nothing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let want = &cell("UploadFiles")["runs"]["up_other_ns"];
    let ex = run(
        &db,
        ws,
        &flow("upload", NS, &[write_task(), upload_task("up_other_ns")]),
    )
    .await;
    let message = message_of(&db.pool, ex, "up_other_ns", want["state"].as_str().unwrap()).await;
    let other = want["error_names"].as_str().unwrap();
    assert!(
        message.contains(&format!("'{other}'")) && message.contains("writes only its own"),
        "{message}"
    );
    assert_eq!(rows(&db.pool, ws, other).await, BTreeMap::new());
    assert_eq!(rows(&db.pool, ws, NS).await, BTreeMap::new());
}

/// The upload-flow variant of `tasks.UploadFiles.premise`, nothing seeded:
/// every run's state as recorded, errors by the path or property they name,
/// `w`'s URI masked, and `rows_after` row for row.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_conflict_error_and_skip() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let up = cell("UploadFiles");
    let runs = &up["runs"];
    let source = upload_variant();
    assert!(!source.contains("absent.txt") && !source.contains("id: wd"));
    assert!(source
        .trim_end()
        .ends_with("e.txt: \"{{ outputs.w.uri }}\""));
    let ex = run(&db, ws, &source).await;

    let w = succeeded(&db.pool, ex, "w").await;
    let ours = egret_workflow::storage::uri::parse(w["uri"].as_str().unwrap()).unwrap();
    assert_eq!(ours.execution, ex);
    assert_eq!(
        masked(w["uri"].as_str().unwrap(), true),
        masked(runs["w"]["outputs"]["uri"].as_str().unwrap(), true),
        "{}",
        runs["w"]["compare"]
    );
    for t in [
        "up_map",
        "up_map_dest_no_slash",
        "up_map_default_dest",
        "up_conflict_skip",
    ] {
        assert_eq!(succeeded(&db.pool, ex, t).await, json!({}), "{t}");
    }
    let state = |t: &str| runs[t]["state"].as_str().unwrap().to_string();
    let error = message_of(
        &db.pool,
        ex,
        "up_conflict_error",
        &state("up_conflict_error"),
    )
    .await;
    assert!(
        error.contains("'/up/m.txt'") && error.contains("ERROR"),
        "{error}"
    );
    let nothing = message_of(&db.pool, ex, "up_nothing", &state("up_nothing")).await;
    assert!(
        nothing.contains("files or filesMap is required")
            && nothing.contains("`files`")
            && nothing.contains("`filesMap`"),
        "{nothing}"
    );
    let other = message_of(&db.pool, ex, "up_other_ns", &state("up_other_ns")).await;
    assert!(other.contains("'w9b.elsewhere'"), "{other}");

    let after = &up["rows_after"];
    let mut want = BTreeMap::new();
    for (path, revs) in after.as_object().unwrap() {
        if path.starts_with('/') && revs.is_array() {
            let revs = revs
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_i64().unwrap() as i32)
                .collect();
            want.insert(path.clone(), revs);
        }
    }
    assert_eq!(want.len(), 4, "the four comparable rows");
    assert_eq!(after["/up2m.txt"], json!(404));
    assert_eq!(rows(&db.pool, ws, NS).await, want);
    let search: BTreeSet<String> = after["search ?q=.txt"]["set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let live: BTreeSet<String> = rows(&db.pool, ws, NS)
        .await
        .into_keys()
        .filter(|p| p.contains(".txt"))
        .collect();
    assert_eq!(live, search);
    assert_eq!(after["w9b.elsewhere"], json!([]));
    assert_eq!(rows(&db.pool, ws, "w9b.elsewhere").await, BTreeMap::new());
}

/// An unrelated execution in the same workspace holds a `.txt` file the glob
/// would match; `**.txt` never reaches it (the readable set).
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_glob_never_reaches_an_unreadable_execution() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let unrelated = "  - id: w\n    type: io.kestra.plugin.core.storage.Write\n    \
                     content: secret\n    extension: \".unrelated.txt\"\n";
    let other = run(&db, ws, &flow("unrelated", NS, &[unrelated.into()])).await;
    let theirs = succeeded(&db.pool, other, "w").await["uri"]
        .as_str()
        .unwrap()
        .to_string();
    let indexed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
    )
    .bind(ws)
    .bind(&theirs)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        indexed, 1,
        "the control: the unrelated file is indexed in this workspace"
    );

    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              files: [\"**.txt\"]\n    destination: /g\n";
    let ex = run(&db, ws, &flow("up-glob", NS, &[write_task(), up.into()])).await;
    succeeded(&db.pool, ex, "up").await;
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::from([("/g/content.txt".to_string(), vec![1])]),
        "only the execution's own file; never content.unrelated.txt"
    );
}

/// More readable files than `max_namespace_files`: the glob is refused
/// naming the cap and nothing is written; the listing fetches cap + 1 rows.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_glob_past_the_cap_is_refused_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let n = CAP as usize + 1;
    let mut tasks: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "  - id: w{i}\n    type: io.kestra.plugin.core.storage.Write\n    \
                 content: c{i}\n    extension: \".txt\"\n"
            )
        })
        .collect();
    tasks.push(
        "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
         files: [\"**.txt\"]\n    destination: /g\n"
            .into(),
    );
    let ex = run(&db, ws, &flow("up-cap", NS, &tasks)).await;
    let message = message_of(&db.pool, ex, "up", "FAILED").await;
    assert!(
        message.contains("workflow.storage.max_namespace_files")
            && message.contains(&format!("more than {CAP}")),
        "{message}"
    );
    assert_eq!(rows(&db.pool, ws, NS).await, BTreeMap::new());

    // The listing itself: at most cap + 1 rows, the `+1` the refusal.
    let mut conn = db.pool.acquire().await.unwrap();
    let readable = storage::readable_executions(&mut conn, ws, ex)
        .await
        .unwrap();
    for (cap, fetched) in [(2u64, 3usize), (CAP, n), (n as u64, n)] {
        let got = nst::readable_execution_files(&mut conn, ws, &readable, cap)
            .await
            .unwrap();
        assert_eq!(got.len(), fetched, "cap {cap}");
    }
}

// ---------------------------------------------------------------------------
// DownloadFiles
// ---------------------------------------------------------------------------

/// `tasks.DownloadFiles.premise`: `w9b.tasks` as Kestra's upload flow left it.
async fn seed_download_premise(db: &Db, fs: &FileStore, ws: Uuid) {
    for (path, bytes) in [
        ("/up/m.txt", "map content"),
        ("/up/sub/n.txt", "map content"),
        ("/rootm.txt", "map content"),
        ("/up2m.txt", "map content"),
        ("/globbed/top.txt", "top"),
        ("/globbed/g/one.txt", "g1"),
    ] {
        seed(db, fs, ws, NS, path, bytes.as_bytes()).await;
    }
}

fn download_task(id: &str) -> String {
    task_of("w9b.tasks/download", id)
}

/// Task `id` of the fixture's raw flow `flow`, as written.
fn task_of(flow: &str, id: &str) -> String {
    let yaml = raw_flow(flow);
    let start = format!("  - id: {id}\n");
    let from = yaml.find(&start).unwrap_or_else(|| panic!("no task {id}"));
    let rest = &yaml[from + start.len()..];
    let end = rest.find("\n  - id: ").map_or(rest.len(), |e| e + 1);
    format!("{start}{}", &rest[..end])
}

/// `runs.dl`, `dl_dest`, `dl_string`, `dl_none` and `read_back`: outputs
/// path -> URI under the run's prefix, masked as `compare` says; each file
/// is indexed with its size once the outcome commits, and `read()` of it
/// gives the namespace file's bytes.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_copies_into_the_task_prefix_and_indexes_at_commit() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_download_premise(&db, &fs, ws).await;
    let ids = ["dl", "dl_dest", "dl_string", "dl_none", "read_back"];
    let tasks: Vec<String> = ids.iter().map(|t| download_task(t)).collect();
    let ex = run(&db, ws, &flow("download", NS, &tasks)).await;
    let runs = &cell("DownloadFiles")["runs"];
    for t in &ids[..4] {
        let ours = succeeded(&db.pool, ex, t).await;
        assert_eq!(runs[t]["state"], "SUCCESS");
        assert_eq!(
            masked_values(&ours),
            masked_values(&runs[t]["outputs"]),
            "{t}"
        );
        let run_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(ex)
        .bind(t)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        for (path, file) in ours["files"].as_object().unwrap() {
            let file = file.as_str().unwrap();
            let (rows, size, by_run): (i64, Option<i64>, Option<Uuid>) = sqlx::query_as(
                "SELECT count(*), max(size), max(task_run_id::text)::uuid \
                   FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
            )
            .bind(ws)
            .bind(file)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            let bytes = ns_bytes(&fs, ws, NS, path, 1).await;
            assert_eq!(rows, 1, "{t} {file}: indexed");
            assert_eq!(size, Some(bytes.len() as i64), "{t} {file}");
            assert_eq!(by_run, Some(run_id), "{t} {file}: this run's file");
            let object = fs.object_uri(ws, file).unwrap();
            assert_eq!(fs.store.get(&object).await.unwrap(), bytes, "{t} {file}");
        }
    }
    let back = succeeded(&db.pool, ex, "read_back").await;
    let value = back["value"].as_str().unwrap();
    let (json_part, text) = value.split_once(" :: ").unwrap();
    let theirs = runs["read_back"]["outputs"]["value"].as_str().unwrap();
    let (their_json, their_text) = theirs.split_once(" :: ").unwrap();
    assert_eq!(text, their_text);
    assert_eq!(
        masked_values(&serde_json::from_str(json_part).unwrap()),
        masked_values(&serde_json::from_str(their_json).unwrap())
    );
}

/// `runs.dl_other`: `w9b.s1`, another namespace of the workspace, is readable.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_reads_another_namespace_of_the_workspace() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, "w9b.s1", "/x.txt", b"x").await;
    seed(&db, &fs, ws, "w9b.s1", "/only1.txt", b"only1").await;
    let ex = run(&db, ws, &flow("download", NS, &[download_task("dl_other")])).await;
    let want = &cell("DownloadFiles")["runs"]["dl_other"];
    let ours = succeeded(&db.pool, ex, "dl_other").await;
    assert_eq!(masked_values(&ours), masked_values(&want["outputs"]));
    let file = ours["files"]["/only1.txt"].as_str().unwrap();
    assert_eq!(
        fs.store
            .get(&fs.object_uri(ws, file).unwrap())
            .await
            .unwrap(),
        b"only1"
    );
}

/// A namespace with a `_files` segment is refused by `check_namespace`
/// (through `authorize`), naming it; nothing is downloaded.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_of_a_files_segment_namespace_is_refused_by_name() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let dl = "  - id: dl\n    type: io.kestra.plugin.core.namespace.DownloadFiles\n    \
              namespace: w9b._files\n    files: [\"**\"]\n";
    let ex = run(&db, ws, &flow("download", NS, &[dl.into()])).await;
    let message = message_of(&db.pool, ex, "dl", "FAILED").await;
    assert!(
        message.contains("'w9b._files'") && message.contains("segment '_files'"),
        "{message}"
    );
    let indexed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE execution_id = $1")
            .bind(ex)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(indexed, 0);
}

// ---------------------------------------------------------------------------
// failures that are not the task's
// ---------------------------------------------------------------------------

/// The attempts `task` of `ex` has spent.
async fn attempts(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Tick (one claim at a time) until `task` of `ex` exists and nothing else
/// is claimable before it.
async fn tick_until_claimable(pool: &sqlx::PgPool, ex: Uuid, task: &str) {
    let reg = registry(pool);
    for _ in 0..64 {
        let exists: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(ex)
        .bind(task)
        .fetch_one(pool)
        .await
        .unwrap();
        if exists == 1 {
            return;
        }
        let ticked = tick(pool, 1, &reg).await;
        assert!(ticked.is_ok(), "{ticked:?}");
    }
    panic!("{task} never became claimable");
}

/// An unreachable store while either task streams is a worker error: the
/// run stays RUNNING with no attempt spent, and once the store is back the
/// re-claimed run succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_outage_in_either_task_is_a_worker_error_not_a_failed_task() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, NS, "/up/m.txt", b"map content").await;
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              filesMap:\n      a.txt: \"{{ outputs.w.uri }}\"\n";
    let source = flow(
        "outage",
        NS,
        &[write_task(), up.into(), download_task("dl_string")],
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    let reg = registry(&db.pool);
    for task in ["up", "dl_string"] {
        tick_until_claimable(&db.pool, ex, task).await;
        OUTAGE.store(true, Ordering::SeqCst);
        let err = tick(&db.pool, 1, &reg).await;
        OUTAGE.store(false, Ordering::SeqCst);
        let err = err.expect_err("an outage is a worker error");
        assert!(
            err.to_string().contains("internal storage is unavailable"),
            "{task}: {err}"
        );
        assert!(!err.to_string().contains("s3://"), "{task}: {err}");
        let (state, outputs, _) = result_of(&db.pool, ex, task).await;
        assert_eq!((state.as_str(), outputs), ("RUNNING", None), "{task}");
        assert_eq!(
            attempts(&db.pool, ex, task).await,
            1,
            "{task}: no attempt spent"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            reap(&db.pool, std::time::Duration::ZERO).await.unwrap(),
            1,
            "{task}"
        );
        let ticked = tick(&db.pool, 1, &reg).await;
        assert!(ticked.is_ok(), "{task}: {ticked:?}");
        succeeded(&db.pool, ex, task).await;
    }
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::from([
            ("/a.txt".to_string(), vec![1]),
            ("/up/m.txt".to_string(), vec![1]),
        ])
    );
}

/// With no `[storage]`, both tasks fail naming it.
#[tokio::test(flavor = "multi_thread")]
async fn without_storage_both_fail_naming_storage() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let mut reg = Handlers::with_core_tasks(storage::current());
    reg.add_publishable(Arc::new(UploadFiles::new(db.pool.clone(), None)));
    reg.add_publishable(Arc::new(DownloadFiles::new(db.pool.clone(), None)));
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              allowFailure: true\n    filesMap:\n      a.txt: x\n";
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &flow("nostore", NS, &[up.into(), download_task("dl")]),
        json!({}),
    )
    .await
    .unwrap();
    drain_with(&db.pool, &reg).await;
    for (task, state) in [("up", "WARNING"), ("dl", "FAILED")] {
        let message = message_of(&db.pool, ex, task, state).await;
        assert!(message.contains("`[storage]`"), "{task}: {message}");
    }
    assert_eq!(rows(&db.pool, ws, NS).await, BTreeMap::new());
}

/// `uri` is used for the masked compare's parse; this keeps the import honest.
#[test]
fn masking_hides_only_the_ids_and_the_generated_name() {
    let k = "kestra:///w9b/tasks/upload/executions/4N1/tasks/w/5rx/1307.txt";
    assert_eq!(
        masked(k, true),
        "kestra:///w9b/tasks/upload/executions/<exec>/tasks/w/<taskrun>/<name>.txt"
    );
    assert_eq!(
        masked(k, false),
        "kestra:///w9b/tasks/upload/executions/<exec>/tasks/w/<taskrun>/1307.txt"
    );
    assert!(uri::parse(k).is_err(), "Kestra's ids are not egret's UUIDs");
}

// ---------------------------------------------------------------------------
// fix round 1 (2026-10-06)
// ---------------------------------------------------------------------------

/// The task run of `task` in `ex`.
async fn run_id_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(ex)
        .bind(task)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I1: `a` commits, the store fails while `b` uploads (an infrastructure
/// error: no attempt spent), and the re-claimed run ends SUCCESS with exactly
/// one revision of each -- under ERROR (no conflict with its own write) and
/// under OVERWRITE (no second revision). The fault fails only that one put.
#[tokio::test(flavor = "multi_thread")]
async fn a_reclaim_after_a_partial_upload_skips_what_its_earlier_attempt_wrote() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let reg = registry(&db.pool);
    for mode in ["ERROR", "OVERWRITE"] {
        let dir = format!("/{}", mode.to_lowercase());
        let up = format!(
            "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
             conflict: {mode}\n    destination: {dir}\n    filesMap:\n      \
             a.txt: \"{{{{ outputs.w.uri }}}}\"\n      b.txt: \"contents b\"\n"
        );
        let ex = start_execution(
            &db.pool,
            ws,
            None,
            &flow("partial", NS, &[write_task(), up]),
            json!({}),
        )
        .await
        .unwrap();
        tick_until_claimable(&db.pool, ex, "up").await;
        *FAIL_PUT_ONCE.lock().unwrap() = Some(format!("/_files{dir}/b.txt"));
        let err = tick(&db.pool, 1, &reg).await;
        assert!(
            FAIL_PUT_ONCE.lock().unwrap().is_none(),
            "{mode}: the fault fired"
        );
        let err = err.expect_err("a store failure is a worker error");
        assert!(
            err.to_string().contains("internal storage is unavailable"),
            "{mode}: {err}"
        );
        let a = format!("{dir}/a.txt");
        let b = format!("{dir}/b.txt");
        assert_eq!(
            rows(&db.pool, ws, NS).await,
            BTreeMap::from([(a.clone(), vec![1])]),
            "{mode}: a committed, b did not"
        );
        let (state, _, _) = result_of(&db.pool, ex, "up").await;
        assert_eq!(state, "RUNNING", "{mode}");
        assert_eq!(attempts(&db.pool, ex, "up").await, 1, "{mode}");
        let run: Option<Uuid> = sqlx::query_scalar(
            "SELECT created_by_task_run FROM workflow.namespace_files \
              WHERE workspace_id = $1 AND namespace = $2 AND path = $3",
        )
        .bind(ws)
        .bind(NS)
        .bind(&a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(run, Some(run_id_of(&db.pool, ex, "up").await), "{mode}");

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
        let ticked = tick(&db.pool, 1, &reg).await;
        assert!(ticked.is_ok(), "{mode}: {ticked:?}");
        let (state, _, message) = result_of(&db.pool, ex, "up").await;
        assert_eq!(state, "SUCCESS", "{mode}: {message:?}");
        assert_eq!(
            rows(&db.pool, ws, NS).await,
            BTreeMap::from([(a.clone(), vec![1]), (b.clone(), vec![1])]),
            "{mode}: one revision each"
        );
        assert_eq!(ns_bytes(&fs, ws, NS, &b, 1).await, b"contents b");
        // Clean the namespace for the next mode.
        sqlx::query("DELETE FROM workflow.namespace_files WHERE workspace_id = $1")
            .bind(ws)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// M-R1 (re-review 1): a source whose rendered length differs between the
/// first attempt and the re-claim (`read('/ctl.txt')`, rewritten in between,
/// stands in for `{{ now() }}`) is still skipped on the re-claim: under ERROR
/// the task ends SUCCESS with one revision, and attempt 1's bytes are kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_reclaim_keeps_the_first_attempts_bytes_when_the_source_renders_longer() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let reg = registry(&db.pool);
    seed(&db, &fs, ws, NS, "/ctl.txt", b"first").await;
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              conflict: ERROR\n    destination: /rerender\n    filesMap:\n      \
              a.txt: \"{{ read('/ctl.txt') }}\"\n      b.txt: \"contents b\"\n"
        .to_string();
    let ex = start_execution(&db.pool, ws, None, &flow("rerender", NS, &[up]), json!({}))
        .await
        .unwrap();
    tick_until_claimable(&db.pool, ex, "up").await;
    *FAIL_PUT_ONCE.lock().unwrap() = Some("/_files/rerender/b.txt".to_string());
    assert!(tick(&db.pool, 1, &registry(&db.pool)).await.is_err());
    assert!(FAIL_PUT_ONCE.lock().unwrap().is_none(), "the fault fired");
    // The source now renders longer than attempt 1's 5 bytes.
    seed(&db, &fs, ws, NS, "/ctl.txt", b"a much longer second render").await;

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(reap(&db.pool, std::time::Duration::ZERO).await.unwrap(), 1);
    let ticked = tick(&db.pool, 1, &reg).await;
    assert!(ticked.is_ok(), "{ticked:?}");
    let (state, _, message) = result_of(&db.pool, ex, "up").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let all = rows(&db.pool, ws, NS).await;
    assert_eq!(all["/rerender/a.txt"], vec![1], "one revision of a.txt");
    assert_eq!(all["/rerender/b.txt"], vec![1]);
    assert_eq!(
        ns_bytes(&fs, ws, NS, "/rerender/a.txt", 1).await,
        b"first",
        "attempt 1's bytes are kept"
    );
}

/// I2 (the coordinator's ruling, 2026-10-06, R3): two Writes name their
/// files `content.txt`, so `**.txt` plans two DIFFERENT sources for
/// `/o/content.txt`; the task is refused before anything is written, naming
/// the path and both sources.
#[tokio::test(flavor = "multi_thread")]
async fn two_different_sources_for_one_path_are_refused_before_anything_is_written() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let w = |id: &str, content: &str| {
        format!(
            "  - id: {id}\n    type: io.kestra.plugin.core.storage.Write\n    \
             content: {content}\n    extension: \".txt\"\n"
        )
    };
    for mode in ["ERROR", "OVERWRITE"] {
        let up = format!(
            "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
             conflict: {mode}\n    files: [\"**.txt\"]\n    destination: /o\n"
        );
        let ex = run(
            &db,
            ws,
            &flow("collide", NS, &[w("zz", "first"), w("aa", "second"), up]),
        )
        .await;
        let zz = succeeded(&db.pool, ex, "zz").await["uri"]
            .as_str()
            .unwrap()
            .to_string();
        let aa = succeeded(&db.pool, ex, "aa").await["uri"]
            .as_str()
            .unwrap()
            .to_string();
        let message = message_of(&db.pool, ex, "up", "FAILED").await;
        assert!(
            message.contains("'/o/content.txt'")
                && message.contains(&zz)
                && message.contains(&aa)
                && message.contains("two different sources"),
            "{mode}: {message}"
        );
        assert_eq!(rows(&db.pool, ws, NS).await, BTreeMap::new(), "{mode}");
    }
}

/// M2: a namespace-file source -- `nsfile:///…`, `nsfile://other.ns/…` in
/// `files` (kept at its namespace path), a `kestra:///…/_files/…` URI in
/// `filesMap` (at its key) -- is read through `authorize` (Read) and copied
/// into the flow's own namespace; one that names nothing, or a refused
/// namespace, fails the task naming it and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_copies_namespace_files_and_names_one_that_does_not_resolve() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, NS, "/src/s.txt", b"S").await;
    seed(&db, &fs, ws, "w9b.s1", "/x.txt", b"X").await;
    let up = "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
              files: [\"nsfile:///src/s.txt\", \"nsfile://w9b.s1/x.txt\"]\n    \
              filesMap:\n      k.txt: \"kestra:///w9b/tasks/_files/src/s.txt\"\n    \
              destination: /cp\n";
    let ex = run(&db, ws, &flow("copy", NS, &[up.into()])).await;
    assert_eq!(succeeded(&db.pool, ex, "up").await, json!({}));
    let rows_now = rows(&db.pool, ws, NS).await;
    for (path, bytes) in [
        ("/cp/src/s.txt", &b"S"[..]),
        ("/cp/x.txt", b"X"),
        ("/cp/k.txt", b"S"),
    ] {
        assert_eq!(rows_now.get(path), Some(&vec![1]), "{path}: {rows_now:?}");
        assert_eq!(ns_bytes(&fs, ws, NS, path, 1).await, bytes, "{path}");
    }
    for (id, item, names) in [
        ("missing", "nsfile:///nope.txt", "'/nope.txt'"),
        ("refused", "nsfile://w9b._files/x.txt", "'w9b._files'"),
    ] {
        let up = format!(
            "  - id: up\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
             files: [\"{item}\"]\n    filesMap:\n      first.txt: contents\n    destination: /{id}\n"
        );
        let ex = run(&db, ws, &flow(id, NS, &[up])).await;
        let message = message_of(&db.pool, ex, "up", "FAILED").await;
        assert!(
            message.contains(item) && message.contains(names),
            "{id}: {message}"
        );
        let written: Vec<String> = rows(&db.pool, ws, NS)
            .await
            .into_keys()
            .filter(|p| p.starts_with(&format!("/{id}/")))
            .collect();
        assert!(
            written.is_empty(),
            "{id}: nothing written, not even first.txt: {written:?}"
        );
    }
}

/// M3: the resolved sources are bounded by `max_namespace_files` and their
/// bytes by `max_task_bytes`, each refused naming the cap before any write.
#[tokio::test(flavor = "multi_thread")]
async fn upload_files_bounds_the_count_and_the_bytes_before_writing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let small = capped(storage::Caps {
        max_namespace_files: 2,
        max_task_bytes: 5,
        ..storage::Caps::default()
    });
    let reg = registry_over(&db.pool, small);
    let up = |id: &str, map: &str| {
        format!(
            "  - id: {id}\n    type: io.kestra.plugin.core.namespace.UploadFiles\n    \
             allowFailure: true\n    filesMap:\n{map}"
        )
    };
    let source = flow(
        "bounds",
        NS,
        &[
            up("count", "      a: x\n      b: y\n      c: z\n"),
            up("bytes", "      d: abc\n      e: def\n"),
            up("fits", "      f: abc\n      g: de\n"),
        ],
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain_with(&db.pool, &reg).await;
    let count = message_of(&db.pool, ex, "count", "WARNING").await;
    assert!(
        count.contains("workflow.storage.max_namespace_files") && count.contains("3 files"),
        "{count}"
    );
    let bytes = message_of(&db.pool, ex, "bytes", "WARNING").await;
    assert!(
        bytes.contains("workflow.storage.max_task_bytes") && bytes.contains("6 bytes"),
        "{bytes}"
    );
    succeeded(&db.pool, ex, "fits").await;
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        BTreeMap::from([("/f".to_string(), vec![1]), ("/g".to_string(), vec![1])]),
        "only the control wrote"
    );
}

/// I3 (a): matches whose rows hold more than `max_task_bytes` together are
/// refused naming the cap before any copy; the control under it succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_over_max_task_bytes_is_refused_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, NS, "/a.txt", b"abc").await;
    seed(&db, &fs, ws, NS, "/b.txt", b"def").await;
    let reg = registry_over(
        &db.pool,
        capped(storage::Caps {
            max_task_bytes: 5,
            ..storage::Caps::default()
        }),
    );
    let dl = |id: &str, files: &str| {
        format!(
            "  - id: {id}\n    type: io.kestra.plugin.core.namespace.DownloadFiles\n    \
             allowFailure: true\n    namespace: {NS}\n    files: {files}\n"
        )
    };
    let ex = start_execution(
        &db.pool,
        ws,
        None,
        &flow(
            "dl-bytes",
            NS,
            &[dl("over", "[\"/*.txt\"]"), dl("under", "\"/a.txt\"")],
        ),
        json!({}),
    )
    .await
    .unwrap();
    drain_with(&db.pool, &reg).await;
    let message = message_of(&db.pool, ex, "over", "WARNING").await;
    assert!(
        message.contains("workflow.storage.max_task_bytes") && message.contains("6 bytes"),
        "{message}"
    );
    let over = run_id_of(&db.pool, ex, "over").await;
    let indexed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE task_run_id = $1")
            .bind(over)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(indexed, 0);
    let under = succeeded(&db.pool, ex, "under").await;
    assert_eq!(under["files"].as_object().unwrap().len(), 1, "the control");
}

/// I3 (b): more namespace rows than `max_namespace_files` is a failed task
/// naming the cap (`list_live`'s `cap + 1`), not an error of the worker.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_past_the_listing_cap_is_refused_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, NS, "/a.txt", b"a").await;
    seed(&db, &fs, ws, NS, "/b.txt", b"b").await;
    let reg = registry_over(
        &db.pool,
        capped(storage::Caps {
            max_namespace_files: 1,
            ..storage::Caps::default()
        }),
    );
    let dl = format!(
        "  - id: dl\n    type: io.kestra.plugin.core.namespace.DownloadFiles\n    \
         namespace: {NS}\n    files: [\"/a.txt\"]\n"
    );
    let ex = start_execution(&db.pool, ws, None, &flow("dl-cap", NS, &[dl]), json!({}))
        .await
        .unwrap();
    drain_with(&db.pool, &reg).await;
    let message = message_of(&db.pool, ex, "dl", "FAILED").await;
    assert!(
        message.contains("workflow.storage.max_namespace_files") && message.contains("more than 1"),
        "{message}"
    );
}

/// I3 (c): an object whose size is not its row's is not copied: a failed
/// task naming the path and the revision.
#[tokio::test(flavor = "multi_thread")]
async fn download_files_of_an_object_whose_size_differs_from_its_row_fails_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed(&db, &fs, ws, NS, "/a.txt", b"abc").await;
    let grant = authorize(ws, Actor::Api, NS, Access::Read).unwrap();
    let key = fs.namespace_object_uri(&grant, &NsPath::parse_file("/a.txt").unwrap(), 1);
    fs.store.put(&key, b"abcdef".to_vec()).await.unwrap();
    let dl = format!(
        "  - id: dl\n    type: io.kestra.plugin.core.namespace.DownloadFiles\n    \
         namespace: {NS}\n    files: [\"/a.txt\"]\n"
    );
    let ex = run(&db, ws, &flow("dl-drift", NS, &[dl])).await;
    let message = message_of(&db.pool, ex, "dl", "FAILED").await;
    assert!(
        message.contains("revision 1 of namespace file '/a.txt'") && message.contains("(3)"),
        "{message}"
    );
}

/// I4: `destination: dl` (no trailing `/`) is a directory joined with one
/// `/` -- the chosen cell `tasks.DownloadFiles.runs.dl_dest_no_slash`
/// (Kestra concatenates: `dlup/m.txt`).
#[tokio::test(flavor = "multi_thread")]
async fn download_files_destination_is_a_directory_joined_with_one_slash() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_download_premise(&db, &fs, ws).await;
    let want = &cell("DownloadFiles")["runs"]["dl_dest_no_slash"];
    assert_eq!(want["basis"], "chosen");
    let task = &want["task"];
    let dl = format!(
        "  - id: {}\n    type: {}\n    namespace: {}\n    files: {}\n    destination: {}\n",
        task["id"].as_str().unwrap(),
        task["type"].as_str().unwrap(),
        task["namespace"].as_str().unwrap(),
        task["files"],
        task["destination"].as_str().unwrap(),
    );
    let ex = run(&db, ws, &flow("download", NS, &[dl])).await;
    let ours = succeeded(&db.pool, ex, task["id"].as_str().unwrap()).await;
    assert_eq!(masked_values(&ours), masked_values(&want["outputs"]));
    assert!(
        masked_values(&want["kestra"]["value"])
            .to_string()
            .contains("/dlup/m.txt"),
        "the reference: Kestra concatenates"
    );
}

// ---------------------------------------------------------------------------
// DeleteFiles (Task 7)
// ---------------------------------------------------------------------------

/// The live entries directly in `dir` of `ns` (`None` = the root), by name
/// as a listing shows it (`top.txt`, `sub`).
async fn listing(pool: &sqlx::PgPool, ws: Uuid, ns: &str, dir: Option<&str>) -> Vec<String> {
    let grant = authorize(ws, Actor::Api, ns, Access::Read).unwrap();
    let dir = dir.map(|d| NsPath::parse_dir(d).unwrap());
    let mut conn = pool.acquire().await.unwrap();
    let prefix = dir.as_ref().map_or("/", NsPath::as_str).to_string();
    list_live(&mut conn, &grant, dir.as_ref(), false, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| {
            e.path.as_str()[prefix.len()..]
                .trim_end_matches('/')
                .to_string()
        })
        .collect()
}

/// Whether `path` (a file, or a directory ending `/`) is live in `ns`.
async fn live(pool: &sqlx::PgPool, ws: Uuid, ns: &str, path: &str) -> bool {
    let grant = authorize(ws, Actor::Api, ns, Access::Read).unwrap();
    let p = if path.ends_with('/') {
        NsPath::parse_dir(path).unwrap()
    } else {
        NsPath::parse_file(path).unwrap()
    };
    let mut conn = pool.acquire().await.unwrap();
    match resolve(&mut conn, &grant, &p, Want::Latest).await {
        Ok(_) => true,
        Err(NsError::NotFound(_)) => false,
        Err(e) => panic!("{ns} {path}: {e:?}"),
    }
}

/// The objects under `ns`'s `_files` prefix in `ws`, relative to it, with
/// their sizes.
async fn objects(fs: &FileStore, ws: Uuid, ns: &str) -> BTreeMap<String, i64> {
    let grant = authorize(ws, Actor::Api, ns, Access::Read).unwrap();
    let probe_key = fs.namespace_object_uri(&grant, &NsPath::parse_file("/x").unwrap(), 1);
    let key = probe_key
        .strip_prefix("s3://")
        .and_then(|k| k.split_once('/'))
        .unwrap()
        .1;
    let prefix = key.strip_suffix('x').unwrap();
    assert!(prefix.ends_with("/_files/"), "{prefix}");
    fs.store
        .list(prefix, None)
        .await
        .unwrap()
        .objects
        .into_iter()
        .map(|o| (o.key[prefix.len()..].to_string(), o.size))
        .collect()
}

/// A fixture `objects_after` listing (`f ./main/<ns-as-path>/_files/<rel> <size>`)
/// to its files: rel -> size; `d` lines are directories, not objects.
fn fixture_objects(lines: &Value) -> BTreeMap<String, i64> {
    lines
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| {
            let l = l.as_str().unwrap();
            let rest = l.strip_prefix("f ")?;
            let (path, size) = rest.rsplit_once(' ').unwrap();
            let rel = path.split_once("/_files/").unwrap().1;
            Some((rel.to_string(), size.parse().unwrap()))
        })
        .collect()
}

/// `tasks.DeleteFiles.premise`: `w9b.tasks` as Kestra's upload flow left it,
/// and `w9b.elsewhere`'s `/e.txt`, which egret's refused `up_other_ns`
/// never writes.
async fn seed_delete_premise(db: &Db, fs: &FileStore, ws: Uuid) {
    seed_download_premise(db, fs, ws).await;
    seed(db, fs, ws, "w9b.elsewhere", "/e.txt", b"elsewhere").await;
}

fn delete_task(id: &str) -> String {
    task_of("w9b.tasks/delete", id)
}

/// `runs.del_glob_parent`, `del_string`, `del_none`: SUCCESS with `{}`; the
/// matches are soft-deleted and every file object stays (`objects_after`).
#[tokio::test(flavor = "multi_thread")]
async fn delete_files_soft_deletes_matches_and_keeps_objects() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_delete_premise(&db, &fs, ws).await;
    let before = objects(&fs, ws, NS).await;
    let ids = ["del_glob_parent", "del_string", "del_none"];
    let tasks: Vec<String> = ids.iter().map(|t| delete_task(t)).collect();
    let ex = run(&db, ws, &flow("delete", NS, &tasks)).await;
    let c = cell("DeleteFiles");
    for t in ids {
        assert_eq!(c["runs"][t]["state"], "SUCCESS", "{t}");
        assert_eq!(
            succeeded(&db.pool, ex, t).await,
            c["runs"][t]["outputs"],
            "{t}"
        );
    }
    // Soft: every file object stays, exactly the artifact's files.
    let after = objects(&fs, ws, NS).await;
    assert_eq!(after, fixture_objects(&c["objects_after"]));
    assert_eq!(after, before, "a soft delete removes no object");
    // What was deleted, and what was not.
    for (path, want) in [
        ("/globbed/g/one.txt", false),
        ("/up/m.txt", false),
        ("/globbed/top.txt", true),
        ("/up/sub/n.txt", true),
        ("/rootm.txt", true),
        ("/up2m.txt", true),
    ] {
        assert_eq!(live(&db.pool, ws, NS, path).await, want, "{path}");
    }
    // Every row stays: a delete adds no revision and removes none.
    assert_eq!(
        rows(&db.pool, ws, NS).await,
        [
            "/globbed/g/one.txt",
            "/globbed/top.txt",
            "/rootm.txt",
            "/up/m.txt",
            "/up/sub/n.txt",
            "/up2m.txt"
        ]
        .into_iter()
        .map(|p| (p.to_string(), vec![1]))
        .collect::<BTreeMap<_, _>>()
    );
}

/// `deleteParentFolder` (no `namespace`: the flow's own, ruling D): each
/// parent of a deleted file is soft-deleted, deepest first, only when
/// nothing live is left directly in it. `/p/q/r/` empties first, which
/// empties `/p/q/`; `/p/` keeps `keep.txt`. Without the flag no directory
/// is touched.
#[tokio::test(flavor = "multi_thread")]
async fn delete_parent_folder_removes_parents_left_empty_deepest_first() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let own = "w9b.dpf";
    for path in ["/p/q/r/x.txt", "/p/keep.txt", "/s/t/y.txt"] {
        seed(&db, &fs, ws, own, path, b"x").await;
    }
    let with = "  - id: with\n    type: io.kestra.plugin.core.namespace.DeleteFiles\n    \
                files: \"/p/q/**\"\n    deleteParentFolder: true\n";
    let without = "  - id: without\n    type: io.kestra.plugin.core.namespace.DeleteFiles\n    \
                   files: [\"/s/**\"]\n    deleteParentFolder: \"{{ 1 == 2 }}\"\n";
    let ex = run(&db, ws, &flow("dpf", own, &[with.into(), without.into()])).await;
    assert_eq!(succeeded(&db.pool, ex, "with").await, json!({}));
    assert_eq!(succeeded(&db.pool, ex, "without").await, json!({}));
    for (path, want) in [
        ("/p/q/r/x.txt", false),
        ("/p/q/r/", false),
        ("/p/q/", false),
        ("/p/", true),
        ("/p/keep.txt", true),
        ("/s/t/y.txt", false),
        ("/s/t/", true),
        ("/s/", true),
    ] {
        assert_eq!(live(&db.pool, ws, own, path).await, want, "{path}");
    }
    assert_eq!(listing(&db.pool, ws, own, Some("/p/")).await, ["keep.txt"]);
}

/// `runs.del_other_ns` (chosen): another namespace is refused by the write
/// rule, naming it, and nothing is deleted in either namespace.
#[tokio::test(flavor = "multi_thread")]
async fn delete_files_in_another_namespace_is_refused_and_deletes_nothing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_delete_premise(&db, &fs, ws).await;
    let want = &cell("DeleteFiles")["runs"]["del_other_ns"];
    let ex = run(&db, ws, &flow("delete", NS, &[delete_task("del_other_ns")])).await;
    let message = message_of(
        &db.pool,
        ex,
        "del_other_ns",
        want["state"].as_str().unwrap(),
    )
    .await;
    let named = want["error_names"].as_str().unwrap();
    assert!(message.contains(&format!("'{named}'")), "{message}");
    assert!(
        message.contains("writes only its own namespace"),
        "{message}"
    );
    assert_eq!(want["deleted"], json!([]));
    assert!(live(&db.pool, ws, "w9b.elsewhere", "/e.txt").await);
    assert_eq!(
        listing(&db.pool, ws, "w9b.elsewhere", None).await,
        ["e.txt"]
    );
    assert_eq!(rows(&db.pool, ws, NS).await.len(), 6);
    for path in ["/up/m.txt", "/globbed/g/one.txt"] {
        assert!(live(&db.pool, ws, NS, path).await, "{path}");
    }
}

/// R1 (`tasks.DeleteFiles.rows_after`, chosen): after the whole delete flow,
/// a deleted file's revisions stay listed, flagged deleted, and readable by
/// number; `w9b.elsewhere` keeps `e.txt`.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_files_revisions_stay_listed_and_readable_by_number() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_delete_premise(&db, &fs, ws).await;
    run(&db, ws, &raw_flow("w9b.tasks/delete")).await;
    let want = &cell("DeleteFiles")["rows_after"];
    assert_eq!(
        json!(listing(&db.pool, ws, NS, Some("/globbed/")).await),
        want["/globbed (listing)"]
    );
    assert_eq!(want["/globbed/g"], json!(404));
    assert!(!live(&db.pool, ws, NS, "/globbed/g/").await);
    assert_eq!(
        json!(listing(&db.pool, ws, NS, Some("/up/")).await),
        want["/up (listing)"]
    );
    let grant = authorize(ws, Actor::Api, NS, Access::Read).unwrap();
    let m = NsPath::parse_file("/up/m.txt").unwrap();
    let mut conn = db.pool.acquire().await.unwrap();
    let revs = revisions(&mut conn, &grant, &m).await.unwrap();
    let w = &want["/up/m.txt revisions"];
    assert_eq!(w["status"], json!(200));
    assert_eq!(
        json!(revs.iter().map(|e| e.revision).collect::<Vec<_>>()),
        w["revisions"]
    );
    assert_eq!(
        json!(revs
            .iter()
            .filter(|e| e.deleted)
            .map(|e| e.revision)
            .collect::<Vec<_>>()),
        w["deleted"]
    );
    let w = &want["/up/m.txt?revision=1"];
    assert_eq!(w["status"], json!(200));
    let e = resolve(&mut conn, &grant, &m, Want::Revision(1))
        .await
        .unwrap();
    let mut text = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut open(&fs, &grant, &e).await.unwrap(), &mut text)
        .await
        .unwrap();
    assert_eq!(json!(text), w["value"]);
    // The control: a LATEST read of it is not-found.
    assert!(matches!(
        resolve(&mut conn, &grant, &m, Want::Latest).await,
        Err(NsError::NotFound(_))
    ));
    assert_eq!(
        json!(listing(&db.pool, ws, "w9b.elsewhere", None).await),
        want["w9b.elsewhere"]
    );
}

// ---------------------------------------------------------------------------
// PurgeFiles (Task 7)
// ---------------------------------------------------------------------------

/// `n` revisions of `path` in `ns`, each two bytes (`r1`, `r2`, ...).
async fn seed_revisions(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, path: &str, n: u32) {
    for i in 1..=n {
        seed(db, fs, ws, ns, path, format!("r{i}").as_bytes()).await;
    }
}

fn purge_task(id: &str, props: &str) -> String {
    let mut t = format!("  - id: {id}\n    type: io.kestra.plugin.core.namespace.PurgeFiles\n");
    for line in props.lines().filter(|l| !l.is_empty()) {
        t.push_str(&format!("    {line}\n"));
    }
    t
}

/// The `size` a SUCCESSful purge task reported.
async fn purged(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Value {
    succeeded(pool, ex, task).await["size"].clone()
}

/// Revisions of each namespace's one file `path`, by namespace.
async fn revisions_by_ns(
    pool: &sqlx::PgPool,
    ws: Uuid,
    namespaces: &[&str],
    path: &str,
) -> BTreeMap<String, Vec<i32>> {
    let mut out = BTreeMap::new();
    for ns in namespaces {
        let r = rows(pool, ws, ns).await;
        out.insert(ns.to_string(), r.get(path).cloned().unwrap_or_default());
    }
    out
}

fn as_revisions(v: &Value) -> BTreeMap<String, Vec<i32>> {
    serde_json::from_value(v.clone()).unwrap()
}

/// Spec §8.4 (approved: default the flow's own namespace; children on a dot
/// boundary) and `purge.includeChildNamespaces_a.b_vs_a.bc` (chosen): from
/// `a.b`, the rows of `a.b` and `a.b.c` are purged; `a.bc` and `a` are not.
#[tokio::test(flavor = "multi_thread")]
async fn purge_files_defaults_to_own_namespace_and_its_children_on_a_dot_boundary() {
    let _turn = SERIAL.lock().await;
    let all = ["a", "a.b", "a.b.c", "a.bc"];
    // The fixture's run: `namespaces: ["a.b"]`, children included by default.
    {
        let Some((db, ws, fs)) = setup().await else {
            return;
        };
        for ns in all {
            seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
        }
        let want = &fixture()["egret_expected"]["purge"]["includeChildNamespaces_a.b_vs_a.bc"];
        let ex = run(&db, ws, &raw_flow("a.b/purge-children")).await;
        assert_eq!(purged(&db.pool, ex, "purge").await, want["output"]["size"]);
        let got = revisions_by_ns(&db.pool, ws, &all, "/f.txt").await;
        for (ns, revs) in as_revisions(&want["revisions_after"]) {
            assert_eq!(got[&ns], revs, "{ns}");
        }
        assert_eq!(got["a"], [1, 2]);
    }
    // No `namespaces` at all: the flow's own, and its children.
    {
        let (db, ws, fs) = setup().await.unwrap();
        for ns in all {
            seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
        }
        let ex = run(&db, ws, &flow("pd", "a.b", &[purge_task("purge", "")])).await;
        assert_eq!(purged(&db.pool, ex, "purge").await, json!(2));
        assert_eq!(
            revisions_by_ns(&db.pool, ws, &all, "/f.txt").await,
            BTreeMap::from([
                ("a".to_string(), vec![1, 2]),
                ("a.b".to_string(), vec![2]),
                ("a.b.c".to_string(), vec![2]),
                ("a.bc".to_string(), vec![1, 2]),
            ])
        );
    }
    // `includeChildNamespaces: false`: the flow's own only.
    {
        let (db, ws, fs) = setup().await.unwrap();
        for ns in all {
            seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
        }
        let task = purge_task("purge", "includeChildNamespaces: false");
        let ex = run(&db, ws, &flow("pd", "a.b", &[task])).await;
        assert_eq!(purged(&db.pool, ex, "purge").await, json!(1));
        let got = revisions_by_ns(&db.pool, ws, &all, "/f.txt").await;
        assert_eq!(
            (got["a.b"].clone(), got["a.b.c"].clone()),
            (vec![2], vec![1, 2])
        );
    }
}

/// A listed namespace outside the purge rule (a sibling sharing the
/// prefix, a parent) fails the task naming it, and NOTHING is purged --
/// not even the flow's own namespace, listed first.
#[tokio::test(flavor = "multi_thread")]
async fn purge_files_refuses_a_listed_namespace_outside_the_purge_rule() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let all = ["w9b", "w9b.pr", "w9b.prx"];
    for ns in all {
        seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
    }
    for (listed, refused) in [
        (r#"["w9b.pr", "w9b.prx"]"#, "w9b.prx"),
        (r#"["w9b.pr", "w9b"]"#, "w9b"),
    ] {
        let task = purge_task("purge", &format!("namespaces: {listed}"));
        let ex = run(&db, ws, &flow("pr", "w9b.pr", &[task])).await;
        let message = message_of(&db.pool, ex, "purge", "FAILED").await;
        assert!(
            message.contains(&format!("namespace '{refused}'"))
                && message.contains("purges only its own namespace"),
            "{listed}: {message}"
        );
        for ns in all {
            assert_eq!(
                rows(&db.pool, ws, ns).await["/f.txt"],
                [1, 2],
                "{listed}: {ns}"
            );
        }
    }
}

/// `purge.namespacePattern.with_namespaces_too` (chosen): both set is
/// refused naming both, before anything is purged; an EMPTY `namespaces`
/// beside a pattern is absent, as Kestra's `isEmpty` reads it.
#[tokio::test(flavor = "multi_thread")]
async fn namespaces_and_namespace_pattern_are_exclusive() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_revisions(&db, &fs, ws, "w9b.pp.a", "/f.txt", 2).await;
    let want = &fixture()["egret_expected"]["purge"]["namespacePattern"]["with_namespaces_too"];
    assert_eq!(want["refused"], json!(true));
    let both = task_of("w9b.pp/purge-pattern", "both");
    assert!(both.contains("allowFailure: true"), "{both}");
    let ex = run(&db, ws, &flow("both", "w9b.pp", &[both])).await;
    let message = message_of(&db.pool, ex, "both", "WARNING").await;
    for name in want["names"].as_array().unwrap() {
        assert!(
            message.contains(&format!("`{}`", name.as_str().unwrap())),
            "{message}"
        );
    }
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a").await["/f.txt"], [1, 2]);
    let task = purge_task("purge", "namespaces: []\nnamespacePattern: \"w9b.pp.*\"");
    let ex = run(&db, ws, &flow("empty", "w9b.pp", &[task])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(1));
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a").await["/f.txt"], [2]);
}

/// R2 (`purge.namespacePattern`, chosen): Kestra's wildcard over the
/// namespaces holding files that are the flow's own or its children.
/// `w9b.pp.*` from `w9b.pp` purges `w9b.pp.a`, `w9b.pp.b` and
/// `w9b.pp.noflow` (files, no flow) and never `w9b.ppx.c`; `*` never reaches
/// `w9b.ppx.c` either (a filter, not a refusal); `?` is one character;
/// `w9b.pp.a.b` takes `w9b.pp.a.b` and not `w9b.pp.a-b` (a regex would take
/// both); the match is case-sensitive.
#[tokio::test(flavor = "multi_thread")]
async fn namespace_pattern_is_a_wildcard_over_own_and_child_namespaces_holding_files() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let want = &fixture()["egret_expected"]["purge"]["namespacePattern"];
    assert_eq!(want["pattern"], json!("w9b.pp.*"));
    let all = ["w9b.pp.a", "w9b.pp.b", "w9b.pp.noflow", "w9b.ppx.c"];
    for ns in all {
        seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
    }
    let ex = run(
        &db,
        ws,
        &flow("pp", "w9b.pp", &[task_of("w9b.pp/purge-pattern", "purge")]),
    )
    .await;
    assert_eq!(purged(&db.pool, ex, "purge").await, want["output"]["size"]);
    assert_eq!(
        revisions_by_ns(&db.pool, ws, &all, "/f.txt").await,
        as_revisions(&want["revisions_after"])
    );

    // `*` from w9b.pp: every candidate, and w9b.ppx.c is not one.
    seed(&db, &fs, ws, "w9b.pp.b", "/f.txt", b"r3").await;
    let ex = run(
        &db,
        ws,
        &flow(
            "pp",
            "w9b.pp",
            &[purge_task("purge", "namespacePattern: \"*\"")],
        ),
    )
    .await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(1));
    assert_eq!(rows(&db.pool, ws, "w9b.ppx.c").await["/f.txt"], [1, 2]);
    assert_eq!(rows(&db.pool, ws, "w9b.pp.b").await["/f.txt"], [3]);

    // A dot is a dot: `w9b.pp.a.b` and `w9b.pp.a-b` are both children.
    for ns in ["w9b.pp.a.b", "w9b.pp.a-b"] {
        seed_revisions(&db, &fs, ws, ns, "/f.txt", 2).await;
    }
    let task = purge_task("purge", "namespacePattern: w9b.pp.a.b");
    let ex = run(&db, ws, &flow("pp", "w9b.pp", &[task])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(1));
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a.b").await["/f.txt"], [2]);
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a-b").await["/f.txt"], [1, 2]);

    // `?` is exactly one character; case matters.
    seed(&db, &fs, ws, "w9b.pp.a", "/f.txt", b"r3").await;
    seed(&db, &fs, ws, "w9b.pp.noflow", "/f.txt", b"r3").await;
    for (pattern, size) in [("W9B.PP.?", 0), ("w9b.pp.?", 1)] {
        let task = purge_task("purge", &format!("namespacePattern: \"{pattern}\""));
        let ex = run(&db, ws, &flow("pp", "w9b.pp", &[task])).await;
        assert_eq!(
            purged(&db.pool, ex, "purge").await,
            json!(size),
            "{pattern}"
        );
    }
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a").await["/f.txt"], [3]);
    assert_eq!(rows(&db.pool, ws, "w9b.pp.noflow").await["/f.txt"], [2, 3]);
    assert_eq!(rows(&db.pool, ws, "w9b.pp.a-b").await["/f.txt"], [1, 2]);
}

/// `keepAmount: N` keeps each path's N highest revisions; `size` counts
/// the revisions purged; the default behavior is `keepAmount: 1`; 0 keeps
/// nothing, as Kestra's `skip(0)`. Purged objects are gone, kept ones stay.
#[tokio::test(flavor = "multi_thread")]
async fn keep_amount_keeps_the_n_highest_revisions_and_size_counts_revisions_purged() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let own = "w9b.ka";
    seed_revisions(&db, &fs, ws, own, "/k.txt", 4).await;
    seed_revisions(&db, &fs, ws, own, "/one.txt", 1).await;
    seed_revisions(&db, &fs, ws, own, "/d/n.txt", 2).await;
    let keep = |n: u32| {
        purge_task(
            "purge",
            &format!("behavior:\n  type: version\n  keepAmount: {n}"),
        )
    };
    let ex = run(&db, ws, &flow("ka", own, &[keep(3)])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(1));
    assert_eq!(rows(&db.pool, ws, own).await["/k.txt"], [2, 3, 4]);
    let ex = run(&db, ws, &flow("ka", own, &[purge_task("purge", "")])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(3));
    assert_eq!(
        rows(&db.pool, ws, own).await,
        BTreeMap::from([
            ("/d/n.txt".to_string(), vec![2]),
            ("/k.txt".to_string(), vec![4]),
            ("/one.txt".to_string(), vec![1]),
        ])
    );
    assert_eq!(
        objects(&fs, ws, own).await.into_keys().collect::<Vec<_>>(),
        ["d/n.txt.v2", "k.txt.v4", "one.txt"]
    );
    let ex = run(&db, ws, &flow("ka", own, &[keep(0)])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(3));
    assert_eq!(rows(&db.pool, ws, own).await, BTreeMap::new());
    assert_eq!(objects(&fs, ws, own).await, BTreeMap::new());
}

/// `purge.before` (measured): `filePattern` with a leading `/` matches
/// nothing; `before` purges revisions created before the instant and never
/// a path's latest, even when it is older too.
#[tokio::test(flavor = "multi_thread")]
async fn before_purges_older_revisions_and_never_the_latest() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let want = &fixture()["egret_expected"]["purge"]["before"];
    let own = "w9b.pb";
    // The setup as recorded, each revision's creation time set to Kestra's.
    let mut per_path: BTreeMap<String, i32> = BTreeMap::new();
    for step in want["setup"].as_array().unwrap() {
        assert_eq!(step["ns"], json!(own));
        let path = step["path"].as_str().unwrap();
        seed(&db, &fs, ws, own, path, b"xx").await;
        let rev = per_path.entry(path.to_string()).or_default();
        *rev += 1;
        let at: chrono::DateTime<chrono::Utc> = step["at"].as_str().unwrap().parse().unwrap();
        let n = sqlx::query(
            "UPDATE workflow.namespace_files SET created_at = $5 \
              WHERE workspace_id = $1 AND namespace = $2 AND path = $3 AND revision = $4",
        )
        .bind(ws)
        .bind(own)
        .bind(path)
        .bind(*rev)
        .bind(at)
        .execute(&db.pool)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(n, 1);
    }
    let ex = run(&db, ws, &raw_flow("w9b.pb/purge-before")).await;
    let none = &want["filePattern_with_leading_slash"];
    assert_eq!(none["pattern"], json!("/q.txt"));
    for t in ["purge_mid", "purge_end_q2"] {
        assert_eq!(purged(&db.pool, ex, t).await, none["output"]["size"], "{t}");
    }
    let ex = run(&db, ws, &raw_flow("w9b.pb/purge-before2")).await;
    assert_eq!(
        purged(&db.pool, ex, "purge_mid_q").await,
        want["filePattern_q.txt_before_t_mid"]["output"]["size"]
    );
    assert_eq!(
        purged(&db.pool, ex, "purge_end_q2").await,
        want["filePattern_q2*_before_after_every_write"]["output"]["size"]
    );
    assert_eq!(
        rows(&db.pool, ws, own).await,
        BTreeMap::from([
            ("/q.txt".to_string(), vec![3]),
            ("/q2.txt".to_string(), vec![2]),
        ])
    );
    assert_eq!(
        objects(&fs, ws, own).await,
        fixture_objects(&want["objects_after"])
    );
}

/// `purge.keepAmount_1_on_soft_deleted` (chosen; Kestra keeps the deleted
/// latest revision, size 4): a path whose latest revision is soft-deleted
/// is purged entirely -- size 5, its revisions unreadable, its objects
/// gone -- and a rewrite starts again at revision 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_soft_deleted_path_is_purged_entirely() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let want = &fixture()["egret_expected"]["purge"]["keepAmount_1_on_soft_deleted"];
    assert_eq!(want["kestra"]["output"]["size"], json!(4));
    let own = "w9b.pk";
    seed_revisions(&db, &fs, ws, own, "/p.txt", 3).await;
    seed_revisions(&db, &fs, ws, own, "/live.txt", 3).await;
    assert_eq!(
        objects(&fs, ws, own).await,
        fixture_objects(&want["objects_before"])
    );
    let write = authorize(ws, Actor::Api, own, Access::Write).unwrap();
    let p = NsPath::parse_file("/p.txt").unwrap();
    assert_eq!(soft_delete(&db.pool, &write, &p).await.unwrap().len(), 1);
    let ex = run(&db, ws, &raw_flow("w9b.pk/purge")).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, want["output"]["size"]);
    assert_eq!(
        json!(objects(&fs, ws, own).await.into_keys().collect::<Vec<_>>()),
        want["objects_after"]
    );
    let grant = authorize(ws, Actor::Api, own, Access::Read).unwrap();
    let mut conn = db.pool.acquire().await.unwrap();
    let live_revs = revisions(&mut conn, &grant, &NsPath::parse_file("/live.txt").unwrap())
        .await
        .unwrap();
    assert_eq!(
        json!(live_revs.iter().map(|e| e.revision).collect::<Vec<_>>()),
        want["revisions_after"]["/live.txt"]["revisions"]
    );
    assert_eq!(want["revisions_after"]["/p.txt"]["status"], json!(404));
    assert!(matches!(
        revisions(&mut conn, &grant, &p).await,
        Err(NsError::NotFound(_))
    ));
    for (n, status) in want["p_revision_reads_after"].as_object().unwrap() {
        assert_eq!(status, &json!(404));
        let n: u32 = n.parse().unwrap();
        assert!(
            matches!(
                resolve(&mut conn, &grant, &p, Want::Revision(n)).await,
                Err(NsError::NotFound(_))
            ),
            "revision {n}"
        );
    }
    drop(conn);
    let rewrite = &want["rewrite_after_purge"];
    seed(&db, &fs, ws, own, "/p.txt", b"rewrite").await;
    assert_eq!(
        json!(rows(&db.pool, ws, own).await["/p.txt"]),
        rewrite["revisions"]
    );
    assert_eq!(
        json!(objects(&fs, ws, own).await.into_keys().collect::<Vec<_>>()),
        rewrite["objects"]
    );
}

/// `filePattern` is Commons-IO's wildcard on the path without its leading
/// `/`: `*` spans `/`, `?` is one character, case-sensitive.
#[tokio::test(flavor = "multi_thread")]
async fn file_pattern_is_a_commons_io_wildcard() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let own = "w9b.fp";
    for path in ["/a/x.txt", "/a/y.csv", "/b.txt", "/a/sub/z.txt"] {
        seed_revisions(&db, &fs, ws, own, path, 2).await;
    }
    for (pattern, size) in [
        ("/b.txt", 0),
        ("*.CSV", 0),
        ("a/*.txt", 2),
        ("?.txt", 1),
        ("*.csv", 1),
    ] {
        let task = purge_task("purge", &format!("filePattern: \"{pattern}\""));
        let ex = run(&db, ws, &flow("fp", own, &[task])).await;
        assert_eq!(
            purged(&db.pool, ex, "purge").await,
            json!(size),
            "{pattern}"
        );
    }
    assert_eq!(
        rows(&db.pool, ws, own).await,
        ["/a/sub/z.txt", "/a/x.txt", "/a/y.csv", "/b.txt"]
            .into_iter()
            .map(|p| (p.to_string(), vec![2]))
            .collect::<BTreeMap<_, _>>()
    );
}

/// After purges of every kind (keepAmount, a soft-deleted path, a
/// directory's files), every surviving row has its object, and every
/// object left has its row: a purge deletes exact keys.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_leaves_no_row_without_its_object() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let own = "w9b.pn";
    seed_revisions(&db, &fs, ws, own, "/a.txt", 3).await;
    seed_revisions(&db, &fs, ws, own, "/a.txt.d/b.txt", 2).await;
    seed_revisions(&db, &fs, ws, own, "/gone.txt", 2).await;
    seed_revisions(&db, &fs, ws, "w9b.pn.c", "/c.txt", 2).await;
    let write = authorize(ws, Actor::Api, own, Access::Write).unwrap();
    soft_delete(&db.pool, &write, &NsPath::parse_file("/gone.txt").unwrap())
        .await
        .unwrap();
    let ex = run(&db, ws, &flow("pn", own, &[purge_task("purge", "")])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(6));
    let mut kept = BTreeSet::new();
    for ns in [own, "w9b.pn.c"] {
        let grant = authorize(ws, Actor::Api, ns, Access::Read).unwrap();
        for (path, revs) in rows(&db.pool, ws, ns).await {
            for rev in revs {
                let p = NsPath::parse_file(&path).unwrap();
                let key = fs.namespace_object_uri(&grant, &p, rev as u32);
                assert!(
                    fs.store.head(&key).await.unwrap().is_some(),
                    "{ns} {path} r{rev}"
                );
                kept.insert(format!("{ns} {path} r{rev}"));
            }
        }
        assert_eq!(
            objects(&fs, ws, ns).await.len(),
            rows(&db.pool, ws, ns)
                .await
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            "{ns}: an object per row, no more"
        );
    }
    assert_eq!(
        kept,
        BTreeSet::from([
            format!("{own} /a.txt r3"),
            format!("{own} /a.txt.d/b.txt r2"),
            "w9b.pn.c /c.txt r2".to_string(),
        ])
    );
}

/// Every list is bounded, and crossing a bound is the author's error naming
/// it, before anything is deleted or purged: DeleteFiles' live listing and
/// PurgeFiles' revisions (both `max_namespace_files`, 10 in this binary),
/// PurgeFiles' listed namespaces (`MAX_NAMESPACES`).
#[tokio::test(flavor = "multi_thread")]
async fn delete_and_purge_bounds_are_author_errors_naming_the_cap() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let own = "w9b.cap";
    for i in 0..11 {
        seed(&db, &fs, ws, own, &format!("/f{i:02}.txt"), b"x").await;
    }
    let del = "  - id: del\n    type: io.kestra.plugin.core.namespace.DeleteFiles\n    \
               files: [\"**\"]\n";
    let ex = run(&db, ws, &flow("cap", own, &[del.into()])).await;
    let message = message_of(&db.pool, ex, "del", "FAILED").await;
    assert!(
        message.contains("workflow.storage.max_namespace_files") && message.contains("10"),
        "{message}"
    );
    assert!(live(&db.pool, ws, own, "/f00.txt").await);
    // 11 revisions over two namespaces: refused, nothing purged.
    let other = "w9b.cap2";
    seed_revisions(&db, &fs, ws, other, "/g.txt", 2).await;
    let write = authorize(ws, Actor::Api, own, Access::Write).unwrap();
    for i in 2..11 {
        soft_delete(
            &db.pool,
            &write,
            &NsPath::parse_file(&format!("/f{i:02}.txt")).unwrap(),
        )
        .await
        .unwrap();
    }
    let task = purge_task("purge", "namespaces: [w9b.cap2, w9b.cap]");
    let ex = run(&db, ws, &flow("cap", own, &[task])).await;
    // `w9b.cap2` is not a child of `w9b.cap`: the rule first.
    let message = message_of(&db.pool, ex, "purge", "FAILED").await;
    assert!(message.contains("'w9b.cap2'"), "{message}");
    let ex = run(&db, ws, &flow("cap", "w9b", &[task_of_purge_two()])).await;
    let message = message_of(&db.pool, ex, "purge", "FAILED").await;
    assert!(
        message.contains("workflow.storage.max_namespace_files")
            && message.contains("nothing was purged"),
        "{message}"
    );
    assert_eq!(rows(&db.pool, ws, own).await.len(), 11);
    assert_eq!(rows(&db.pool, ws, other).await["/g.txt"], [1, 2]);
    // The control: one namespace alone (11 rows) is still over; with 10 it runs.
    soft_delete(&db.pool, &write, &NsPath::parse_file("/f00.txt").unwrap())
        .await
        .unwrap();
    let grant = authorize(ws, Actor::Api, own, Access::Purge).unwrap();
    egret_workflow::storage::nsfiles::store::purge_revisions(
        &db.pool,
        &fs,
        &grant,
        &NsPath::parse_file("/f00.txt").unwrap(),
        &[1],
    )
    .await
    .unwrap();
    let ex = run(&db, ws, &flow("cap", own, &[purge_task("purge", "")])).await;
    assert_eq!(purged(&db.pool, ex, "purge").await, json!(9));
    // MAX_NAMESPACES + 1 listed: refused naming the bound.
    let many: Vec<String> = (0..=egret_workflow::storage::nsfiles::stage::MAX_NAMESPACES)
        .map(|i| format!("w9b.cap.n{i}"))
        .collect();
    let task = purge_task("purge", &format!("namespaces: {}", json!(many)));
    let ex = run(&db, ws, &flow("cap", own, &[task])).await;
    let message = message_of(&db.pool, ex, "purge", "FAILED").await;
    assert!(
        message.contains(&format!(
            "more than the {}",
            egret_workflow::storage::nsfiles::stage::MAX_NAMESPACES
        )),
        "{message}"
    );
}

/// `w9b.cap` and `w9b.cap2` from their parent `w9b`, children included.
fn task_of_purge_two() -> String {
    purge_task(
        "purge",
        "namespaces: [w9b.cap, w9b.cap2]\nincludeChildNamespaces: false",
    )
}
