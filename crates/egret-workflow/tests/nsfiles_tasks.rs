//! W9b Task 6: `io.kestra.plugin.core.namespace.{UploadFiles, DownloadFiles}`
//! through the real worker (W9b spec §8.1, §8.2), against
//! `oracle-exec/w9b-namespace-files.json`'s `tasks.UploadFiles` and
//! `tasks.DownloadFiles`. Requires DATABASE_URL.
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
use egret_workflow::storage::nsfiles::store::{put, Conflict, Written};
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
    assert_eq!(nst::DECLARED.len(), 2);
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
    assert_eq!(
        cell("UploadFiles")["schema"]["properties"]["namespace"]["kestra"]["required"],
        json!(true),
        "the control: Kestra's own answer differs, and DECLARED follows the top level"
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
    let yaml = raw_flow("w9b.tasks/download");
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
