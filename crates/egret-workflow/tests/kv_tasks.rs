//! W9c Task 5: `io.kestra.plugin.core.kv.{Set, Get, GetKeys, Delete, Put,
//! PurgeKV}` (W9c spec §8) against `oracle-exec/w9c-kv.json`. The DB tests
//! require DATABASE_URL.
//!
//! Most tests call a handler directly, with a [`TaskInput`] whose render
//! scope names the flow's namespace ([`FlowNs`]): a re-claim is then the
//! same input run twice. One test runs a whole flow through the worker.
//!
//! The store is [`Probe`] over a `MemStore`, installed once for this binary:
//! nothing here touches MinIO, so every delete and purge is on memory. Every
//! test makes its own workspace; the tests take turns on [`SERIAL`] because
//! the probe's switches and a claim are global. `multi_thread`: a render
//! that reads `kv()` blocks its thread on the runtime.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::executor::RunState;
use egret_workflow::logs::RunLog;
use egret_workflow::runners::files::{FileMeta, FileScope, FileUnavailable, RenderFiles};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::store::{
    self as kv_store, read_value, resolve, KvWritten, NewValue,
};
use egret_workflow::storage::kv::value::{
    coerce, encode, parse_api_body, parse_ttl, KvData, KvNumber, KvType, KvValue, Ttl,
};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::store::Want;
use egret_workflow::storage::nsfiles::{self, CreatedBy, NsError};
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::handlers::kv::{
    self as kvt, put_patch, render_value, set_value, DeleteKv, GetKeys, GetKv, PurgeKv, PutKv,
    SetKv,
};
use egret_workflow::worker::{start_execution, tick, Handlers, Outcome, TaskHandler, TaskInput};
use egret_workflow::{render_typed, RenderContext};
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// While set, `get_stream` fails as an unreachable S3 does.
static OUTAGE: AtomicBool = AtomicBool::new(false);
/// While set, `put` and `copy` fail as an unreachable S3 does.
static WRITE_OUTAGE: AtomicBool = AtomicBool::new(false);
/// While set, `get_stream` signals [`STALLED`] and waits for [`RELEASE`].
static STALL: AtomicBool = AtomicBool::new(false);
static STALLED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static RELEASE: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn down(uri: &str) -> StorageError {
    StorageError::S3(format!("{uri}: dispatch failure: connection refused"))
}

struct Probe(egret_storage::MemStore);

#[async_trait::async_trait]
impl ObjectStore for Probe {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        if WRITE_OUTAGE.load(Ordering::SeqCst) {
            return Err(down(uri));
        }
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        if WRITE_OUTAGE.load(Ordering::SeqCst) {
            return Err(down(dst));
        }
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
            return Err(down(uri));
        }
        if STALL.load(Ordering::SeqCst) {
            STALLED.notify_one();
            RELEASE.notified().await;
        }
        self.0.get_stream(uri).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        if WRITE_OUTAGE.load(Ordering::SeqCst) {
            return Err(down(uri));
        }
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
}

fn probe() -> Arc<Probe> {
    static P: std::sync::OnceLock<Arc<Probe>> = std::sync::OnceLock::new();
    P.get_or_init(|| Arc::new(Probe(egret_storage::MemStore::new())))
        .clone()
}

/// A `FileStore` over the probe's objects with `caps`.
fn capped(caps: storage::Caps) -> Arc<FileStore> {
    Arc::new(FileStore {
        store: probe(),
        bucket: "egret-test".to_string(),
        prefix: "workflow/".to_string(),
        caps,
        retention_days: 30,
    })
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        storage::install(capped(storage::Caps::default()))
            .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

async fn setup() -> Option<(Db, Uuid, Arc<FileStore>)> {
    let db = common::db().await?;
    nsfiles::install_db_for_tests(&db.pool);
    OUTAGE.store(false, Ordering::SeqCst);
    WRITE_OUTAGE.store(false, Ordering::SeqCst);
    STALL.store(false, Ordering::SeqCst);
    let fs = installed();
    let (ws, _) = common::seed_workspace(&db).await;
    Some((db, ws, fs))
}

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        serde_json::from_str(include_str!("../oracle-exec/w9c-kv.json")).expect("the fixture")
    })
}

fn cells(group: &str) -> &'static Value {
    &fixture()["egret_expected"][group]
}

// ---------------------------------------------------------------------------
// running a handler directly
// ---------------------------------------------------------------------------

/// A render scope that knows only the flow's namespace.
struct FlowNs(String);

impl RenderFiles for FlowNs {
    fn lookup(&self, uri: &str) -> Result<FileMeta, FileUnavailable> {
        Err(FileUnavailable::Refused(format!("{uri}: no files here")))
    }
    fn read_utf8(&self, uri: &str) -> Result<String, String> {
        Err(format!("{uri}: no files here"))
    }
    fn read_bytes(&self, uri: &str, _whole: &str) -> Result<Vec<u8>, String> {
        Err(format!("{uri}: no files here"))
    }
    fn task_prefix(&self) -> Option<&str> {
        None
    }
    fn flow_namespace(&self) -> Option<&str> {
        Some(&self.0)
    }
}

/// One task run of a flow of namespace `ns` in `ws`.
fn input(ws: Uuid, ns: &str, type_id: &str, spec: Value) -> TaskInput {
    TaskInput {
        workspace_id: ws,
        execution_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        task_id: "t".to_string(),
        type_id: type_id.to_string(),
        value: None,
        attempt: 1,
        spec,
        render: RenderContext::new().with_files(FileScope(Arc::new(FlowNs(ns.to_string())))),
        timeout: None,
        working_dir: None,
        principal: None,
        claimed_at: chrono::Utc::now(),
        log: RunLog::discard(),
    }
}

/// What one run answered: SUCCESS with its outputs, or FAILED with its
/// message without the type prefix.
#[derive(Debug, Clone, PartialEq)]
enum Ran {
    Ok(Value),
    Failed(String),
}

impl Ran {
    fn ok(self) -> Value {
        match self {
            Ran::Ok(v) => v,
            Ran::Failed(m) => panic!("failed: {m}"),
        }
    }
    fn failed(self) -> String {
        match self {
            Ran::Failed(m) => m,
            Ran::Ok(v) => panic!("succeeded: {v}"),
        }
    }
}

fn ran(type_id: &str, o: Outcome) -> Ran {
    match o.state {
        RunState::Success => Ran::Ok(o.outputs.unwrap_or(json!({}))),
        RunState::Failed => {
            let m = o.message.unwrap_or_default();
            let prefix = format!("{type_id}: ");
            Ran::Failed(m.strip_prefix(&prefix).map_or(m.clone(), str::to_string))
        }
        other => panic!("{type_id}: state {other:?}"),
    }
}

/// The six handlers over `pool` and `fs`.
struct Tasks {
    set: SetKv,
    get: GetKv,
    keys: GetKeys,
    del: DeleteKv,
    put: PutKv,
    purge: PurgeKv,
}

impl Tasks {
    fn new(pool: &sqlx::PgPool, fs: Option<Arc<FileStore>>) -> Self {
        Self {
            set: SetKv::new(pool.clone(), fs.clone()),
            get: GetKv::new(pool.clone(), fs.clone()),
            keys: GetKeys::new(pool.clone(), fs.clone()),
            del: DeleteKv::new(pool.clone(), fs.clone()),
            put: PutKv::new(pool.clone(), fs.clone()),
            purge: PurgeKv::new(pool.clone(), fs),
        }
    }

    fn handler(&self, type_id: &str) -> &dyn TaskHandler {
        match type_id {
            kvt::SET => &self.set,
            kvt::GET => &self.get,
            kvt::GET_KEYS => &self.keys,
            kvt::DELETE => &self.del,
            kvt::PUT => &self.put,
            kvt::PURGE_KV => &self.purge,
            other => panic!("{other}"),
        }
    }

    /// Run `task`; a worker error (`Err`) fails the test here.
    async fn run(&self, task: &TaskInput) -> Ran {
        let o = self
            .handler(&task.type_id)
            .run(task)
            .await
            .unwrap_or_else(|e| panic!("{}: a worker error: {e}", task.type_id));
        ran(&task.type_id, o)
    }

    /// A fresh run of `type_id` with `spec`, as a flow of `ns`.
    async fn go(&self, ws: Uuid, ns: &str, type_id: &str, spec: Value) -> Ran {
        self.run(&input(ws, ns, type_id, spec)).await
    }
}

// ---------------------------------------------------------------------------
// seeding and reading
// ---------------------------------------------------------------------------

fn key(k: &str) -> KvKey {
    KvKey::parse(k).unwrap()
}

fn api(ws: Uuid, ns: &str, access: Access) -> Grant {
    authorize(ws, Actor::Api, ns, Resource::Kv, access).unwrap()
}

const USER: CreatedBy = CreatedBy::User(Uuid::nil());

/// One revision of `k` in `ns`, as the API writes it.
async fn seed(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, k: &str, value: KvValue) {
    let new = NewValue {
        value,
        description: None,
        ttl: None,
        overwrite: true,
    };
    match kv_store::set(
        &db.pool,
        fs,
        &api(ws, ns, Access::Write),
        &key(k),
        new,
        USER,
    )
    .await
    {
        Ok(KvWritten::Written(_)) => {}
        other => panic!("seed {ns} {k}: {other:?}"),
    }
}

fn string(s: &str) -> KvValue {
    KvValue {
        kind: KvType::String,
        data: KvData::String(s.to_string()),
    }
}

fn typed(v: &Value) -> KvValue {
    KvValue::from_json(KvType::of(v), v).unwrap()
}

/// A revision row inserted directly (no object): `ago` seconds old, expired
/// or soft-deleted as asked.
#[allow(clippy::too_many_arguments)]
async fn row(
    db: &Db,
    ws: Uuid,
    ns: &str,
    k: &str,
    rev: i32,
    ago: f64,
    expired: bool,
    deleted: bool,
) {
    sqlx::query(
        "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, \
           value_type, size, created_at, expires_at, deleted, deleted_at) \
         VALUES ($1, $2, $3, $4, 'STRING', 1, now() - make_interval(secs => $5), \
           CASE WHEN $6 THEN now() - interval '1 second' END, $7, \
           CASE WHEN $7 THEN now() END)",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .bind(rev)
    .bind(ago)
    .bind(expired)
    .bind(deleted)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn expire_latest(db: &Db, ws: Uuid, ns: &str, k: &str) {
    sqlx::query(
        "UPDATE workflow.kv_entries SET expires_at = now() - interval '1 second' \
         WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND revision = \
           (SELECT max(revision) FROM workflow.kv_entries \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3)",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// `(revision, latest, deleted, expires set)` of every row of `k`, ascending.
async fn rows(db: &Db, ws: Uuid, ns: &str, k: &str) -> Vec<(i32, bool, bool, bool)> {
    let all: Vec<(i32, bool, bool)> = sqlx::query_as(
        "SELECT revision, deleted, expires_at IS NOT NULL FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3 ORDER BY revision",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let n = all.len();
    all.into_iter()
        .enumerate()
        .map(|(i, (r, d, e))| (r, i + 1 == n, d, e))
        .collect()
}

/// The fixture's `rows_after` in [`rows`]' shape.
fn want_rows(cell: &Value) -> Vec<(i32, bool, bool, bool)> {
    cell["rows_after"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["revision"].as_i64().unwrap() as i32,
                r["latest"].as_bool().unwrap(),
                r["deleted"].as_bool().unwrap(),
                !r["expires"].is_null(),
            )
        })
        .collect()
}

async fn count_rows(db: &Db, ws: Uuid, ns: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1 AND namespace = $2",
    )
    .bind(ws)
    .bind(ns)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// The latest live value of `k`, as a reader sees it.
async fn latest(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, k: &str) -> Value {
    let g = api(ws, ns, Access::Read);
    let e = {
        let mut c = db.pool.acquire().await.unwrap();
        resolve(&mut c, &g, &key(k), Want::Latest).await.unwrap()
    };
    read_value(&db.pool, fs, &g, &e).await.unwrap().to_json()
}

/// The bytes of the latest revision of `k`.
async fn latest_bytes(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, k: &str) -> String {
    let g = api(ws, ns, Access::Read);
    let e = {
        let mut c = db.pool.acquire().await.unwrap();
        resolve(&mut c, &g, &key(k), Want::Latest).await.unwrap()
    };
    let object = fs.namespace_kv_uri(&g, &key(k), e.revision);
    String::from_utf8(fs.store.get(&object).await.unwrap()).unwrap()
}

/// JSON-equal as the fixture's `compare` says: object key order is not
/// compared, and two numbers are equal when their texts or their `f64`s are
/// (the outputs API prints a double `1e20` as `100000000000000000000`).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.to_string() == y.to_string() || (x.as_f64().is_some() && x.as_f64() == y.as_f64())
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The bytes `encode` makes of a value, as text.
fn bytes(v: &KvValue) -> String {
    String::from_utf8(encode(v).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------
// the shapes (no database)
// ---------------------------------------------------------------------------

/// DECLARED equals each `schema` cell's top level: names in order,
/// `required`, `default`; outputs by name. The six classes are the
/// artifact's listing, and each is a known type at publish.
#[tokio::test]
async fn declared_properties_and_outputs_match_the_artifact() {
    let schema = cells("schema");
    assert_eq!(kvt::DECLARED.len(), 6);
    for (type_id, props, outputs) in kvt::DECLARED {
        let class = type_id.rsplit('.').next().unwrap();
        let cell = &schema[class];
        assert_eq!(cell["class"], json!(type_id), "{class}");
        let theirs = cell["properties"].as_object().unwrap();
        let names: Vec<&str> = props.iter().map(|p| p.0).collect();
        let their_names: Vec<&str> = theirs.keys().map(String::as_str).collect();
        assert_eq!(names, their_names, "{class}: property names");
        for (name, required, default) in *props {
            let p = &theirs[*name];
            assert_eq!(p["required"], json!(required), "{class}.{name} required");
            assert_eq!(p["default"].as_str(), *default, "{class}.{name} default");
        }
        let outs: Vec<&str> = cell["outputs"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(*outputs, outs.as_slice(), "{class}: outputs");
        assert!(
            egret_workflow::spec::registry::known_type(type_id).is_some(),
            "{type_id} is a registry row"
        );
    }
    let listed: BTreeSet<&str> = schema["listing"]["kv_classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let ours: BTreeSet<&str> = kvt::DECLARED.iter().map(|c| c.0).collect();
    assert_eq!(ours, listed);
    // The shipping registry runs all six.
    let mut r = Handlers::new();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    kvt::register(&mut r, pool, None);
    for t in &ours {
        assert!(r.get(t).is_some(), "{t} registered");
    }
}

/// [`render_typed`]'s value as a reader sees it.
fn rendered_typed(template: &str, cx: &RenderContext) -> Value {
    render_typed(template, cx)
        .unwrap_or_else(|e| panic!("{template}: {e}"))
        .unwrap_or_else(|e| panic!("{template}: {e}"))
        .to_json()
}

/// w9c `typed_render`, every string cell: the typed render, then `coerce`
/// with no kvType, then the bytes and the type.
#[test]
fn render_typed_matches_the_artifact() {
    let cx = RenderContext::new();
    let mut examined = 0;
    for (name, cell) in cells("typed_render").as_object().unwrap() {
        let Some(template) = cell["value"].as_str() else {
            continue;
        };
        let rendered = render_typed(template, &cx)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let v = coerce(rendered, None).unwrap();
        assert_eq!(json!(v.kind.as_str()), cell["stored_type"], "{name}");
        let got = bytes(&v);
        if name == "tr_now" {
            let inner = got
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap();
            assert!(
                chrono::DateTime::parse_from_rfc3339(inner).is_ok(),
                "{name}: {got}"
            );
        } else {
            // `tr_div` included: chosen (the user's ruling, 2026-10-08),
            // egret's float division `3.5e0` at its top level.
            assert_eq!(json!(got), cell["ion_bytes"], "{name}");
        }
        examined += 1;
    }
    assert_eq!(examined, 18);
    // One region and nothing else, the region's end found past nested braces.
    for (template, want) in [
        ("{{ {'a': {'b': 1}} }}", json!({"a": {"b": 1}})),
        ("{{- 2 -}}", json!(2)),
        ("x {{ 1 }}", json!("x 1")),
        ("{{ 1 }} x", json!("1 x")),
        ("{{ '}}' }}", json!("}}")),
        ("{{ 1 }}{{ 2 }}", json!("12")),
    ] {
        assert_eq!(rendered_typed(template, &cx), want, "{template}");
    }
}

/// R-a: `Set` and `Put` render a YAML map or list by one rule (w9c
/// `typed_render.tr_yaml_*`, chosen; `put_merge.templated_leaf`, `templated`).
#[test]
fn set_and_put_render_a_yaml_map_by_one_rule() {
    let cx = RenderContext::new();
    let tr = cells("typed_render");
    for name in ["tr_yaml_map", "tr_yaml_list", "tr_yaml_int", "tr_yaml_bool"] {
        let cell = &tr[name];
        let v = set_value(&cell["value"], None, &cx).unwrap();
        assert_eq!(json!(v.kind.as_str()), cell["stored_type"], "{name}");
        assert_eq!(json!(bytes(&v)), cell["ion_bytes"], "{name}");
        assert_eq!(v.data.to_json(), cell["kv_returns"], "{name}");
        // Put's patch is the same value.
        assert_eq!(put_patch(&cell["value"], &cx).unwrap(), v.data, "{name}");
        assert_eq!(
            render_value(&cell["value"], &cx).unwrap().to_json(),
            cell["kv_returns"]
        );
    }
    let pm = cells("put_merge");
    assert_eq!(
        put_patch(&pm["templated_leaf"]["patch"], &cx)
            .unwrap()
            .to_json(),
        json!({"b": 2})
    );
    assert_eq!(
        put_patch(&pm["templated"]["patch"], &cx).unwrap().to_json(),
        json!({"b": 2})
    );
    // Keys are rendered as text, other leaves kept.
    assert_eq!(
        render_value(&json!({"{{ 'k' ~ 1 }}": [true, null, 1.5, "{{ 3 }}"]}), &cx)
            .unwrap()
            .to_json(),
        json!({"k1": [true, null, 1.5, 3]})
    );
}

/// w9c `set_coercion`, every cell, and every `types` cell's `set`: the value
/// rendered, coerced by its kvType, and the bytes, type or message.
#[test]
fn every_set_coercion_matches_the_artifact() {
    let cx = RenderContext::new();
    let mut examined = 0;
    let mut through_the_handler = 0;
    for (name, cell) in cells("set_coercion").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let input = &cell["input"];
        let raw = input["value"].as_str().unwrap();
        let kind = input["kvType"].as_str().unwrap();
        // A kvType that is not one of the seven names is refused by the
        // handler, before the value: `kv_type_refusals_run_through_the_handler`.
        let Some(t) = KvType::parse(kind) else {
            assert!(
                ["type_lower", "type_bad"].contains(&name.as_str()),
                "{name}"
            );
            through_the_handler += 1;
            continue;
        };
        let got: Result<KvValue, String> =
            set_value(&json!(raw), Some(t), &cx).map_err(|e| e.to_string());
        match cell["outcome"].as_str().unwrap() {
            "ok" => {
                let v = got.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(json!(bytes(&v)), cell["ion_bytes"], "{name}");
                assert_eq!(json!(v.kind.as_str()), cell["stored_type"], "{name}");
            }
            "error" => {
                let m = got.err().unwrap_or_else(|| panic!("{name}: accepted"));
                if let Some(want) = cell["message"].as_str() {
                    assert_eq!(m, want, "{name}");
                }
                if let Some(p) = cell["message_prefix"].as_str() {
                    assert!(m.starts_with(p), "{name}: {m}");
                }
                for frag in cell["message_contains"].as_array().into_iter().flatten() {
                    assert!(m.contains(frag.as_str().unwrap()), "{name}: {m}");
                }
            }
            other => panic!("{name}: {other}"),
        }
        examined += 1;
    }
    assert_eq!((examined, through_the_handler), (61, 2));
    let mut types = 0;
    for (name, cell) in cells("types").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let set = &cell["set"];
        let t = set["kvType"].as_str().map(|t| KvType::parse(t).unwrap());
        let v = set_value(&set["value"], t, &cx).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(json!(bytes(&v)), cell["ion_bytes"], "{name}");
        types += 1;
    }
    assert_eq!(types, 39);
    // A flow's number is never a decimal: `1.5` is the float `1.5e0`.
    let v = set_value(&json!("1.5"), Some(KvType::Number), &cx).unwrap();
    assert_eq!(v.data, KvData::Number(KvNumber::Float(1.5)));
}

/// The amendment's 10,000-deep value: refused as the author's, no abort.
/// The raw value is built iteratively (never `json!`, which recurses), judged
/// on a 2 MiB thread (tokio's blocking-pool size), and dropped on a big one.
#[test]
fn a_raw_value_10000_deep_is_refused_without_recursing() {
    fn deep(n: usize) -> Value {
        let mut v = json!(1);
        for i in 0..n {
            v = if i % 2 == 0 {
                Value::Array(vec![v])
            } else {
                let mut m = serde_json::Map::new();
                m.insert("k".into(), v);
                Value::Object(m)
            };
        }
        v
    }
    let v = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| deep(10_000))
        .unwrap()
        .join()
        .unwrap();
    let (v, set, put) = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let cx = RenderContext::new();
            let set = set_value(&v, None, &cx).map(drop);
            let put = put_patch(&v, &cx).map(drop);
            (v, set, put)
        })
        .unwrap()
        .join()
        .unwrap();
    for r in [set, put] {
        match r {
            Err(NsError::Author(m)) => assert!(m.contains("deeper than 100"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || drop(v))
        .unwrap()
        .join()
        .unwrap();
}

/// The amendment's test through the tasks: a RENDERED value 10,000 deep
/// (and one 120 deep, which the renderer builds and the KV store refuses)
/// fails `Set` and `Put` as the author's error, with no abort. No store call
/// is reached, so the pool never connects.
#[tokio::test(flavor = "multi_thread")]
async fn a_rendered_value_10000_deep_fails_set_and_put_as_the_authors() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    let t = Tasks::new(&pool, Some(capped(storage::Caps::default())));
    for depth in [10_000, 120] {
        let text = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        let value = format!("{{{{ fromJson('{text}') }}}}");
        for type_id in [kvt::SET, kvt::PUT] {
            let m = t
                .go(
                    Uuid::new_v4(),
                    "w9c.deep",
                    type_id,
                    json!({"key": "d", "value": value}),
                )
                .await
                .failed();
            assert!(
                m.contains("deeper than 100") || m.contains("recursion limit exceeded"),
                "{type_id} {depth}: {m}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Set and Get
// ---------------------------------------------------------------------------

/// w9c `set_coercion.type_lower` and `type_bad` through the `Set` handler
/// (Task 5 review I2): a kvType that is not one of the seven names fails the
/// task naming it, BEFORE the key rule and the write rule (a bad key in
/// another namespace still fails on the kvType), and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn kv_type_refusals_run_through_the_handler() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let mut examined = 0;
    for name in ["type_lower", "type_bad"] {
        let cell = &cells("set_coercion")[name];
        let kind = cell["input"]["kvType"].as_str().unwrap();
        let value = cell["input"]["value"].clone();
        assert_eq!(cell["outcome"], json!("error"), "{name}");
        for spec in [
            json!({"key": "k", "value": value, "kvType": kind}),
            json!({"key": "bad key", "value": value, "kvType": kind, "namespace": "w9c.else"}),
        ] {
            let m = t.go(ws, "w9c.kvt", kvt::SET, spec).await.failed();
            assert_eq!(
                m,
                format!(
                    "`kvType` must be one of STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION \
                     or JSON, and it is {kind:?}"
                ),
                "{name}"
            );
            for frag in cell["message_contains"].as_array().unwrap() {
                assert!(m.contains(frag.as_str().unwrap()), "{name}: {m}");
            }
        }
        examined += 1;
    }
    assert_eq!(examined, 2);
    assert_eq!(count_rows(&db, ws, "w9c.kvt").await, 0);
    assert_eq!(count_rows(&db, ws, "w9c.else").await, 0);
}

/// A run that is not a flow's task run has no namespace of its own: each
/// of the six tasks fails naming that, and nothing is read or written
/// (Task 5 review I2).
#[tokio::test(flavor = "multi_thread")]
async fn each_task_outside_a_flow_run_fails_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    seed(&db, &fs, ws, "w9c.nf", "k", string("v")).await;
    let want = "this run is not a task run of a flow's execution, so it has no namespace of its \
                own to read or write the KV store from";
    for (type_id, spec) in [
        (
            kvt::SET,
            json!({"key": "k", "value": "v", "namespace": "w9c.nf"}),
        ),
        (kvt::GET, json!({"key": "k", "namespace": "w9c.nf"})),
        (kvt::GET_KEYS, json!({"namespace": "w9c.nf"})),
        (kvt::DELETE, json!({"key": "k", "namespace": "w9c.nf"})),
        (
            kvt::PUT,
            json!({"key": "k", "value": {"b": 1}, "namespace": "w9c.nf"}),
        ),
        (
            kvt::PURGE_KV,
            json!({"behavior": {"type": "version", "keepAmount": 0}}),
        ),
    ] {
        let mut task = input(ws, "w9c.nf", type_id, spec);
        task.render = RenderContext::new();
        assert_eq!(t.run(&task).await.failed(), want, "{type_id}");
    }
    assert_eq!(rows(&db, ws, "w9c.nf", "k").await.len(), 1);
    assert_eq!(latest(&db, &fs, ws, "w9c.nf", "k").await, json!("v"));
}

/// Two YAML keys that render to one key are refused naming it, for `Set` and
/// `Put` alike, where one value would silently be lost (Task 5 review M2).
#[test]
fn two_keys_rendering_to_one_key_are_refused_naming_it() {
    let cx = RenderContext::new();
    let raw = json!({"x": {"{{ 'a' }}": 1, "a": 2}});
    let want = "`value.x`: two keys render to the same key 'a', so one value would be lost; \
                render them to different keys";
    for got in [
        render_value(&raw, &cx).map(|_| ()),
        set_value(&raw, None, &cx).map(|_| ()),
        put_patch(&raw, &cx).map(|_| ()),
    ] {
        assert!(
            matches!(&got, Err(NsError::Author(m)) if m == want),
            "{got:?}"
        );
    }
    // Distinct keys are kept, each with its value.
    assert_eq!(
        render_value(&json!({"{{ 'a' }}": 1, "b": 2}), &cx)
            .unwrap()
            .to_json(),
        json!({"a": 1, "b": 2})
    );
}

/// The typed render never changes what an expression means by wrapping it
/// (Task 5 review M3): `{{ 1) ~ (2 }}` is a syntax error, as in Pebble, not
/// the number 12; a top-level comma is not a tuple. Brackets inside a string
/// and balanced ones are kept.
#[test]
fn render_typed_fails_an_expression_the_wrapper_would_change() {
    let cx = RenderContext::new();
    for template in ["{{ 1) ~ (2 }}", "{{ 1 ] ~ [ 2 }}", "{{ 1, 2 }}", "{{ (1 }}"] {
        let e = render_typed(template, &cx).unwrap_err().to_string();
        assert!(e.contains("syntax error"), "{template}: {e}");
    }
    for (template, want) in [
        ("{{ (1) ~ (2) }}", json!("12")),
        ("{{ ')' ~ '(' }}", json!(")(")),
        ("{{ [1, 2] }}", json!([1, 2])),
        ("{{ {'a': 1, 'b': [2, 3]} }}", json!({"a": 1, "b": [2, 3]})),
    ] {
        assert_eq!(rendered_typed(template, &cx), want, "{template}");
    }
}

/// w9c `types`: Set (as the cell), then Get's `outputs.value` JSON-equal
/// `get_value`, and the stored bytes `ion_bytes`.
#[tokio::test(flavor = "multi_thread")]
async fn set_then_get_round_trips_every_type() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.types";
    let mut examined = 0;
    for (name, cell) in cells("types").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let mut spec = json!({"key": name, "value": cell["set"]["value"]});
        if let Some(k) = cell["set"]["kvType"].as_str() {
            spec["kvType"] = json!(k);
        }
        assert_eq!(
            t.go(ws, ns, kvt::SET, spec).await,
            Ran::Ok(json!({})),
            "{name}"
        );
        assert_eq!(
            json!(latest_bytes(&db, &fs, ws, ns, name).await),
            cell["ion_bytes"],
            "{name}"
        );
        let got = t.go(ws, ns, kvt::GET, json!({"key": name})).await.ok();
        assert!(
            same(&got["value"], &cell["get_value"]),
            "{name}: {} vs {}",
            got["value"],
            cell["get_value"]
        );
        examined += 1;
    }
    assert_eq!(examined, 39);
}

/// w9c `overwrite`: over a live key fails with Kestra's message; over a
/// deleted, expired or missing key it writes; `true` over a live key
/// writes revision 2.
#[tokio::test(flavor = "multi_thread")]
async fn set_with_overwrite_false_over_a_live_key_fails_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.ow";
    let c = cells("overwrite");
    seed(&db, &fs, ws, ns, "live", string("v1")).await;
    seed(&db, &fs, ws, ns, "del", string("v1")).await;
    kv_store::delete(&db.pool, &api(ws, ns, Access::Write), &key("del"), USER)
        .await
        .unwrap();
    seed(&db, &fs, ws, ns, "exp", string("v1")).await;
    expire_latest(&db, ws, ns, "exp").await;
    let set =
        |k: &str, v: &str, overwrite: bool| json!({"key": k, "value": v, "overwrite": overwrite});
    let m = t
        .go(ws, ns, kvt::SET, set("live", "v2", false))
        .await
        .failed();
    assert_eq!(json!(m), c["over_live"]["message"]);
    assert_eq!(rows(&db, ws, ns, "live").await.len(), 1, "nothing written");
    for (k, cell) in [("del", "over_deleted"), ("exp", "over_expired")] {
        t.go(ws, ns, kvt::SET, set(k, "v2", false)).await.ok();
        assert_eq!(rows(&db, ws, ns, k).await, want_rows(&c[cell]), "{cell}");
    }
    t.go(ws, ns, kvt::SET, set("never", "v", false)).await.ok();
    t.go(ws, ns, kvt::SET, set("live", "v3", true)).await.ok();
    let r = rows(&db, ws, ns, "live").await;
    assert_eq!(json!(r.last().unwrap().0), c["true_over_live"]["revision"]);
    for (k, cell) in &[("live", "live"), ("del", "del"), ("exp", "exp")] {
        let want = &c["api_after"][cell];
        assert_eq!(
            json!(latest(&db, &fs, ws, ns, k).await),
            want["value"],
            "{k}"
        );
        let r = rows(&db, ws, ns, k).await;
        assert_eq!(json!(r.last().unwrap().0), want["revision"], "{k}");
    }
}

/// w9c `ttl`: zero and negative refused with the measured message, a
/// malformed one naming it and `ttl`; the others expire after their ttl.
#[tokio::test(flavor = "multi_thread")]
async fn set_ttl_zero_negative_and_malformed_as_measured() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.ttl";
    let mut examined = 0;
    for (name, cell) in cells("ttl").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let k = format!("ttl_{}", name.replace('-', "neg"));
        let mut spec = json!({"key": k, "value": "v"});
        if !cell["ttl"].is_null() {
            spec["ttl"] = cell["ttl"].clone();
        }
        let got = t.go(ws, ns, kvt::SET, spec).await;
        match cell["outcome"].as_str().unwrap() {
            "error" => {
                let m = got.failed();
                if let Some(want) = cell["message"].as_str() {
                    assert_eq!(m, want, "{name}");
                }
                for f in cell["message_contains"].as_array().into_iter().flatten() {
                    assert!(m.contains(f.as_str().unwrap()), "{name}: {m}");
                }
                assert!(rows(&db, ws, ns, &k).await.is_empty(), "{name}");
            }
            _ => {
                got.ok();
                let after: Option<f64> = sqlx::query_scalar(
                    "SELECT extract(epoch FROM expires_at - created_at)::float8 \
                       FROM workflow.kv_entries \
                      WHERE workspace_id = $1 AND namespace = $2 AND key = $3",
                )
                .bind(ws)
                .bind(ns)
                .bind(&k)
                .fetch_one(&db.pool)
                .await
                .unwrap();
                match cell["expires_after_write_s"].as_f64() {
                    None => assert_eq!(after, None, "{name}"),
                    Some(s) => {
                        let a = after.unwrap();
                        assert!((a - s).abs() <= 1.0, "{name}: {a} vs {s}");
                    }
                }
            }
        }
        examined += 1;
    }
    assert_eq!(examined, 8);
    // Over 1,000 years is the author's, naming ttl.
    let m = t
        .go(
            ws,
            ns,
            kvt::SET,
            json!({"key": "far", "value": "v", "ttl": "P365251D"}),
        )
        .await
        .failed();
    assert!(m.contains("ttl") && m.contains("1,000 years"), "{m}");
    assert_eq!(parse_ttl("PT1S", Ttl::Task).unwrap().num_seconds(), 1);
}

/// w9c `write_scope`: a write, delete or purge of another namespace fails
/// naming it and touches nothing; a read of a namespace with no KV answers
/// `{}`, no `value` field. Mutation (a) (`Set` with `Access::Read`) dies here.
#[tokio::test(flavor = "multi_thread")]
async fn set_in_another_namespace_is_refused_naming_it_and_writes_nothing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    seed(&db, &fs, ws, "w9c.api", "k", string("api")).await;
    let mut examined = 0;
    for (name, cell) in cells("write_scope").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let own = cell["flow_namespace"].as_str().unwrap();
        let target = cell["target"].as_str().unwrap();
        let (type_id, spec) = match name.split('_').next().unwrap() {
            "set" => (
                kvt::SET,
                json!({"key": "k", "value": "x", "namespace": target}),
            ),
            "put" => (
                kvt::PUT,
                json!({"key": "k", "value": {"a": 1}, "namespace": target}),
            ),
            "delete" => (kvt::DELETE, json!({"key": "k", "namespace": target})),
            "get" => (kvt::GET, json!({"key": "k", "namespace": target})),
            "purge" => (
                kvt::PURGE_KV,
                json!({"namespaces": [target], "behavior": {"type": "version", "keepAmount": 0}}),
            ),
            other => panic!("{other}"),
        };
        let before = rows(&db, ws, target, "k").await;
        let got = t.go(ws, own, type_id, spec).await;
        match cell["outcome"].as_str().unwrap() {
            "error" => {
                let m = got.failed();
                for f in cell["message_contains"].as_array().unwrap() {
                    assert!(m.contains(f.as_str().unwrap()), "{name}: {m}");
                }
                // THE rule refused it (not a store check behind it).
                let rule = if type_id == kvt::PURGE_KV {
                    "a flow purges only its own namespace's KV store"
                } else {
                    "a flow writes only its own namespace's KV store"
                };
                assert!(m.contains(rule), "{name}: {m}");
                assert_eq!(
                    rows(&db, ws, target, "k").await,
                    before,
                    "{name}: untouched"
                );
            }
            _ => assert_eq!(got.ok(), cell["outputs"], "{name}"),
        }
        examined += 1;
    }
    assert_eq!(examined, 8);
    assert_eq!(count_rows(&db, ws, "w9c.wsp.ws").await, 0);
    assert_eq!(latest(&db, &fs, ws, "w9c.api", "k").await, json!("api"));
}

/// `max_read_bytes` caps a value's encoded bytes; nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn set_of_a_value_over_max_read_bytes_fails_naming_the_cap() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _)) = setup().await else {
        return;
    };
    let fs = capped(storage::Caps {
        max_read_bytes: 4096,
        ..storage::Caps::default()
    });
    let t = Tasks::new(&db.pool, Some(fs));
    let m = t
        .go(
            ws,
            "w9c.cap",
            kvt::SET,
            json!({"key": "big", "value": "x".repeat(5000)}),
        )
        .await
        .failed();
    assert!(m.contains("max_read_bytes") && m.contains("4096"), "{m}");
    assert_eq!(count_rows(&db, ws, "w9c.cap").await, 0);
    t.go(
        ws,
        "w9c.cap",
        kvt::SET,
        json!({"key": "fits", "value": "x".repeat(4000)}),
    )
    .await
    .ok();
}

/// A rendered list of 200 000 zeros: the dense-value limit (163 840 values
/// at the default cap), named; nothing written by either task.
#[tokio::test(flavor = "multi_thread")]
async fn set_and_put_of_a_value_over_the_dense_limit_fail_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let zeros = format!("[{}]", vec!["0"; 200_000].join(","));
    let value = format!("{{{{ fromJson('{zeros}') }}}}");
    for type_id in [kvt::SET, kvt::PUT] {
        let m = t
            .go(
                ws,
                "w9c.dense",
                type_id,
                json!({"key": "d", "value": value}),
            )
            .await
            .failed();
        assert!(m.contains("more than 163840 values"), "{type_id}: {m}");
    }
    assert_eq!(count_rows(&db, ws, "w9c.dense").await, 0);
    // The control: 100 000 fit.
    let fits = format!("{{{{ fromJson('[{}]') }}}}", vec!["0"; 100_000].join(","));
    t.go(
        ws,
        "w9c.dense",
        kvt::SET,
        json!({"key": "d", "value": fits}),
    )
    .await
    .ok();
}

/// The amendment (fix round 3): a number of 1,001 digits is refused naming
/// the limit, as `TooLarge` (mutation (j) dies at the class assertion); a
/// JSON list holding one too; 1,000 digits write and read back exactly.
#[tokio::test(flavor = "multi_thread")]
async fn a_set_of_a_1001_digit_number_fails_naming_the_limit() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let cx = RenderContext::new();
    let n1001 = "7".repeat(1001);
    for (raw, kind) in [
        (json!(n1001), KvType::Number),
        (json!(format!("[{n1001}]")), KvType::Json),
    ] {
        match set_value(&raw, Some(kind), &cx) {
            Err(NsError::TooLarge(m)) => assert!(m.contains("1000 digits"), "{m}"),
            other => panic!("{kind}: {other:?}"),
        }
    }
    // The Invalid arm stays the author's.
    assert!(matches!(
        set_value(&json!("yes"), Some(KvType::Boolean), &cx),
        Err(NsError::Author(_))
    ));
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.digits";
    let m = t
        .go(
            ws,
            ns,
            kvt::SET,
            json!({"key": "n", "value": n1001, "kvType": "NUMBER"}),
        )
        .await
        .failed();
    assert!(m.contains("1000 digits"), "{m}");
    assert_eq!(count_rows(&db, ws, ns).await, 0);
    let n1000 = "7".repeat(1000);
    t.go(
        ws,
        ns,
        kvt::SET,
        json!({"key": "n", "value": n1000, "kvType": "NUMBER"}),
    )
    .await
    .ok();
    assert_eq!(latest_bytes(&db, &fs, ws, ns, "n").await, n1000);
    let got = t.go(ws, ns, kvt::GET, json!({"key": "n"})).await.ok();
    assert_eq!(got["value"].to_string(), n1000);
}

/// An API-stored decimal survives a `Put`; `Get` outputs its digits.
#[tokio::test(flavor = "multi_thread")]
async fn put_over_an_api_decimal_keeps_it_and_get_outputs_its_digits() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.dec";
    let value = KvValue {
        kind: KvType::Json,
        data: KvData::Struct(vec![(
            "a".into(),
            KvData::Number(KvNumber::Decimal("1.5".into())),
        )]),
    };
    seed(&db, &fs, ws, ns, "d", value).await;
    t.go(ws, ns, kvt::PUT, json!({"key": "d", "value": {"b": 2}}))
        .await
        .ok();
    assert_eq!(latest_bytes(&db, &fs, ws, ns, "d").await, "{a:1.5,b:2}");
    let got = t.go(ws, ns, kvt::GET, json!({"key": "d"})).await.ok();
    assert_eq!(got["value"].to_string(), r#"{"a":1.5,"b":2}"#);
}

/// The fixture's `inheritance._setup`, each value `{key}@{label}`.
async fn seed_inheritance(db: &Db, fs: &FileStore, ws: Uuid) {
    let label = |ns: &str| -> String {
        match ns.strip_prefix("w9c.") {
            Some(rest) => rest.strip_prefix("inh.").unwrap_or(rest).to_string(),
            None => ns.to_string(),
        }
    };
    const DEEP: [&str; 6] = ["deep1", "deep2", "deep3", "deep4", "deep5", "deep6"];
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
    live.extend(DEEP.map(|k| ("w9c.inh", k)));
    for (ns, k) in live {
        seed(db, fs, ws, ns, k, string(&format!("{k}@{}", label(ns)))).await;
    }
    seed(db, fs, ws, "w9c.inh.a", "delmid", string("delmid@a")).await;
    kv_store::delete(
        &db.pool,
        &api(ws, "w9c.inh.a", Access::Write),
        &key("delmid"),
        USER,
    )
    .await
    .unwrap();
    seed(db, fs, ws, "w9c.inh.a", "midexp", string("midexp@a")).await;
    expire_latest(db, ws, "w9c.inh.a", "midexp").await;
    for k in DEEP {
        seed(db, fs, ws, "w9c.inh.a.b", k, string(&format!("{k}@a.b"))).await;
        expire_latest(db, ws, "w9c.inh.a.b", k).await;
    }
}

/// w9c `inheritance`'s task cells: `Get` walks only when its namespace is
/// the flow's own; `GetKeys` and `Delete` never walk. Mutation (b) (a walk
/// for any namespace) dies at `get_top_ns_parent_N1`.
///
/// `get_top_ns_parent_N1` is measured as `{}`: a missing key's output has
/// no `value` field, on Kestra and on egret (Task 5 fix round 1, Concern 2).
#[tokio::test(flavor = "multi_thread")]
async fn get_walks_only_when_its_namespace_is_the_flows() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    seed_inheritance(&db, &fs, ws).await;
    let t = Tasks::new(&db.pool, Some(fs));
    let mut examined = 0;
    for (name, cell) in cells("inheritance").as_object().unwrap() {
        let Some(task) = cell.get("task").filter(|t| {
            t["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("io.kestra.plugin.core.kv."))
        }) else {
            continue;
        };
        let mut spec = task.clone();
        let type_id = spec["type"].as_str().unwrap().to_string();
        spec.as_object_mut().unwrap().remove("type");
        let got = t.go(ws, "w9c.inh.a.b", &type_id, spec).await;
        match cell["outcome"].as_str().unwrap() {
            "ok" => assert_eq!(got.ok(), cell["outputs"], "{name}"),
            _ => assert_eq!(json!(got.failed()), cell["message"], "{name}"),
        }
        examined += 1;
    }
    assert_eq!(examined, 13);
}

/// Missing, soft-deleted and expired keys under each `errorOnMissing`.
#[tokio::test(flavor = "multi_thread")]
async fn get_missing_under_each_error_on_missing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.miss";
    seed(&db, &fs, ws, ns, "del", string("v")).await;
    kv_store::delete(&db.pool, &api(ws, ns, Access::Write), &key("del"), USER)
        .await
        .unwrap();
    seed(&db, &fs, ws, ns, "exp", string("v")).await;
    expire_latest(&db, ws, ns, "exp").await;
    for k in ["never", "del", "exp"] {
        let got = t.go(ws, ns, kvt::GET, json!({"key": k})).await.ok();
        assert_eq!(got, json!({}), "{k}");
        let got = t
            .go(ws, ns, kvt::GET, json!({"key": k, "errorOnMissing": false}))
            .await
            .ok();
        assert_eq!(got, json!({}), "{k}");
        let m = t
            .go(
                ws,
                ns,
                kvt::GET,
                json!({"key": k, "errorOnMissing": "{{ true }}"}),
            )
            .await
            .failed();
        assert_eq!(
            m,
            format!(
                "No value found for key '{k}' in namespace '{ns}' and `errorOnMissing` is set \
                 to true"
            )
        );
    }
    // A key that breaks the key rule is the author's.
    let m = t
        .go(ws, ns, kvt::GET, json!({"key": "bad key"}))
        .await
        .failed();
    assert!(m.starts_with("key 'bad key': Key must start"), "{m}");
}

// ---------------------------------------------------------------------------
// GetKeys
// ---------------------------------------------------------------------------

/// w9c `get_keys`, every cell, with every object read failing: `GetKeys`
/// reads rows only (mutation (g) dies here).
#[tokio::test(flavor = "multi_thread")]
async fn get_keys_lists_live_keys_in_the_artifacts_order_with_prefix() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    const GK: &str = "w9c.gk";
    for k in [
        "b", "a_b", "A", "a.1", "0", "a", "B", "aa", "a1", "a-b", "a.b", "Z9", "a0",
    ] {
        seed(&db, &fs, ws, GK, k, string("v")).await;
    }
    kv_store::delete(&db.pool, &api(ws, GK, Access::Write), &key("a_b"), USER)
        .await
        .unwrap();
    expire_latest(&db, ws, GK, "B").await;
    let c = cells("get_keys");
    OUTAGE.store(true, Ordering::SeqCst);
    let mut examined = 0;
    for name in [
        "all",
        "prefix_a",
        "prefix_A",
        "prefix_a.",
        "prefix_none",
        "prefix_empty",
        "other_ns_default_empty",
        "prefix_B_expired",
    ] {
        let cell = &c[name];
        let got = t.go(ws, GK, kvt::GET_KEYS, cell["props"].clone()).await;
        assert_eq!(got.ok(), cell["outputs"], "{name}");
        examined += 1;
    }
    OUTAGE.store(false, Ordering::SeqCst);
    let steps = c["after_rewrites"]["steps"].as_array().unwrap();
    seed(&db, &fs, ws, GK, "b", string("v2")).await;
    seed(&db, &fs, ws, GK, "0", string("v2")).await;
    let got = t.go(ws, GK, kvt::GET_KEYS, json!({})).await.ok();
    assert_eq!(got["keys"], steps[0]["keys"]);
    seed(&db, &fs, ws, GK, "a_b", string("v2")).await;
    let got = t.go(ws, GK, kvt::GET_KEYS, json!({})).await.ok();
    assert_eq!(got["keys"], steps[1]["keys"]);
    assert_eq!(examined + steps.len(), 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_keys_past_max_namespace_files_fails_naming_the_cap_and_count() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let small = capped(storage::Caps {
        max_namespace_files: 3,
        ..storage::Caps::default()
    });
    let t = Tasks::new(&db.pool, Some(small));
    let ns = "w9c.gkcap";
    for k in ["a", "b", "c"] {
        seed(&db, &fs, ws, ns, k, string("v")).await;
    }
    assert_eq!(
        t.go(ws, ns, kvt::GET_KEYS, json!({})).await.ok(),
        json!({"keys": ["a", "b", "c"]})
    );
    seed(&db, &fs, ws, ns, "d", string("v")).await;
    let m = t.go(ws, ns, kvt::GET_KEYS, json!({})).await.failed();
    assert!(
        m.contains("more than 3 keys")
            && m.contains("max_namespace_files")
            && m.contains("4 were seen"),
        "{m}"
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// w9c `delete`, every task cell: outputs, messages and rows.
#[tokio::test(flavor = "multi_thread")]
async fn delete_outputs_deleted_and_honours_error_on_missing() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    const NS: &str = "w9c.del";
    let w = api(ws, NS, Access::Write);
    for k in ["live_f", "live_t", "del_f", "del_t", "exp_f", "exp_t"] {
        seed(&db, &fs, ws, NS, k, string("v1")).await;
    }
    for k in ["del_f", "del_t"] {
        kv_store::delete(&db.pool, &w, &key(k), USER).await.unwrap();
    }
    for k in ["exp_f", "exp_t"] {
        expire_latest(&db, ws, NS, k).await;
    }
    for v in ["v1", "v2", "v3"] {
        seed(&db, &fs, ws, NS, "multi", string(v)).await;
    }
    let c = cells("delete");
    let objects_before = probe().0.list("", None).await.unwrap();
    let mut examined = 0;
    for (name, k, eom) in [
        ("live_eom_false", "live_f", false),
        ("live_eom_true", "live_t", true),
        ("missing_eom_false", "nope_f", false),
        ("missing_eom_true", "nope_t", true),
        ("deleted_eom_false", "del_f", false),
        ("deleted_eom_true", "del_t", true),
        ("expired_eom_false", "exp_f", false),
        ("expired_eom_true", "exp_t", true),
        ("multi_revisions", "multi", false),
    ] {
        let cell = &c[name];
        let got = t
            .go(
                ws,
                NS,
                kvt::DELETE,
                json!({"key": k, "errorOnMissing": eom}),
            )
            .await;
        match cell["outcome"].as_str().unwrap() {
            "ok" => assert_eq!(got.ok(), cell["outputs"], "{name}"),
            _ => assert_eq!(json!(got.failed()), cell["message"], "{name}"),
        }
        assert_eq!(rows(&db, ws, NS, k).await, want_rows(cell), "{name}");
        examined += 1;
    }
    // objects_after: a soft delete removes no object.
    let objects_after = probe().0.list("", None).await.unwrap();
    assert_eq!(
        objects_before.objects.len() - objects_after.objects.len(),
        c["objects_after"]["objects_removed_by_delete"]
            .as_u64()
            .unwrap() as usize
    );
    // rewrite_after_delete
    t.go(ws, NS, kvt::SET, json!({"key": "live_f", "value": "v2"}))
        .await
        .ok();
    let cell = &c["rewrite_after_delete"];
    let want: Vec<_> = cell["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["revision"].as_i64().unwrap() as i32,
                r["latest"].as_bool().unwrap(),
                r["deleted"].as_bool().unwrap(),
                !r["expires"].is_null(),
            )
        })
        .collect();
    assert_eq!(rows(&db, ws, NS, "live_f").await, want);
    assert_eq!(
        json!(latest(&db, &fs, ws, NS, "live_f").await),
        cell["get"]["value"]
    );
    assert_eq!(examined, 9);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_in_another_namespace_is_refused() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    seed(&db, &fs, ws, "w9c.other", "k", string("v")).await;
    for target in ["w9c.other", "w9c.own.child", "w9c"] {
        let m = t
            .go(
                ws,
                "w9c.own",
                kvt::DELETE,
                json!({"key": "k", "namespace": target}),
            )
            .await
            .failed();
        assert!(
            m.contains(&format!("'{target}'")) && m.contains("writes only"),
            "{m}"
        );
    }
    assert_eq!(latest(&db, &fs, ws, "w9c.other", "k").await, json!("v"));
    assert_eq!(
        rows(&db, ws, "w9c.other", "k").await,
        vec![(1, true, false, false)]
    );
}

// ---------------------------------------------------------------------------
// Put
// ---------------------------------------------------------------------------

/// Every w9c `put_merge` cell through the task (the templates included):
/// seeded as the cell's state, then `Put` with its patch and
/// `errorOnMissing`; outcome, bytes, result, message and metadata as the
/// cell; an error writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn put_matches_every_merge_cell() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.put";
    let w = api(ws, ns, Access::Write);
    let mut examined = 0;
    for (name, cell) in cells("put_merge").as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let k = cell["rows_after"][0]["key"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                cell["message"].as_str().and_then(|m| {
                    m.strip_prefix("No value found for key '")
                        .and_then(|r| r.split_once('\''))
                        .map(|(k, _)| k.to_string())
                })
            })
            .unwrap_or_else(|| name.clone());
        let state = cell["existing_state"].as_str().unwrap();
        if state != "missing" {
            let new = NewValue {
                value: typed(&cell["existing"]),
                description: cell["existing_description"].as_str().map(str::to_string),
                ttl: cell["existing_ttl"].as_str().map(|t| {
                    parse_ttl(t, Ttl::Task)
                        .unwrap()
                        .max(chrono::TimeDelta::seconds(60))
                }),
                overwrite: true,
            };
            kv_store::set(&db.pool, &fs, &w, &key(&k), new, USER)
                .await
                .unwrap();
            match state {
                "live" => {}
                "expired" => expire_latest(&db, ws, ns, &k).await,
                "deleted" => {
                    kv_store::delete(&db.pool, &w, &key(&k), USER)
                        .await
                        .unwrap();
                }
                other => panic!("{name}: state {other}"),
            }
        }
        let before = rows(&db, ws, ns, &k).await;
        let mut spec = json!({"key": k, "value": cell["patch"]});
        if !cell["errorOnMissing"].is_null() {
            spec["errorOnMissing"] = cell["errorOnMissing"].clone();
        }
        let got = t.go(ws, ns, kvt::PUT, spec).await;
        match cell["outcome"].as_str().unwrap() {
            "ok" => {
                assert_eq!(got, Ran::Ok(json!({})), "{name}");
                let r = rows(&db, ws, ns, &k).await;
                if let Some(rev) = cell["revision_after"].as_i64() {
                    assert_eq!(r.last().unwrap().0 as i64, rev, "{name}");
                }
                if let Some(ion) = cell["ion_bytes"].as_str() {
                    assert_eq!(latest_bytes(&db, &fs, ws, ns, &k).await, ion, "{name}");
                }
                assert_eq!(
                    serde_json::to_string(&latest(&db, &fs, ws, ns, &k).await).unwrap(),
                    serde_json::to_string(&cell["result"]).unwrap(),
                    "{name}"
                );
                if cell.get("description_after").is_some() {
                    let d: Option<String> = sqlx::query_scalar(
                        "SELECT description FROM workflow.kv_entries WHERE workspace_id = $1 \
                           AND namespace = $2 AND key = $3 ORDER BY revision DESC LIMIT 1",
                    )
                    .bind(ws)
                    .bind(ns)
                    .bind(&k)
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
                    assert_eq!(json!(d), cell["description_after"], "{name}");
                }
                if cell.get("expires_after").is_some() {
                    let e = r.last().unwrap().3;
                    assert_eq!(json!(e.then_some("<ts>")), cell["expires_after"], "{name}");
                }
            }
            "error" => {
                assert_eq!(json!(got.failed()), cell["message"], "{name}");
                assert_eq!(
                    rows(&db, ws, ns, &k).await,
                    before,
                    "{name}: nothing written"
                );
            }
            other => panic!("{name}: {other}"),
        }
        examined += 1;
    }
    assert_eq!(examined, 47);
}

// ---------------------------------------------------------------------------
// PurgeKV
// ---------------------------------------------------------------------------

/// `x`, expired, in each of `namespaces`.
async fn seed_expired(db: &Db, ws: Uuid, namespaces: &[&str], k: &str) {
    for ns in namespaces {
        row(db, ws, ns, k, 1, 10.0, true, false).await;
    }
}

/// The default: the flow's own namespace and its children on a dot
/// boundary (`a.bc` is not a child of `a.b`; mutation (e) dies here), and
/// w9c `purge.default_namespaces`.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_default_purges_expired_keys_of_own_and_children() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    seed_expired(&db, ws, &["a.b", "a.b.c", "a.bc", "a"], "x").await;
    row(&db, ws, "a.b", "live", 1, 10.0, false, false).await;
    let got = t.go(ws, "a.b", kvt::PURGE_KV, json!({})).await.ok();
    assert_eq!(got, json!({"size": 2}));
    for (ns, left) in [("a.b", 1), ("a.b.c", 0), ("a.bc", 1), ("a", 1)] {
        assert_eq!(count_rows(&db, ws, ns).await, left, "{ns}");
    }
    let cell = &cells("purge")["default_namespaces"];
    seed_expired(&db, ws, &["w9c.pd", "w9c.pd.c", "w9c.put"], "pdef").await;
    let got = t
        .go(ws, "w9c.pd", kvt::PURGE_KV, cell["props"].clone())
        .await
        .ok();
    assert_eq!(got, cell["outputs"]);
    for ns in cell["purged_namespaces"].as_array().unwrap() {
        assert_eq!(count_rows(&db, ws, ns.as_str().unwrap()).await, 0, "{ns}");
    }
    for ns in cell["untouched"].as_array().unwrap() {
        assert_eq!(count_rows(&db, ws, ns.as_str().unwrap()).await, 1, "{ns}");
    }
}

/// The fixture's `purge._setup` in `ns`; answers `t_mid`.
async fn seed_purge(db: &Db, ws: Uuid, ns: &str) -> chrono::DateTime<chrono::Utc> {
    for k in ["k3", "kd", "kx3"] {
        row(db, ws, ns, k, 1, 60.0, false, false).await;
        row(db, ws, ns, k, 2, 50.0, false, false).await;
        row(db, ws, ns, k, 3, 20.0, false, k == "kd").await;
    }
    row(db, ws, ns, "ke", 1, 60.0, false, false).await;
    row(db, ws, ns, "ke", 2, 30.0, true, false).await;
    row(db, ws, ns, "k1", 1, 60.0, false, false).await;
    sqlx::query_scalar("SELECT now() - interval '40 seconds'")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn surviving(db: &Db, ws: Uuid, ns: &str) -> Value {
    let mut out = serde_json::Map::new();
    for k in ["k1", "k3", "kd", "ke", "kx3"] {
        let r: Vec<i32> = rows(db, ws, ns, k).await.into_iter().map(|r| r.0).collect();
        if !r.is_empty() {
            out.insert(k.into(), json!(r));
        }
    }
    Value::Object(out)
}

/// The `behavior` of a cell's props, `<t_mid>` replaced.
fn purge_props(cell: &Value, t_mid: chrono::DateTime<chrono::Utc>) -> Value {
    let mut props = cell["props"].clone();
    if props["behavior"]["before"] == json!("<t_mid>") {
        props["behavior"]["before"] = json!(t_mid.to_rfc3339());
    }
    props
}

#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_expired_only_false_is_refused_naming_keep_amount_zero() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let cell = &cells("purge")["key_expiredOnly_false"];
    let t_mid = seed_purge(&db, ws, "w9c.pg").await;
    let m = t
        .go(ws, "w9c.pg", kvt::PURGE_KV, purge_props(cell, t_mid))
        .await
        .failed();
    for f in cell["message_contains"].as_array().unwrap() {
        assert!(m.contains(f.as_str().unwrap()), "{m}");
    }
    assert_eq!(
        surviving(&db, ws, "w9c.pg").await,
        cell["surviving_revisions"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_version_with_neither_property_is_refused_naming_both() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let c = cells("purge");
    let t_mid = seed_purge(&db, ws, "w9c.pg").await;
    let before = surviving(&db, ws, "w9c.pg").await;
    let cell = &c["version_neither"];
    let m = t
        .go(ws, "w9c.pg", kvt::PURGE_KV, purge_props(cell, t_mid))
        .await
        .failed();
    for f in cell["message_contains"].as_array().unwrap() {
        assert!(m.contains(f.as_str().unwrap()), "{m}");
    }
    assert_eq!(
        surviving(&db, ws, "w9c.pg").await,
        cell["surviving_revisions"]
    );
    let cell = &c["version_before_keep1"];
    let m = t
        .go(ws, "w9c.pg", kvt::PURGE_KV, purge_props(cell, t_mid))
        .await
        .failed();
    assert_eq!(json!(m), cell["message"]);
    assert_eq!(surviving(&db, ws, "w9c.pg").await, before);
}

/// Every w9c `purge` behaviour cell through the task, each in its own
/// namespace: `size` and the surviving revisions exact.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_version_keep_amount_before_and_zero() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let c = cells("purge");
    let mut examined = 0;
    for name in [
        "key_default",
        "key_expiredOnly_true",
        "version_keep1",
        "version_keep0",
        "version_keep2",
        "version_before",
        "key_default_keyPattern",
        "version_keep1_keyPattern",
    ] {
        let cell = &c[name];
        let ns = format!("w9c.pg2.{}", name.to_lowercase().replace('_', ""));
        let t_mid = seed_purge(&db, ws, &ns).await;
        let got = t
            .go(ws, &ns, kvt::PURGE_KV, purge_props(cell, t_mid))
            .await
            .ok();
        assert_eq!(got, cell["outputs"], "{name}");
        assert_eq!(
            surviving(&db, ws, &ns).await,
            cell["surviving_revisions"],
            "{name}"
        );
        examined += 1;
    }
    assert_eq!(examined, 8);
}

/// w9c `purge.namespaces`, every cell: the namespaces purged as a set, and
/// `namespacePattern`'s `.` literal.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_key_pattern_and_namespace_pattern_are_wildcards() {
    let _turn = SERIAL.lock().await;
    let Some((db, _, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let mut examined = 0;
    for (name, cell) in cells("purge")["namespaces"].as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let (ws, _) = common::seed_workspace(&db).await;
        // The refusal cells carry no `seeded`: the premise's five.
        let seeded: Vec<&str> = cell["seeded"].as_object().map_or_else(
            || vec!["w9c.pp", "w9c.pp.a", "w9c.pp.a.b", "w9c.ppx", "w9c.p"],
            |m| m.keys().map(String::as_str).collect(),
        );
        seed_expired(&db, ws, &seeded, "x").await;
        let own = cell["flow_namespace"].as_str().unwrap_or("w9c.pp");
        let got = t.go(ws, own, kvt::PURGE_KV, cell["props"].clone()).await;
        if cell["outcome"] == json!("error") {
            let m = got.failed();
            for f in cell["message_contains"].as_array().unwrap() {
                assert!(m.contains(f.as_str().unwrap()), "{name}: {m}");
            }
        } else {
            got.ok();
        }
        let mut purged = BTreeSet::new();
        for ns in &seeded {
            if count_rows(&db, ws, ns).await == 0 {
                purged.insert(ns.to_string());
            }
        }
        let want: BTreeSet<String> = cell["purged_namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(purged, want, "{name}");
        examined += 1;
    }
    assert_eq!(examined, 8);
    // `.` is literal: `w9c.pp.a.b` does not take `w9c.pp.axb`.
    let (ws, _) = common::seed_workspace(&db).await;
    seed_expired(&db, ws, &["w9c.pp.a.b", "w9c.pp.axb"], "x").await;
    t.go(
        ws,
        "w9c.pp",
        kvt::PURGE_KV,
        json!({"namespacePattern": "w9c.pp.a.b"}),
    )
    .await
    .ok();
    assert_eq!(count_rows(&db, ws, "w9c.pp.a.b").await, 0);
    assert_eq!(count_rows(&db, ws, "w9c.pp.axb").await, 1);
    // keyPattern: `?` one character.
    seed_expired(&db, ws, &["w9c.pp"], "k1").await;
    seed_expired(&db, ws, &["w9c.pp"], "k12").await;
    let got = t
        .go(ws, "w9c.pp", kvt::PURGE_KV, json!({"keyPattern": "k?"}))
        .await
        .ok();
    assert_eq!(got, json!({"size": 1}));
    assert!(rows(&db, ws, "w9c.pp", "k1").await.is_empty());
    assert_eq!(rows(&db, ws, "w9c.pp", "k12").await.len(), 1);
}

/// A listed namespace outside the rule fails the task naming it, and
/// NOTHING is purged, not even the listed namespaces before it. The refused
/// namespace sorts AFTER the allowed ones (`w9c.owx` > `w9c.own.c`; the
/// purge runs in byte order), so a purge that authorized namespace by
/// namespace inside `run_purge` would purge `w9c.own` and `w9c.own.c` first:
/// the brief's mutation (f) dies here (Task 5 review I1).
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_refuses_a_listed_namespace_outside_the_rule_before_purging_anything() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let all = ["w9c.own", "w9c.own.c", "w9c.owx"];
    seed_expired(&db, ws, &all, "x").await;
    let m = t
        .go(
            ws,
            "w9c.own",
            kvt::PURGE_KV,
            json!({"namespaces": ["w9c.owx", "w9c.own.c", "w9c.own"]}),
        )
        .await
        .failed();
    assert!(m.contains("'w9c.owx'") && m.contains("purges only"), "{m}");
    for ns in all {
        assert_eq!(count_rows(&db, ws, ns).await, 1, "{ns}");
    }
    // The control: without the refused one, both are purged.
    let got = t
        .go(
            ws,
            "w9c.own",
            kvt::PURGE_KV,
            json!({"namespaces": ["w9c.own", "w9c.own.c"], "includeChildNamespaces": false}),
        )
        .await
        .ok();
    assert_eq!(got, json!({"size": 2}));
    for (ns, left) in [("w9c.own", 0), ("w9c.own.c", 0), ("w9c.owx", 1)] {
        assert_eq!(count_rows(&db, ws, ns).await, left, "{ns}");
    }
    // Both lists at once: Kestra's message.
    let m = t
        .go(
            ws,
            "w9c.own",
            kvt::PURGE_KV,
            json!({"namespaces": ["w9c.own"], "namespacePattern": "w9c.*"}),
        )
        .await
        .failed();
    assert!(m.contains("can't be used at the same time"), "{m}");
}

/// The 64-namespace cap (`MAX_NAMESPACES`), on both paths, counting
/// DISTINCT namespaces (Task 5 review I2, M6): 65 listed fails naming 65
/// before anything is read; 65 entries naming one namespace is one; a
/// default scope resolving to 65 (own + 64 children holding KV) fails naming
/// 65, and one resolving to 64 purges. Nothing is purged by a refusal.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_covers_at_most_64_distinct_namespaces_on_either_path() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let children: Vec<String> = (0..64).map(|i| format!("w9c.cap.c{i:02}")).collect();
    let refs: Vec<&str> = children.iter().map(String::as_str).collect();
    seed_expired(&db, ws, &refs, "x").await;
    seed_expired(&db, ws, &["w9c.cap"], "x").await;

    // Listed: 65 distinct (own + 64 children) is refused naming 65.
    let mut listed: Vec<String> = children.clone();
    listed.push("w9c.cap".to_string());
    let m = t
        .go(
            ws,
            "w9c.cap",
            kvt::PURGE_KV,
            json!({"namespaces": listed, "includeChildNamespaces": false}),
        )
        .await
        .failed();
    assert!(
        m.contains("`namespaces` names 65 namespaces, more than the 64"),
        "{m}"
    );
    // The default scope: own + 64 children = 65 is refused naming 65.
    let m = t.go(ws, "w9c.cap", kvt::PURGE_KV, json!({})).await.failed();
    assert!(
        m.contains("65 namespaces would be purged, more than the 64"),
        "{m}"
    );
    assert_eq!(count_rows(&db, ws, "w9c.cap").await, 1);
    for ns in &refs {
        assert_eq!(count_rows(&db, ws, ns).await, 1, "{ns}: untouched");
    }
    // 65 entries naming ONE namespace are one namespace.
    let one = vec!["w9c.cap"; 65];
    let got = t
        .go(
            ws,
            "w9c.cap",
            kvt::PURGE_KV,
            json!({"namespaces": one, "includeChildNamespaces": false}),
        )
        .await
        .ok();
    assert_eq!(got, json!({"size": 1}));
    // Own (now empty of KV) + 63 children holding KV = 64: purged.
    sqlx::query("DELETE FROM workflow.kv_entries WHERE workspace_id = $1 AND namespace = $2")
        .bind(ws)
        .bind("w9c.cap.c63")
        .execute(&db.pool)
        .await
        .unwrap();
    let got = t.go(ws, "w9c.cap", kvt::PURGE_KV, json!({})).await.ok();
    assert_eq!(got, json!({"size": 63}));
}

/// Every refusal of `behavior` (Task 5 review I2), through the task, each
/// naming what is wrong, with nothing purged.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_refuses_each_malformed_behavior_naming_it() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    seed_purge(&db, ws, "w9c.pb").await;
    let before = surviving(&db, ws, "w9c.pb").await;
    let key_only = "`behavior` of type \"key\" takes only `expiredOnly`; `keepAmount` and \
                    `before` belong to type \"version\"";
    let instant = "`behavior.before` must be an ISO 8601 instant such as 2025-01-01T00:00:00Z";
    for (behavior, want) in [
        (json!({"type": "key", "keepAmount": 1}), key_only),
        (
            json!({"type": "key", "before": "2025-01-01T00:00:00Z"}),
            key_only,
        ),
        (
            json!({"type": "version", "expiredOnly": true, "keepAmount": 1}),
            "`behavior` of type \"version\" takes `keepAmount` or `before`; `expiredOnly` \
             belongs to type \"key\"",
        ),
        (
            json!({"type": "foo"}),
            "`behavior.type` must be \"key\" or \"version\", and it is \"foo\"",
        ),
        (
            json!({}),
            "`behavior.type` must be \"key\" or \"version\", and it is missing",
        ),
        (
            json!({"type": "key", "expiredOnly": "maybe"}),
            "`behavior.expiredOnly` must be true or false, and it is \"maybe\"",
        ),
        (
            json!({"type": "version", "keepAmount": -1}),
            "`behavior.keepAmount` must be a whole number, 0 or more, and it is -1",
        ),
        (
            json!({"type": "version", "keepAmount": 1.5}),
            "`behavior.keepAmount` must be a whole number, 0 or more, and it is 1.5",
        ),
        (json!({"type": "version", "before": "yesterday"}), instant),
        (json!({"type": "version", "before": 5}), instant),
    ] {
        let m = t
            .go(
                ws,
                "w9c.pb",
                kvt::PURGE_KV,
                json!({"behavior": behavior.clone()}),
            )
            .await
            .failed();
        assert!(m.starts_with(want), "{behavior}: {m}");
        assert_eq!(surviving(&db, ws, "w9c.pb").await, before, "{behavior}");
    }
}

/// `max_namespace_files` 3, 8 keys (some of two revisions): pages past one
/// page; a key holding more than the cap is refused naming it.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_pages_past_one_page() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _)) = setup().await else {
        return;
    };
    let small = capped(storage::Caps {
        max_namespace_files: 3,
        ..storage::Caps::default()
    });
    let t = Tasks::new(&db.pool, Some(small));
    let ns = "w9c.page";
    for i in 0..8 {
        let k = format!("k{i}");
        if i % 3 == 0 {
            row(&db, ws, ns, &k, 1, 20.0, false, false).await;
            row(&db, ws, ns, &k, 2, 10.0, true, false).await;
        } else {
            row(&db, ws, ns, &k, 1, 10.0, true, false).await;
        }
    }
    let got = t.go(ws, ns, kvt::PURGE_KV, json!({})).await.ok();
    assert_eq!(got, json!({"size": 11}));
    assert_eq!(count_rows(&db, ws, ns).await, 0);
    for rev in 1..=4 {
        row(&db, ws, ns, "many", rev, 10.0, rev == 4, false).await;
    }
    row(&db, ws, ns, "a", 1, 10.0, true, false).await;
    let m = t.go(ws, ns, kvt::PURGE_KV, json!({})).await.failed();
    assert!(
        m.contains("key 'many'") && m.contains("more than 3 revisions"),
        "{m}"
    );
    assert!(
        rows(&db, ws, ns, "a").await.is_empty(),
        "the key before it was purged"
    );
    // A key the pattern does not take is skipped, whatever its revisions.
    t.go(ws, ns, kvt::PURGE_KV, json!({"keyPattern": "z*"}))
        .await
        .ok();
}

/// w9c `purge.size`: revisions, not keys.
#[tokio::test(flavor = "multi_thread")]
async fn purge_kv_size_counts_as_kestra_does() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs));
    let cell = &cells("purge")["size"];
    assert_eq!(cell["counts"], json!("revisions"));
    seed_purge(&db, ws, "w9c.size").await;
    let got = t
        .go(
            ws,
            "w9c.size",
            kvt::PURGE_KV,
            json!({"behavior": {"type": "version", "keepAmount": 0}}),
        )
        .await
        .ok();
    assert_eq!(got, json!({"size": 12}));
}

// ---------------------------------------------------------------------------
// re-claims, outages, no storage
// ---------------------------------------------------------------------------

/// The same task run run twice (the lease lapsed and the run was re-claimed
/// after it committed): one revision, `deleted: true` again, and PurgeKV's
/// size counts only what its re-claim purged (mutation (h) dies here).
#[tokio::test(flavor = "multi_thread")]
async fn a_reclaimed_set_put_delete_and_purge_are_idempotent() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    let ns = "w9c.reclaim";
    let set = input(
        ws,
        ns,
        kvt::SET,
        json!({"key": "s", "value": "{{ 1 }}", "overwrite": false}),
    );
    for _ in 0..2 {
        assert_eq!(t.run(&set).await, Ran::Ok(json!({})));
    }
    assert_eq!(rows(&db, ws, ns, "s").await.len(), 1);
    let put = input(ws, ns, kvt::PUT, json!({"key": "s", "value": {"b": 2}}));
    t.go(ws, ns, kvt::SET, json!({"key": "s", "value": {"a": 1}}))
        .await
        .ok();
    for _ in 0..2 {
        assert_eq!(t.run(&put).await, Ran::Ok(json!({})));
    }
    assert_eq!(rows(&db, ws, ns, "s").await.len(), 3);
    assert_eq!(latest(&db, &fs, ws, ns, "s").await, json!({"a": 1, "b": 2}));
    let del = input(
        ws,
        ns,
        kvt::DELETE,
        json!({"key": "s", "errorOnMissing": true}),
    );
    for _ in 0..2 {
        assert_eq!(t.run(&del).await, Ran::Ok(json!({"deleted": true})));
    }
    // Another task run sees it missing.
    assert_eq!(
        t.go(ws, ns, kvt::DELETE, json!({"key": "s"})).await,
        Ran::Ok(json!({"deleted": false}))
    );
    let purge = input(
        ws,
        ns,
        kvt::PURGE_KV,
        json!({"behavior": {"type": "version", "keepAmount": 1}}),
    );
    assert_eq!(t.run(&purge).await, Ran::Ok(json!({"size": 3})));
    assert_eq!(t.run(&purge).await, Ran::Ok(json!({"size": 0})));
}

/// A store or database outage is the worker's (`Err`): the run is
/// re-claimed with no attempt spent. Every task over a closed pool; `Set`,
/// `Get` and `Put` over a failing store.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_or_database_outage_in_any_task_is_a_worker_error() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.outage";
    seed(&db, &fs, ws, ns, "k", typed(&json!({"a": 1}))).await;
    let dead = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pool.connect_options()).clone())
        .await
        .unwrap();
    dead.close().await;
    let closed = Tasks::new(&dead, Some(fs.clone()));
    for (type_id, spec) in [
        (kvt::SET, json!({"key": "k", "value": "v"})),
        (kvt::GET, json!({"key": "k"})),
        (kvt::GET_KEYS, json!({})),
        (kvt::DELETE, json!({"key": "k"})),
        (kvt::PUT, json!({"key": "k", "value": {"b": 1}})),
        (kvt::PURGE_KV, json!({})),
    ] {
        let task = input(ws, ns, type_id, spec);
        let r = closed.handler(type_id).run(&task).await;
        assert!(r.is_err(), "{type_id}: {r:?}");
    }
    let t = Tasks::new(&db.pool, Some(fs.clone()));
    OUTAGE.store(true, Ordering::SeqCst);
    for (type_id, spec) in [
        (kvt::GET, json!({"key": "k"})),
        (kvt::PUT, json!({"key": "k", "value": {"b": 1}})),
    ] {
        let task = input(ws, ns, type_id, spec);
        let e = t.handler(type_id).run(&task).await.expect_err(type_id);
        let m = e.to_string();
        assert!(
            m.contains("unavailable") && !m.contains("s3://"),
            "{type_id}: {m}"
        );
    }
    OUTAGE.store(false, Ordering::SeqCst);
    WRITE_OUTAGE.store(true, Ordering::SeqCst);
    let task = input(ws, ns, kvt::SET, json!({"key": "k", "value": "v"}));
    assert!(t.handler(kvt::SET).run(&task).await.is_err());
    WRITE_OUTAGE.store(false, Ordering::SeqCst);
    assert_eq!(rows(&db, ws, ns, "k").await.len(), 1, "nothing written");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_storage_each_task_fails_naming_storage() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _)) = setup().await else {
        return;
    };
    let t = Tasks::new(&db.pool, None);
    for (type_id, spec) in [
        (kvt::SET, json!({"key": "k", "value": "v"})),
        (kvt::GET, json!({"key": "k"})),
        (kvt::GET_KEYS, json!({})),
        (kvt::DELETE, json!({"key": "k"})),
        (kvt::PUT, json!({"key": "k", "value": {"b": 1}})),
        (kvt::PURGE_KV, json!({})),
    ] {
        let o = t
            .handler(type_id)
            .run(&input(ws, "w9c.none", type_id, spec))
            .await
            .unwrap();
        assert_eq!(o.state, RunState::Failed, "{type_id}");
        let m = o.message.unwrap();
        assert!(
            m.contains("needs internal storage") && m.contains(type_id),
            "{m}"
        );
    }
}

/// The hold audit (Step 5): `Get` lets its row-read connection go before it
/// reads the object. With a one-connection pool and the store stalled in
/// `get_stream`, the pool's connection is idle.
#[tokio::test(flavor = "multi_thread")]
async fn get_releases_its_connection_before_reading_the_object() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.hold";
    seed(&db, &fs, ws, ns, "k", string("v")).await;
    let one = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pool.connect_options()).clone())
        .await
        .unwrap();
    let get = GetKv::new(one.clone(), Some(fs.clone()));
    STALL.store(true, Ordering::SeqCst);
    let stalled = STALLED.notified();
    let task = input(ws, ns, kvt::GET, json!({"key": "k"}));
    let run = tokio::spawn(async move { get.run(&task).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), stalled)
        .await
        .expect("get_stream was reached");
    let mut idle = false;
    for _ in 0..100 {
        if one.size() >= 1 && one.num_idle() == one.size() as usize {
            idle = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    STALL.store(false, Ordering::SeqCst);
    RELEASE.notify_one();
    let o = run.await.unwrap().unwrap();
    assert!(idle, "a connection was held across get_stream");
    assert_eq!(ran(kvt::GET, o), Ran::Ok(json!({"value": "v"})));
}

// ---------------------------------------------------------------------------
// end to end, through the worker
// ---------------------------------------------------------------------------

fn registry(pool: &sqlx::PgPool) -> Handlers {
    let mut r = Handlers::with_core_tasks(storage::current());
    kvt::register(&mut r, pool.clone(), storage::current());
    r
}

async fn drain(pool: &sqlx::PgPool, reg: &Handlers) {
    for _ in 0..512 {
        let ticked = tick(pool, 16, reg).await;
        assert!(ticked.is_ok(), "a worker error: {ticked:?}");
        if ticked.unwrap() == 0 {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

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

/// One flow that sets (typed, a map, a 1,000-digit number), reads with
/// `Get` and `kv()`, puts, lists, deletes and purges, run by the worker.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_that_sets_reads_puts_deletes_and_purges() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let n1000 = "7".repeat(1000);
    let source = format!(
        r#"id: kvflow
namespace: w9c.e2e
tasks:
  - id: s
    type: io.kestra.plugin.core.kv.Set
    key: m
    value:
      a: "{{{{ 1 + 1 }}}}"
      b: x
  - id: n
    type: io.kestra.plugin.core.kv.Set
    key: big
    value: "{n1000}"
    kvType: NUMBER
  - id: g
    type: io.kestra.plugin.core.kv.Get
    key: m
  - id: r
    type: io.kestra.plugin.core.debug.Return
    format: "{{{{ kv('m').a }}}}|{{{{ outputs.g.value.b }}}}|{{{{ kv('big') }}}}"
  - id: p
    type: io.kestra.plugin.core.kv.Put
    key: m
    value:
      c: "{{{{ kv('m').a * 10 }}}}"
  - id: g2
    type: io.kestra.plugin.core.kv.Get
    key: m
  - id: k
    type: io.kestra.plugin.core.kv.GetKeys
  - id: d
    type: io.kestra.plugin.core.kv.Delete
    key: big
  - id: purge
    type: io.kestra.plugin.core.kv.PurgeKV
    behavior:
      type: version
      keepAmount: 0
  - id: k2
    type: io.kestra.plugin.core.kv.GetKeys
"#
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool, &registry(&db.pool)).await;
    let out = |t: &'static str| {
        let pool = db.pool.clone();
        async move {
            let (state, outputs, message) = result_of(&pool, ex, t).await;
            assert_eq!(state, "SUCCESS", "{t}: {message:?}");
            outputs.unwrap_or(json!({}))
        }
    };
    assert_eq!(out("g").await, json!({"value": {"a": 2, "b": "x"}}));
    assert_eq!(out("r").await, json!({"value": format!("2|x|{n1000}")}));
    assert_eq!(
        out("g2").await,
        json!({"value": {"a": 2, "b": "x", "c": 20}})
    );
    assert_eq!(out("k").await, json!({"keys": ["big", "m"]}));
    assert_eq!(out("d").await, json!({"deleted": true}));
    assert_eq!(out("purge").await, json!({"size": 3}));
    assert_eq!(out("k2").await, json!({"keys": []}));
}

/// W9c final review F3: an egret bug on the task path (here a key whose
/// next revision would pass `i32`) fails the task once, as a wrong grant
/// does, instead of `Err`, which re-claims the run forever. Mutant: the
/// store's `internal` back to `NsError::Db` makes this run a worker error.
#[tokio::test(flavor = "multi_thread")]
async fn an_egret_bug_on_the_task_path_fails_once() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, _fs)) = setup().await else {
        return;
    };
    let ns = "w9c.f3";
    row(&db, ws, ns, "top", i32::MAX, 1.0, false, false).await;
    let t = Tasks::new(&db.pool, Some(installed()));
    let m = t
        .go(ws, ns, kvt::SET, json!({"key": "top", "value": "x"}))
        .await
        .failed();
    assert!(
        m.contains("internal error (an egret bug, not retried)") && m.contains("2147483648"),
        "{m}"
    );
}

/// A 31-digit decimal and a 40-digit int (past `u128`), as the API stores
/// them (W9c final review F1).
const DEC31: &str = "123456789012345678901234567890.5";
const INT40: &str = "1234567890123456789012345678901234567890";

/// W9c final review F1: an exact number keeps its kind and digits through
/// the typed render, top level and nested; a flow's own literal fraction is
/// still a float. Mutant: `render_typed` going back to the `toJson` text
/// round trip makes every decimal here a float.
#[test]
fn the_typed_render_keeps_an_exact_number_exact() {
    let cx = RenderContext::new();
    let typed = |t: &str| render_typed(t, &cx).unwrap().unwrap();
    let dec = |s: &str| KvData::Number(KvNumber::Decimal(s.into()));
    let int = |s: &str| KvData::Number(KvNumber::Int(s.into()));
    let float = |f: f64| KvData::Number(KvNumber::Float(f));
    assert_eq!(
        typed(&format!("{{{{ '{DEC31}' | number(type='BIGDECIMAL') }}}}")),
        dec(DEC31)
    );
    assert_eq!(
        typed(&format!("{{{{ '{INT40}' | number(type='BIGINTEGER') }}}}")),
        int(INT40)
    );
    assert_eq!(
        typed(&format!(
            "{{{{ {{'a': ['{DEC31}' | number(type='BIGDECIMAL'), 1.5], 'b': '2.50' | \
             number(type='BIGDECIMAL'), 'n': none}} }}}}"
        )),
        KvData::Struct(vec![
            ("a".into(), KvData::List(vec![dec(DEC31), float(1.5)])),
            ("b".into(), dec("2.50")),
        ])
    );
    // A flow's own literals keep `serde_json`'s semantics.
    assert_eq!(typed("{{ 1.5 }}"), float(1.5));
    assert_eq!(
        typed("{{ [7, 2.0] }}"),
        KvData::List(vec![int("7"), float(2.0)])
    );
    // A value a later pass re-rendered is read back from its text, as
    // before F1 (spec §15): the decimal is a float there.
    assert_eq!(
        typed("{{ ['{{ 1 }}', '2.5' | number(type='BIGDECIMAL')] }}"),
        KvData::List(vec![KvData::String("1".into()), float(2.5)])
    );
    // kvType STRING writes a decimal as `BigDecimal.toString()`.
    let v = set_value(
        &json!("{{ '0.0000001' | number(type='BIGDECIMAL') }}"),
        Some(KvType::String),
        &cx,
    )
    .unwrap();
    assert_eq!(v.data, KvData::String("1E-7".into()));
    // Over the digit limit: refused typed, never rounded.
    let long = "1".repeat(1001);
    let e = set_value(
        &json!(format!("{{{{ '{long}.5' | number(type='BIGDECIMAL') }}}}")),
        None,
        &cx,
    )
    .unwrap_err();
    assert!(matches!(e, NsError::TooLarge(_)), "{e:?}");
}

/// W9c final review F1, end to end: a decimal and an int the API stored,
/// copied by `Set` and through a `Put` leaf, are stored byte for byte; a
/// nested map holding the decimal goes through `Set` exact too. Mutant:
/// the typed render's JSON round trip stores `1.2345678901234568e29`.
#[tokio::test(flavor = "multi_thread")]
async fn set_and_put_copy_an_exact_number_byte_for_byte() {
    let _turn = SERIAL.lock().await;
    let Some((db, ws, fs)) = setup().await else {
        return;
    };
    let ns = "w9c.f1";
    let cap = 10 << 20;
    for (k, body) in [
        ("price", DEC31.to_string()),
        ("count", INT40.to_string()),
        ("nested", format!("{{a:{{d:{DEC31},l:[{DEC31},1.5e0]}}}}")),
    ] {
        seed(
            &db,
            &fs,
            ws,
            ns,
            k,
            parse_api_body(body.as_bytes(), cap).unwrap(),
        )
        .await;
    }
    let source = format!(
        r#"id: exact
namespace: {ns}
tasks:
  - id: d
    type: io.kestra.plugin.core.kv.Set
    key: copy_d
    value: "{{{{ kv('price') }}}}"
  - id: i
    type: io.kestra.plugin.core.kv.Set
    key: copy_i
    value: "{{{{ kv('count') }}}}"
  - id: n
    type: io.kestra.plugin.core.kv.Set
    key: copy_n
    value: "{{{{ kv('nested') }}}}"
  - id: m
    type: io.kestra.plugin.core.kv.Set
    key: map
    value:
      outer:
        d: "{{{{ kv('price') }}}}"
        f: 1.5
  - id: p
    type: io.kestra.plugin.core.kv.Put
    key: patched
    value:
      p: "{{{{ kv('price') }}}}"
      i: "{{{{ kv('count') }}}}"
"#
    );
    let ex = start_execution(&db.pool, ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&db.pool, &registry(&db.pool)).await;
    for t in ["d", "i", "n", "m", "p"] {
        let (state, _, message) = result_of(&db.pool, ex, t).await;
        assert_eq!(state, "SUCCESS", "{t}: {message:?}");
    }
    let stored = |k: &'static str| latest_bytes(&db, &fs, ws, ns, k);
    assert_eq!(stored("price").await, DEC31);
    assert_eq!(stored("copy_d").await, stored("price").await);
    assert_eq!(stored("count").await, INT40);
    assert_eq!(stored("copy_i").await, stored("count").await);
    assert_eq!(stored("copy_n").await, stored("nested").await);
    assert_eq!(
        stored("copy_n").await,
        format!("{{a:{{d:{DEC31},l:[{DEC31},1.5e0]}}}}")
    );
    // A flow's own literal fraction stays a float beside the decimal.
    assert_eq!(
        stored("map").await,
        format!("{{outer:{{d:{DEC31},f:1.5e0}}}}")
    );
    assert_eq!(stored("patched").await, format!("{{p:{DEC31},i:{INT40}}}"));
}

/// `valids/retry-with-flowable-errors` names `core.kv.Set`, a known type
/// now: no publish finding names it (it still needs `AllowFailure`).
#[test]
fn the_corpus_flow_with_kv_set_no_longer_names_an_unknown_kv_type() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/oracle-exec/corpus/valids/retry-with-flowable-errors.yaml"
    ))
    .unwrap();
    assert!(source.contains("io.kestra.plugin.core.kv.Set"));
    assert!(egret_workflow::spec::registry::known_type(kvt::SET).is_some());
}
