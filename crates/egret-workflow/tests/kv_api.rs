//! W9c Task 6: the KV API (spec §9) -- seven routes under
//! `/api/ws/{workspace}/workflows/namespaces/{namespace}/kv…`. Requires
//! DATABASE_URL.
//!
//! **One store, installed once by this binary** ([`installed`]): a
//! `MemStore` (every write and delete here runs in memory, never on a
//! shared bucket) with the default `max_read_bytes` (10 MiB) and
//! `max_namespace_files` = [`MAX_FILES`], and with [`Faulty`] in front of
//! it: every copy and read under the namespace [`OUTAGE_NS`] fails as an
//! unreachable S3 does, and every read under [`STALL_NS`] never answers.
//! Each test seeds its own workspace, so no test sees another's rows. The
//! routes without a store are `tests/kv_api_no_store.rs` (the store is
//! installed once per process).
//!
//! The fixture is `oracle-exec/w9c-kv.json`; a test asserts egret's answer,
//! the top-level value of a cell, never its `kestra`, and compares as the
//! cell's `compare` says.

mod common;

/// A method, a URI, its headers and its body.
type Request<'a> = (&'a str, String, Vec<(&'a str, &'a [u8])>, Vec<u8>);

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egret_core::UserId;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Resource};
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::handlers::kv as kvt;
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::{json, Value};
use uuid::Uuid;

const NS: &str = "w9c.api";
const OUTAGE_NS: &str = "w9c.outage";
const STALL_NS: &str = "w9c.stall";
/// `max_namespace_files` for this binary: small, so a listing crosses it,
/// and above the 12 keys of the largest fixture listing (`list_namespace`,
/// `api_inheritance`'s `w9c.inh.a`).
const MAX_FILES: u64 = 16;

/// `MemStore`, failing every copy and read under [`OUTAGE_NS`], and never
/// answering a read under [`STALL_NS`].
struct Faulty(egret_storage::MemStore);

/// Reads opened under [`STALL_NS`].
static STALLED_READS: AtomicUsize = AtomicUsize::new(0);

fn in_outage(uri: &str) -> bool {
    uri.contains("/w9c/outage/")
}

fn outage(uri: &str) -> StorageError {
    StorageError::S3(format!(
        "{uri}: dispatch failure: connection refused (bucket egret-test)"
    ))
}

#[async_trait::async_trait]
impl ObjectStore for Faulty {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        if in_outage(uri) {
            return Err(outage(uri));
        }
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    // A value reaches its key by a copy from its staging object.
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        if in_outage(dst) {
            return Err(outage(dst));
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
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        if in_outage(uri) {
            return Err(outage(uri));
        }
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        if in_outage(uri) {
            return Err(outage(uri));
        }
        if uri.contains("/w9c/stall/") {
            STALLED_READS.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        self.0.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        self.0.delete_prefix(uri).await
    }
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        storage::install(Arc::new(FileStore {
            store: Arc::new(Faulty(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps {
                max_namespace_files: MAX_FILES,
                ..Default::default()
            },
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

fn cap() -> u64 {
    installed().caps.max_read_bytes
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("../oracle-exec/w9c-kv.json")).expect("the fixture")
}

/// `egret_expected.api.<section>`.
fn api(section: &str) -> Value {
    fixture()["egret_expected"]["api"][section].clone()
}

struct Fx {
    db: Db,
    ws: Uuid,
    /// `workflow:read` + `workflow:manage`.
    user: UserId,
    app: common::App,
}

async fn fx() -> Option<Fx> {
    let db = common::db().await?;
    installed();
    egret_workflow::storage::nsfiles::install_db_for_tests(&db.pool);
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    Some(Fx { db, ws, user, app })
}

/// Another user, a member of `ws` holding exactly `perms`.
async fn member(db: &Db, ws: Uuid, perms: &[&str]) -> UserId {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");
    common::grant(db, ws, UserId(uid), perms).await;
    UserId(uid)
}

const TEXT: (&str, &[u8]) = ("content-type", b"text/plain");

async fn text(res: axum::response::Response) -> String {
    common::body_text(res).await
}

/// The status, then the `ErrorBody`'s `error`.
async fn refusal(res: axum::response::Response, status: u16) -> String {
    let got = res.status();
    let body = text(res).await;
    assert_eq!(got, status, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| panic!("not JSON: {body}"));
    v["error"]
        .as_str()
        .unwrap_or_else(|| panic!("{body}"))
        .to_string()
}

async fn ok_json(res: axum::response::Response) -> Value {
    let got = res.status();
    let body = text(res).await;
    assert_eq!(got, 200, "{body}");
    serde_json::from_str(&body).unwrap_or_else(|_| panic!("not JSON: {body}"))
}

fn lock_text(ws: Uuid, ns: &str, key: &str) -> String {
    format!("w9c:{}:{ns}:{key}", ws.hyphenated())
}

impl Fx {
    fn base(&self, ns: &str) -> String {
        format!("/api/ws/{}/workflows/namespaces/{ns}/kv", self.ws)
    }

    async fn put_as(
        &self,
        user: UserId,
        ns: &str,
        key: &str,
        body: impl Into<axum::body::Body>,
        headers: &[(&str, &[u8])],
    ) -> axum::response::Response {
        let uri = format!("{}/{key}", self.base(ns));
        common::send_with(&self.app, "PUT", &uri, user, headers, body.into()).await
    }

    async fn put(&self, ns: &str, key: &str, body: &str) -> axum::response::Response {
        self.put_as(self.user, ns, key, body.to_string(), &[TEXT])
            .await
    }

    async fn ok_put(&self, ns: &str, key: &str, body: &str) {
        let res = self.put(ns, key, body).await;
        let status = res.status();
        let answer = text(res).await;
        assert_eq!(status, 200, "PUT {ns}/{key}: {answer}");
        assert_eq!(answer, "", "a PUT answers no content");
    }

    async fn get_as(&self, user: UserId, ns: &str, rest: &str) -> axum::response::Response {
        common::get(&self.app, &format!("{}{rest}", self.base(ns)), user).await
    }

    async fn get(&self, ns: &str, rest: &str) -> axum::response::Response {
        self.get_as(self.user, ns, rest).await
    }

    async fn del_as(&self, user: UserId, ns: &str, key: &str) -> axum::response::Response {
        common::delete(&self.app, &format!("{}/{key}", self.base(ns)), user).await
    }

    async fn del(&self, ns: &str, key: &str) -> axum::response::Response {
        self.del_as(self.user, ns, key).await
    }

    async fn bulk_as(
        &self,
        user: UserId,
        ns: &str,
        body: Option<&[u8]>,
    ) -> axum::response::Response {
        let headers: &[(&str, &[u8])] = match body {
            Some(_) => &[("content-type", b"application/json")],
            None => &[],
        };
        common::send_with(
            &self.app,
            "DELETE",
            &self.base(ns),
            user,
            headers,
            body.map_or_else(axum::body::Body::empty, |b| b.to_vec().into()),
        )
        .await
    }

    async fn bulk(&self, ns: &str, keys: &[&str]) -> axum::response::Response {
        let body = json!({ "keys": keys }).to_string();
        self.bulk_as(self.user, ns, Some(body.as_bytes())).await
    }

    /// This workspace's rows.
    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1")
            .bind(self.ws)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// Every column of every row of this workspace (with `xmin` and `ctid`,
    /// which any UPDATE changes), hashed, and the count.
    async fn table_state(&self) -> (i64, String) {
        sqlx::query_as(
            "SELECT count(*), coalesce(md5(string_agg(t::text || '|' || t.xmin::text || '|' || \
               t.ctid::text, ',' ORDER BY t.id)), '') \
             FROM workflow.kv_entries t WHERE workspace_id = $1",
        )
        .bind(self.ws)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    async fn staging_rows(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM workflow.namespace_file_staging WHERE workspace_id = $1",
        )
        .bind(self.ws)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    /// Every object under this workspace's prefix.
    async fn objects(&self) -> Vec<String> {
        let fs = installed();
        let listing = fs
            .store
            .list(&format!("{}{}/", fs.prefix, self.ws), None)
            .await
            .unwrap();
        let mut keys: Vec<String> = listing.objects.into_iter().map(|o| o.key).collect();
        keys.sort();
        assert!(
            !keys.is_empty(),
            "the listing of this workspace's objects found none"
        );
        keys
    }

    /// The object of `revision` of `key`.
    async fn object(&self, ns: &str, key: &str, revision: u32) -> Vec<u8> {
        let fs = installed();
        let g = authorize(self.ws, Actor::Api, ns, Resource::Kv, Access::Read).unwrap();
        let uri = fs.namespace_kv_uri(&g, &KvKey::parse(key).unwrap(), revision);
        fs.store.get(&uri).await.unwrap()
    }

    /// The key's live state: `(revision, deleted)` of its latest row.
    async fn latest(&self, ns: &str, key: &str) -> Option<(i32, bool)> {
        sqlx::query_as(
            "SELECT revision, deleted FROM workflow.kv_entries \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3 \
             ORDER BY revision DESC LIMIT 1",
        )
        .bind(self.ws)
        .bind(ns)
        .bind(key)
        .fetch_optional(&self.db.pool)
        .await
        .unwrap()
    }

    /// Rows inserted directly (a live STRING revision 1 of each key, size 1,
    /// no object): the bulk delete touches no object.
    async fn insert_live(&self, ns: &str, keys: &[String]) {
        sqlx::query(
            "INSERT INTO workflow.kv_entries (workspace_id, namespace, key, revision, value_type, \
               size) \
             SELECT $1, $2, k, 1, 'STRING', 1 FROM unnest($3::text[]) AS k",
        )
        .bind(self.ws)
        .bind(ns)
        .bind(keys)
        .execute(&self.db.pool)
        .await
        .unwrap();
    }
}

fn keys_of(listing: &Value) -> Vec<String> {
    listing
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {listing}"))
        .iter()
        .map(|e| e["key"].as_str().unwrap().to_string())
        .collect()
}

// ─── Permissions ────────────────────────────────────────────────────────────

/// workflow:read reads every GET route and writes nothing (403 on the three
/// write routes, nothing changed); workflow:manage writes. Mutants (a) and
/// the route permission (manage → read) die here.
#[tokio::test]
async fn read_routes_need_workflow_read_and_write_routes_need_workflow_manage() {
    let Some(mut fx) = fx().await else { return };
    fx.ok_put(NS, "k", "1").await;
    let reader = member(&fx.db, fx.ws, &["workflow:read"]).await;
    let nobody = member(&fx.db, fx.ws, &[]).await;
    let manager = member(&fx.db, fx.ws, &["workflow:manage"]).await;
    // The PDP reads grants when it is built.
    fx.app = common::app(&fx.db).await;
    for rest in ["", "/inheritance", "/k", "/k/revisions"] {
        let res = fx.get_as(reader, NS, rest).await;
        assert_eq!(res.status(), 200, "GET {rest}: {}", text(res).await);
    }
    let res = fx.get_as(reader, NS, "/never").await;
    assert_eq!(res.status(), 404);
    let before = fx.table_state().await;
    let res = fx.put_as(reader, NS, "k", "2", &[TEXT]).await;
    refusal(res, 403).await;
    let res = fx.put_as(reader, NS, "new", "2", &[TEXT]).await;
    refusal(res, 403).await;
    refusal(fx.del_as(reader, NS, "k").await, 403).await;
    let body = json!({"keys": ["k"]}).to_string();
    refusal(fx.bulk_as(reader, NS, Some(body.as_bytes())).await, 403).await;
    assert_eq!(fx.table_state().await, before, "a read-only caller wrote");
    // A member with neither permission reads nothing.
    for rest in ["", "/inheritance", "/k", "/k/revisions"] {
        refusal(fx.get_as(nobody, NS, rest).await, 403).await;
    }
    // Permission before namespace (fix round 1, review M1; mutant ORDER,
    // `authorize` before `require` in `grant`, answers these 400 naming
    // the `_kv` rule): a caller without the permission learns nothing about
    // the namespace, here one `authorize` refuses.
    let refused = "w9c._kv";
    for rest in ["", "/inheritance", "/k", "/k/revisions"] {
        let msg = refusal(fx.get_as(nobody, refused, rest).await, 403).await;
        assert!(!msg.contains("_kv"), "GET {rest}: {msg}");
    }
    refusal(fx.put_as(reader, refused, "k", "2", &[TEXT]).await, 403).await;
    refusal(fx.del_as(reader, refused, "k").await, 403).await;
    let body = json!({"keys": ["k"]}).to_string();
    refusal(
        fx.bulk_as(reader, refused, Some(body.as_bytes())).await,
        403,
    )
    .await;
    // The control: with the permission, the same namespace is 400.
    let msg = refusal(fx.get(refused, "").await, 400).await;
    assert!(msg.contains("_kv"), "{msg}");
    // workflow:manage alone writes (and reads nothing without workflow:read).
    refusal(fx.get_as(manager, NS, "").await, 403).await;
    let res = fx.put_as(manager, NS, "k", "2", &[TEXT]).await;
    assert_eq!(res.status(), 200, "{}", text(res).await);
    assert_eq!(fx.latest(NS, "k").await, Some((2, false)));
    let res = fx.del_as(manager, NS, "k").await;
    assert_eq!(ok_json(res).await, json!(true));
    fx.ok_put(NS, "b", "1").await;
    let body = json!({"keys": ["b"]}).to_string();
    let res = fx.bulk_as(manager, NS, Some(body.as_bytes())).await;
    assert_eq!(ok_json(res).await, json!({"keys": ["b"]}));
}

// ─── The status table ───────────────────────────────────────────────────────

/// Spec §9's error table, one request per row: 400 (key rule, `_kv` and
/// `_files` namespaces, bad and negative ttl, a non-integer revision, a
/// malformed bulk body, a bad key in a bulk body, a blob), 400 "empty
/// body", 404, 408, 409, 413, 415, 503. Mutant (b) dies at the `_kv` row.
#[tokio::test(flavor = "multi_thread")]
async fn every_row_of_the_status_table() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "k", "1").await;
    let before = fx.table_state().await;
    // 400: the key rule, on every route that takes a key.
    let rule = "Key must start with an alphanumeric character";
    assert!(refusal(fx.put(NS, "_k", "1").await, 400)
        .await
        .contains(rule));
    assert!(refusal(fx.get(NS, "/_k").await, 400).await.contains(rule));
    assert!(refusal(fx.get(NS, "/_k/revisions").await, 400)
        .await
        .contains(rule));
    assert!(refusal(fx.del(NS, "_k").await, 400).await.contains(rule));
    let long = "k".repeat(351);
    assert!(refusal(fx.put(NS, &long, "1").await, 400)
        .await
        .contains("351 bytes long"));
    // 400: a refused namespace, on every route.
    for (ns, says) in [
        ("w9c._kv", "'_kv'"),
        ("w9c._files.x", "'_files'"),
        ("w9c..x", "empty segment"),
        ("W9C", "must match"),
    ] {
        for res in [
            fx.get(ns, "").await,
            fx.get(ns, "/inheritance").await,
            fx.get(ns, "/k").await,
            fx.get(ns, "/k/revisions").await,
            fx.put(ns, "k", "1").await,
            fx.del(ns, "k").await,
            fx.bulk(ns, &["k"]).await,
        ] {
            let msg = refusal(res, 400).await;
            assert!(msg.contains(ns) && msg.contains(says), "{ns}: {msg}");
        }
    }
    // 400: ttl.
    let ttl = |v: &'static str| [TEXT, ("ttl", v.as_bytes())];
    assert_eq!(
        refusal(fx.put_as(fx.user, NS, "t", "1", &ttl("xyz")).await, 400).await,
        "Cannot convert value 'xyz' to Duration"
    );
    assert_eq!(
        refusal(fx.put_as(fx.user, NS, "t", "1", &ttl("-PT1S")).await, 400).await,
        "ttl cannot be negative"
    );
    assert!(refusal(
        fx.put_as(fx.user, NS, "t", "1", &ttl("P365251D")).await,
        400
    )
    .await
    .contains("365250 days"));
    // 400: a revision that is not an integer.
    refusal(fx.get(NS, "/k?revision=abc").await, 400).await;
    refusal(fx.get(NS, "/k?revision=-1").await, 400).await;
    // 400: a malformed bulk body, and a bad key in one (named).
    refusal(fx.bulk_as(fx.user, NS, Some(b"{keys:")).await, 400).await;
    refusal(fx.bulk_as(fx.user, NS, Some(b"{\"keys\": 5}")).await, 400).await;
    refusal(fx.bulk_as(fx.user, NS, None).await, 400).await;
    let msg = refusal(fx.bulk(NS, &["k", "a/b"]).await, 400).await;
    assert!(msg.contains("'a/b'"), "{msg}");
    // 400: a type the KV store does not model; the empty body.
    assert!(refusal(fx.put(NS, "t", "{{aGVsbG8=}}").await, 400)
        .await
        .contains("blob"));
    assert!(refusal(fx.put(NS, "t", " \n").await, 400)
        .await
        .contains("empty body"));
    // 404.
    assert_eq!(
        refusal(fx.get(NS, "/never").await, 404).await,
        format!("No value found for key 'never' in namespace '{NS}'")
    );
    refusal(fx.get(NS, "/never/revisions").await, 404).await;
    refusal(fx.get(NS, "/k?revision=9").await, 404).await;
    // 413: a bulk body over 1,000 keys (and the body cap: its own test).
    let many: Vec<String> = (0..1001).map(|i| format!("k{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert!(refusal(fx.bulk(NS, &many).await, 413)
        .await
        .contains("1000"));
    // 415: not text/plain.
    let res = fx
        .put_as(
            fx.user,
            NS,
            "t",
            "1",
            &[("content-type", b"application/json")],
        )
        .await;
    refusal(res, 415).await;
    // 503: internal storage is unavailable; the store's own error is not sent.
    let msg = refusal(fx.put(OUTAGE_NS, "t", "1").await, 503).await;
    assert!(
        msg.contains("'t'") && !msg.contains("s3") && !msg.contains("dispatch"),
        "{msg}"
    );
    assert_eq!(fx.table_state().await, before, "a refused request wrote");
    // 408: the body stalled (30 s in production; 300 ms here).
    let app = common::app_with_upload_idle(&fx.db, std::time::Duration::from_millis(300)).await;
    let uri = format!("{}/t", fx.base(NS));
    let res = common::send_with(&app, "PUT", &uri, fx.user, &[TEXT], stalling_body(None)).await;
    refusal(res, 408).await;
    // 409: the key's lock held past the API's wait.
    let mut held = fx.db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(lock_text(fx.ws, NS, "k"))
        .execute(&mut *held)
        .await
        .unwrap();
    let msg = refusal(fx.put(NS, "k", "2").await, 409).await;
    assert!(msg.contains("the key is busy"), "{msg}");
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut *held)
        .await
        .unwrap();
    assert_eq!(fx.table_state().await, before, "a refused request wrote");
}

// ─── PUT ────────────────────────────────────────────────────────────────────

/// Every w9c `api.put_bodies` cell, each PUT to its own fresh key: a 200
/// cell's row has its type and its object is exactly its `ion_bytes`; a 400
/// cell names what it says and writes nothing.
#[tokio::test]
async fn put_parses_ion_and_falls_back_to_the_raw_string() {
    let Some(fx) = fx().await else { return };
    let cells = api("put_bodies");
    let mut examined = 0;
    for (name, cell) in cells.as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        examined += 1;
        let key = name.replace('_', "-");
        let body = cell["body"].as_str().unwrap();
        let res = fx.put(NS, &key, body).await;
        if cell["put_status"] == 400 {
            let msg = refusal(res, 400).await;
            for frag in cell["message_contains"].as_array().unwrap() {
                assert!(msg.contains(frag.as_str().unwrap()), "{name}: {msg}");
            }
            assert_eq!(fx.latest(NS, &key).await, None, "{name}: written");
            continue;
        }
        assert_eq!(res.status(), 200, "{name}: {}", text(res).await);
        let (kind,): (String,) = sqlx::query_as(
            "SELECT value_type FROM workflow.kv_entries \
             WHERE workspace_id = $1 AND namespace = $2 AND key = $3",
        )
        .bind(fx.ws)
        .bind(NS)
        .bind(&key)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        assert_eq!(kind, cell["get"]["type"].as_str().unwrap(), "{name}");
        let bytes = fx.object(NS, &key, 1).await;
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            cell["ion_bytes"].as_str().unwrap(),
            "{name}"
        );
    }
    assert_eq!(examined, 54, "every put_bodies cell is examined");
}

/// w9c `api.headers`: `description` stored as given (`""` kept), `ttl`
/// read as the header mode reads it (`PT0S` accepted and expired at once;
/// `xyz` and `-PT1S` refused with the exact message, nothing written).
#[tokio::test]
async fn put_takes_description_and_ttl_headers() {
    let Some(fx) = fx().await else { return };
    let cells = api("headers");
    let mut examined = 0;
    for (name, cell) in cells.as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        examined += 1;
        let key = name.replace('_', "-");
        let mut headers: Vec<(&str, &[u8])> = vec![TEXT];
        if let Some(d) = cell["description"].as_str() {
            headers.push(("description", d.as_bytes()));
        }
        if let Some(t) = cell["ttl"].as_str() {
            headers.push(("ttl", t.as_bytes()));
        }
        let res = fx.put_as(fx.user, NS, &key, "\"v\"", &headers).await;
        if cell["status"] == 400 {
            assert_eq!(
                refusal(res, 400).await,
                cell["message"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(fx.latest(NS, &key).await, None, "{name}: written");
            continue;
        }
        assert_eq!(res.status(), 200, "{name}: {}", text(res).await);
        let revisions = ok_json(fx.get(NS, &format!("/{key}/revisions")).await).await;
        let rows = cell["rows"].as_array().unwrap();
        assert_eq!(revisions.as_array().unwrap().len(), rows.len(), "{name}");
        let (got, want) = (&revisions[0], &rows[0]);
        assert_eq!(got["revision"], want["revision"], "{name}");
        assert_eq!(got["deleted"], want["deleted"], "{name}");
        assert_eq!(got["description"], want["description"], "{name}");
        assert_eq!(
            got["expiresAt"].is_null(),
            want["expires"].is_null(),
            "{name}"
        );
        if let Some(t) = cell["ttl"].as_str() {
            // expiry = write + ttl, by one clock reading.
            let at = |v: &Value| chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap()).unwrap();
            let ttl = egret_workflow::storage::kv::value::parse_ttl(
                t,
                egret_workflow::storage::kv::value::Ttl::Header,
            )
            .unwrap();
            assert_eq!(at(&got["expiresAt"]) - at(&got["created"]), ttl, "{name}");
        }
    }
    assert_eq!(examined, 7, "every headers cell is examined");
    // PT0S: expired at once, so missing on every read, and flagged.
    refusal(fx.get(NS, "/h-ttl-zero").await, 404).await;
    let revisions = ok_json(fx.get(NS, "/h-ttl-zero/revisions").await).await;
    assert_eq!(revisions[0]["expired"], json!(true));
    // A header that is not UTF-8 text is refused.
    let res = fx
        .put_as(fx.user, NS, "bad-d", "1", &[TEXT, ("description", b"\xff")])
        .await;
    assert!(refusal(res, 400).await.contains("description"));
    // A UTF-8 description is kept as given.
    let res = fx
        .put_as(
            fx.user,
            NS,
            "utf8-d",
            "1",
            &[TEXT, ("description", "été".as_bytes())],
        )
        .await;
    assert_eq!(res.status(), 200);
    let detail = ok_json(fx.get(NS, "/utf8-d").await).await;
    assert_eq!(detail["description"], json!("été"));
}

/// A body of `max_read_bytes` + 1 is refused naming the cap; the server stops
/// reading at the cap (mutant c: without the limit it reads all 20 MiB and
/// the refusal is the reader's, not the body's).
#[tokio::test(flavor = "multi_thread")]
async fn a_put_body_over_max_read_bytes_is_413_naming_the_cap_and_nothing_is_written() {
    let Some(fx) = fx().await else { return };
    let cap = cap();
    let before = fx.table_state().await;
    let body = vec![b'a'; cap as usize + 1];
    let msg = refusal(fx.put_as(fx.user, NS, "big", body, &[TEXT]).await, 413).await;
    assert!(
        msg.contains("the request body is over")
            && msg.contains(&format!("{cap} bytes"))
            && msg.contains("max_read_bytes"),
        "{msg}"
    );
    // A streamed body twice the cap: how much of it the server pulled.
    let pulled = Arc::new(AtomicUsize::new(0));
    let counter = pulled.clone();
    let chunk = axum::body::Bytes::from(vec![b'a'; 64 * 1024]);
    let n = (2 * cap as usize) / chunk.len();
    let stream = futures_util::stream::iter((0..n).map(move |_| {
        counter.fetch_add(chunk.len(), Ordering::SeqCst);
        Ok::<_, std::io::Error>(chunk.clone())
    }));
    let res = fx
        .put_as(
            fx.user,
            NS,
            "big",
            axum::body::Body::from_stream(stream),
            &[TEXT],
        )
        .await;
    let msg = refusal(res, 413).await;
    assert!(msg.contains("the request body is over"), "{msg}");
    let pulled = pulled.load(Ordering::SeqCst) as u64;
    assert!(
        pulled <= cap + 2 * 64 * 1024,
        "the server read {pulled} bytes of a body over the cap ({cap})"
    );
    assert_eq!(fx.table_state().await, before);
    assert_eq!(fx.staging_rows().await, 0);
    // The cap itself (fix round 1, review M4; mutant `limit - 1` refuses
    // it 413 "the request body is over"): a quoted Ion string of exactly
    // `max_read_bytes` bytes is read whole, and its encoded value, those
    // same bytes, is exactly the cap: stored.
    let at_cap = format!("\"{}\"", "a".repeat(cap as usize - 2));
    assert_eq!(at_cap.len() as u64, cap);
    fx.ok_put(NS, "at-cap", &at_cap).await;
    assert_eq!(fx.latest(NS, "at-cap").await, Some((1, false)));
    assert_eq!(fx.object(NS, "at-cap", 1).await.len() as u64, cap);
    // One byte more is the body's refusal, and writes nothing.
    let over = format!("\"{}\"", "a".repeat(cap as usize - 1));
    let msg = refusal(fx.put(NS, "at-cap", &over).await, 413).await;
    assert!(msg.contains("the request body is over"), "{msg}");
    assert_eq!(fx.latest(NS, "at-cap").await, Some((1, false)));
}

/// The dense-value limit and the shared expansion budget (fix round 2,
/// NI1): `[0,0,…]` of 200,000 values is 413 naming the limit; 11,000
/// decimals of 1,000 plain digits each (`1d999`: 66 KB of body, 11 MB of
/// digits) are 413 naming `max_read_bytes`, refused while converted, before
/// they are all expanded (mutant h: a fresh budget per decimal builds them,
/// and the refusal is the encoder's, "as stored"); `[1d4000000]` is 413
/// naming the 1,000-digit limit on read (re-review 3, M1). Since that
/// limit, `[1d9000000,1d9000000]` is also refused by it (each decimal is
/// 9,000,001 digits), not by the budget.
#[tokio::test(flavor = "multi_thread")]
async fn a_dense_or_expanding_body_is_413_and_nothing_is_written() {
    let Some(fx) = fx().await else { return };
    let before = fx.table_state().await;
    let zeros = format!("[{}0]", "0,".repeat(199_999));
    let msg = refusal(fx.put(NS, "dense", &zeros).await, 413).await;
    assert!(msg.contains("values (one per 64 bytes"), "{msg}");
    let decimals = format!("[{}1d999]", "1d999,".repeat(10_999));
    let msg = refusal(fx.put(NS, "expand", &decimals).await, 413).await;
    assert!(
        msg.contains("decodes to more than workflow.storage.max_read_bytes"),
        "{msg}"
    );
    for body in ["[1d4000000]", "[1d9000000,1d9000000]"] {
        let msg = refusal(fx.put(NS, "digits", body).await, 413).await;
        assert!(msg.contains("at most 1000 digits"), "{body}: {msg}");
    }
    assert_eq!(fx.table_state().await, before);
    // The control: 10,470 of them fit the budget (10,470,000 digits, under
    // the 10,485,760-byte cap) and are refused only when stored: 1,002
    // bytes each as Ion text (`1000…0.,`) is over the cap.
    let fits = format!("[{}1d999]", "1d999,".repeat(10_469));
    let msg = refusal(fx.put(NS, "fits", &fits).await, 413).await;
    assert!(msg.contains("as stored"), "{msg}");
}

/// Fix round 3, re-review P1: a refusal of the reader is never the raw
/// text. Each refused body writes nothing; the controls are stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_reader_refusal_is_never_stored_as_raw_text() {
    let Some(fx) = fx().await else { return };
    let before = fx.table_state().await;
    // 101 levels: 400.
    let deep = format!("{}1{}", "[".repeat(101), "]".repeat(101));
    refusal(fx.put(NS, "deep", &deep).await, 400).await;
    // A 70,001-digit decimal: 413 naming the limit (mutant i stores it
    // truncated; mutant j stores the raw text).
    let long = format!("1.{}", "1".repeat(70_000));
    let msg = refusal(fx.put(NS, "long", &long).await, 413).await;
    assert!(msg.contains("1000 digits"), "{msg}");
    // A binary Ion int of 2,000 bytes: 413.
    let mut int = vec![0xE0, 0x01, 0x00, 0xEA, 0x2E, 0x0F, 0xD0];
    int.extend(std::iter::repeat_n(0xFF, 2000));
    let msg = refusal(fx.put_as(fx.user, NS, "bin", int, &[TEXT]).await, 413).await;
    assert!(msg.contains("1000 digits"), "{msg}");
    // A symbol table whose symbols cost more than the cap: 413.
    let symbols: Vec<String> = (0..163_000).map(|i| format!("\"s{i}\"")).collect();
    let table = format!("$ion_symbol_table::{{symbols:[{}]}} 1", symbols.join(","));
    let msg = refusal(fx.put(NS, "symbols", &table).await, 413).await;
    assert!(msg.contains("symbol"), "{msg}");
    assert_eq!(fx.table_state().await, before, "a refused body was written");
    // A symbol ID past usize is refused unread by the scan: the raw text.
    let detail = |key: &'static str| {
        let fx = &fx;
        async move { ok_json(fx.get(NS, &format!("/{key}")).await).await }
    };
    fx.ok_put(NS, "sid", "$99999999999999999999").await;
    let d = detail("sid").await;
    assert_eq!(
        (d["type"].clone(), d["value"].clone()),
        (json!("STRING"), json!("$99999999999999999999"))
    );
    // Not Ion, two values, an annotation: the raw text, STRING.
    for (key, body) in [("open", "{a:"), ("two", "1 2"), ("annot", "x::1")] {
        fx.ok_put(NS, key, body).await;
        let d = detail(key).await;
        assert_eq!(
            (d["type"].clone(), d["value"].clone()),
            (json!("STRING"), json!(body))
        );
    }
    // W9c final review F5: the digit limit applies only to one Ion value; a
    // body of several values holding a 1,001-digit run is the raw text.
    let several = format!("note {}", "1".repeat(1001));
    fx.ok_put(NS, "several", &several).await;
    let d = detail("several").await;
    assert_eq!(
        (d["type"].clone(), d["value"].clone()),
        (json!("STRING"), json!(several))
    );
    // 1,000 digits round-trip exactly, as an int and as a decimal.
    let int = "7".repeat(1000);
    let dec = format!("{}.25", "3".repeat(998));
    fx.ok_put(NS, "int1000", &int).await;
    fx.ok_put(NS, "dec1000", &dec).await;
    for (key, digits) in [("int1000", &int), ("dec1000", &dec)] {
        let res = fx.get(NS, &format!("/{key}")).await;
        let body = text(res).await;
        assert!(
            body.contains(&format!("\"value\":{digits},")),
            "{key}: {body}"
        );
    }
}

/// Re-review 3, M3: a raw-text fallback that is not UTF-8 is 400, nothing
/// written (mutant k: a lossy decode stores it, 200).
#[tokio::test]
async fn a_raw_text_fallback_that_is_not_utf8_is_400() {
    let Some(fx) = fx().await else { return };
    let before = fx.table_state().await;
    let malformed = vec![0xE0, 0x01, 0x00, 0xEA, 0xF0];
    for body in [malformed, vec![0xFF, 0xFE, 0x00]] {
        let res = fx.put_as(fx.user, NS, "bytes", body.clone(), &[TEXT]).await;
        let msg = refusal(res, 400).await;
        assert!(msg.contains("not UTF-8 text"), "{body:?}: {msg}");
    }
    assert_eq!(fx.table_state().await, before);
    fx.ok_put(NS, "bytes", "{a:").await;
    let d = ok_json(fx.get(NS, "/bytes").await).await;
    assert_eq!(
        (d["type"].clone(), d["value"].clone()),
        (json!("STRING"), json!("{a:"))
    );
}

/// The user's ruling (2026-10-07): a decimal keeps every digit, stored and
/// answered (w9c `api.put_bodies.nested_dec`, `dec_150`, `ion_decimal_d`).
#[tokio::test]
async fn an_ion_decimal_is_stored_and_answered_exactly() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "a", "{a:1.5}").await;
    assert_eq!(fx.object(NS, "a", 1).await, b"{a:1.5}");
    let d = ok_json(fx.get(NS, "/a").await).await;
    assert_eq!(
        (d["type"].clone(), d["value"].clone()),
        (json!("JSON"), json!({"a": 1.5}))
    );
    let big = "123456789012345678901234567890.5";
    fx.ok_put(NS, "big", big).await;
    assert_eq!(fx.object(NS, "big", 1).await, big.as_bytes());
    let body = text(fx.get(NS, "/big").await).await;
    assert!(body.contains(&format!("\"value\":{big},")), "{body}");
    let cells = api("put_bodies");
    for name in ["nested_dec", "dec_150", "ion_decimal_d"] {
        let cell = &cells[name];
        fx.ok_put(
            NS,
            name.replace('_', "-").as_str(),
            cell["body"].as_str().unwrap(),
        )
        .await;
        let d = ok_json(fx.get(NS, &format!("/{}", name.replace('_', "-"))).await).await;
        assert_eq!(d["type"], cell["get"]["type"], "{name}");
        assert_eq!(d["value"], cell["get"]["value"], "{name}");
    }
}

/// A `text/plain` body that hands over `abc` (and counts itself in
/// `stalled`, if given) and then sends nothing more, ever.
fn stalling_body(stalled: Option<Arc<AtomicUsize>>) -> axum::body::Body {
    use futures_util::StreamExt as _;
    let first = futures_util::stream::once(async {
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"\"abc"))
    });
    let stall = futures_util::stream::once(async move {
        if let Some(s) = stalled {
            s.fetch_add(1, Ordering::SeqCst);
        }
        std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
    });
    axum::body::Body::from_stream(first.chain(stall))
}

/// A body that sends one byte every 20 ms, for ever.
fn trickling_body() -> axum::body::Body {
    use futures_util::StreamExt as _;
    let trickle = futures_util::stream::repeat_with(|| ()).then(|()| async {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"t"))
    });
    axum::body::Body::from_stream(trickle)
}

/// W9b's body clocks (300 ms here): idle, and in all; 408, nothing written,
/// nothing staged, and the key written at once afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_put_body_that_stalls_or_takes_too_long_is_408_and_nothing_is_written() {
    let Some(fx) = fx().await else { return };
    let before = fx.table_state().await;
    let uri = format!("{}/slow", fx.base(NS));
    let app = common::app_with_upload_idle(&fx.db, std::time::Duration::from_millis(300)).await;
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        common::send_with(&app, "PUT", &uri, fx.user, &[TEXT], stalling_body(None)),
    )
    .await
    .expect("the stalled PUT was never refused");
    let msg = refusal(res, 408).await;
    assert!(
        msg.contains("no byte of the request body arrived for 300 ms")
            && msg.contains("nothing was stored"),
        "{msg}"
    );
    let app = common::app_with_upload_total(&fx.db, std::time::Duration::from_millis(300)).await;
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        common::send_with(&app, "PUT", &uri, fx.user, &[TEXT], trickling_body()),
    )
    .await
    .expect("the trickling PUT was never refused");
    let msg = refusal(res, 408).await;
    assert!(
        msg.contains("the request body took longer than 300 ms in all"),
        "{msg}"
    );
    assert_eq!(fx.table_state().await, before, "a row was written");
    assert_eq!(fx.staging_rows().await, 0, "something was staged");
    fx.ok_put(NS, "slow", "1").await;
}

/// The hold audit (mutant d): five PUTs whose bodies stall, on this test's
/// 5-connection pool, hold no pool connection, no advisory lock and no
/// staging row while their bodies arrive; an unrelated GET answers at once.
#[tokio::test(flavor = "multi_thread")]
async fn stalled_puts_hold_no_connection_no_lock_and_stage_nothing() {
    let Some(fx) = fx().await else { return };
    assert_eq!(fx.db.pool.options().get_max_connections(), 5);
    fx.ok_put(NS, "seen", "1").await;
    let stalled = Arc::new(AtomicUsize::new(0));
    let puts = futures_util::future::join_all((0..5).map(|i| {
        let uri = format!("{}/stall{i}", fx.base(NS));
        let body = stalling_body(Some(stalled.clone()));
        let app = &fx.app;
        async move { common::send_with(app, "PUT", &uri, fx.user, &[TEXT], body).await }
    }));
    let probe = async {
        let all_stalled = async {
            while stalled.load(Ordering::SeqCst) < 5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), all_stalled).await;
        // Every connection the pool has opened is idle: the five stalled
        // PUTs hold none (fix round 1, review M11: `num_idle() > 0` let a
        // PUT keep one). A connection goes back to the pool on a task of
        // its own after its guard drops, so this waits up to 2 s for the
        // pool to settle; one held connection never does.
        let settled = async {
            while fx.db.pool.num_idle() != fx.db.pool.size() as usize {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        let held = tokio::time::timeout(std::time::Duration::from_secs(2), settled).await;
        assert!(
            held.is_ok(),
            "the stalled PUTs hold {} of the pool's {} connections",
            fx.db.pool.size() as usize - fx.db.pool.num_idle(),
            fx.db.pool.size()
        );
        let get = tokio::time::timeout(std::time::Duration::from_secs(5), fx.get(NS, "/seen"))
            .await
            .expect("an unrelated GET waited on the stalled PUTs");
        assert_eq!(get.status(), 200, "{}", text(get).await);
        // No advisory lock of any stalled key, and nothing staged.
        let mut locks = 0i64;
        for i in 0..5 {
            let held: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' \
                   AND ((classid::bigint << 32) | objid::bigint) = hashtextextended($1, 0)",
            )
            .bind(lock_text(fx.ws, NS, &format!("stall{i}")))
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
            locks += held;
        }
        assert_eq!(locks, 0, "a stalled PUT holds its key's lock");
        assert_eq!(fx.staging_rows().await, 0, "a stalled PUT staged");
        assert_eq!(stalled.load(Ordering::SeqCst), 5, "not every PUT stalled");
    };
    tokio::select! {
        _ = puts => panic!("a stalled PUT ended"),
        () = probe => {}
    }
    assert_eq!(fx.rows().await, 1, "a stalled PUT wrote");
}

/// The hold audit of `GET …/kv/{key}`: its row's connection is released
/// before the object is read. Five GETs whose object reads never answer
/// hold no pool connection.
#[tokio::test(flavor = "multi_thread")]
async fn get_reads_the_value_holding_no_connection() {
    let Some(fx) = fx().await else { return };
    let keys: Vec<String> = (0..5).map(|i| format!("s{i}")).collect();
    fx.insert_live(STALL_NS, &keys).await;
    let start = STALLED_READS.load(Ordering::SeqCst);
    let paths: Vec<String> = keys.iter().map(|k| format!("/{k}")).collect();
    let gets = futures_util::future::join_all(paths.iter().map(|p| fx.get(STALL_NS, p)));
    let probe = async {
        let all = async {
            while STALLED_READS.load(Ordering::SeqCst) < start + 5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), all)
            .await
            .expect("the five GETs reached their object reads");
        assert!(
            fx.db.pool.num_idle() > 0,
            "the stalled GETs hold every connection"
        );
        let res = tokio::time::timeout(std::time::Duration::from_secs(5), fx.get(NS, ""))
            .await
            .expect("an unrelated GET waited");
        assert_eq!(res.status(), 200);
    };
    tokio::select! {
        _ = gets => panic!("a stalled GET ended"),
        () = probe => {}
    }
}

/// An API write whose key's lock is held waits `API_LOCK_WAIT` (5 s), then
/// answers 409 naming the key, writing nothing and keeping no connection;
/// the same for the delete and the bulk delete; released, the write lands.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_key_answers_409_after_the_lock_wait_holding_nothing() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "busy", "1").await;
    fx.ok_put(NS, "free", "1").await;
    let before = fx.table_state().await;
    let mut held = fx.db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(lock_text(fx.ws, NS, "busy"))
        .execute(&mut *held)
        .await
        .unwrap();
    let wait = egret_workflow::storage::nsfiles::access::API_LOCK_WAIT;
    let started = std::time::Instant::now();
    let res = tokio::time::timeout(wait * 4, fx.put(NS, "busy", "2"))
        .await
        .expect("the PUT's wait is bounded");
    let msg = refusal(res, 409).await;
    assert!(
        msg.contains("'busy'") && msg.contains("the key is busy") && msg.contains("try again"),
        "{msg}"
    );
    assert!(
        started.elapsed() >= wait,
        "it waited {:?}",
        started.elapsed()
    );
    assert!(refusal(fx.del(NS, "busy").await, 409)
        .await
        .contains("the key is busy"));
    // All or nothing: `busy` sorts first, so its wait runs out before
    // `free` is locked, and nothing is deleted.
    assert!(refusal(fx.bulk(NS, &["free", "busy"]).await, 409)
        .await
        .contains("'busy'"));
    assert_eq!(fx.table_state().await, before, "a busy write wrote");
    // The pool has its connections back (all but the one holding the lock).
    let recovered = async {
        while fx.db.pool.num_idle() + 1 < fx.db.pool.size() as usize {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), recovered)
        .await
        .expect("a refused write kept a connection");
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(lock_text(fx.ws, NS, "busy"))
        .fetch_one(&mut *held)
        .await
        .unwrap();
    assert!(released);
    fx.ok_put(NS, "busy", "2").await;
    assert_eq!(fx.latest(NS, "busy").await, Some((2, false)));
}

// ─── GET, revisions, inheritance ────────────────────────────────────────────

/// w9c `api.put_bodies` read back: each 200 cell's `get` (type and value
/// exact; `updated` masked), with Kestra's detail fields (`kv_detail`).
#[tokio::test]
async fn get_answers_kestras_detail_with_the_stored_type() {
    let Some(fx) = fx().await else { return };
    let fields = api("kv_detail")["fields"].clone();
    let mut examined = 0;
    for (name, cell) in api("put_bodies").as_object().unwrap() {
        if name.starts_with('_') || cell["put_status"] != 200 {
            continue;
        }
        examined += 1;
        let key = name.replace('_', "-");
        fx.ok_put(NS, &key, cell["body"].as_str().unwrap()).await;
        let d = ok_json(fx.get(NS, &format!("/{key}")).await).await;
        assert_eq!(d["type"], cell["get"]["type"], "{name}");
        assert_eq!(d["value"], cell["get"]["value"], "{name}");
        for f in fields.as_array().unwrap() {
            assert!(d.get(f.as_str().unwrap()).is_some(), "{name}: no {f}");
        }
        assert_eq!(d["revision"], json!(1), "{name}");
    }
    assert_eq!(examined, 46, "every 200 put_bodies cell is read");
}

/// R1 (w9c `api.routes.get_soft_deleted`, `get_soft_deleted_revision_1`):
/// a latest read of a soft-deleted key is 404 with Kestra's detail; its
/// revision is served (mutant e: the latest is served).
#[tokio::test]
async fn a_soft_deleted_key_is_404_latest_and_200_by_revision() {
    let Some(fx) = fx().await else { return };
    let routes = api("routes");
    fx.ok_put(NS, "r_del", "\"v1\"").await;
    assert_eq!(ok_json(fx.del(NS, "r_del").await).await, json!(true));
    let cell = &routes["get_soft_deleted"];
    assert_eq!(
        refusal(fx.get(NS, "/r_del").await, 404).await,
        cell["detail"].as_str().unwrap()
    );
    let cell = &routes["get_soft_deleted_revision_1"];
    let d = ok_json(fx.get(NS, "/r_del?revision=1").await).await;
    for f in ["type", "value", "revision"] {
        assert_eq!(d[f], cell[f], "{f}");
    }
    refusal(fx.get(NS, "/r_del?revision=2").await, 404).await;
}

/// Expired means missing (§4.5): a latest read and a revision read of an
/// expired key are 404, the listing leaves it out, `revisions` flags it, and
/// no read writes (the rows' hash and the objects are unchanged).
#[tokio::test]
async fn an_expired_key_or_revision_is_404_and_the_get_writes_nothing() {
    let Some(fx) = fx().await else { return };
    let res = fx
        .put_as(fx.user, NS, "gone", "1", &[TEXT, ("ttl", b"PT0S")])
        .await;
    assert_eq!(res.status(), 200);
    fx.ok_put(NS, "live", "1").await;
    let (before, objects) = (fx.table_state().await, fx.objects().await);
    assert_eq!(
        refusal(fx.get(NS, "/gone").await, 404).await,
        format!("No value found for key 'gone' in namespace '{NS}'")
    );
    refusal(fx.get(NS, "/gone?revision=1").await, 404).await;
    assert_eq!(keys_of(&ok_json(fx.get(NS, "").await).await), ["live"]);
    let revisions = ok_json(fx.get(NS, "/gone/revisions").await).await;
    assert_eq!(revisions[0]["expired"], json!(true));
    assert_eq!(revisions[0]["deleted"], json!(false));
    assert_eq!(fx.table_state().await, before, "a read wrote");
    assert_eq!(fx.objects().await, objects, "a read touched an object");
}

/// w9c `api.routes.revisions`: every revision, its deleted flag exact.
#[tokio::test]
async fn revisions_lists_every_revision_flagged() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "r", "\"v1\"").await;
    fx.ok_put(NS, "r", "{a:1}").await;
    assert_eq!(ok_json(fx.del(NS, "r").await).await, json!(true));
    let got = ok_json(fx.get(NS, "/r/revisions").await).await;
    let want = &api("routes")["revisions"]["revisions"];
    let pairs: Vec<Value> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"revision": r["revision"], "deleted": r["deleted"]}))
        .collect();
    assert_eq!(&Value::Array(pairs), want);
    assert_eq!(got[0]["type"], json!("STRING"));
    assert_eq!(got[1]["type"], json!("JSON"));
    assert_eq!(got[1]["size"], json!(5));
    assert!(got[0]["created"].as_str().unwrap() <= got[1]["created"].as_str().unwrap());
    assert_eq!(got[1]["expired"], json!(false));
}

/// w9c `inheritance.api_inheritance`: the seeds of `inheritance._setup`
/// written through the API (soft-deleted by DELETE, expired by `ttl: PT0S`),
/// then each namespace's listing: its ancestors' namespaces in order,
/// deepest first, and each one's keys as a set.
#[tokio::test]
async fn inheritance_lists_ancestors_deepest_first_first_wins() {
    let Some(fx) = fx().await else { return };
    let put = |ns: &'static str, key: String, ttl: Option<&'static str>| {
        let fx = &fx;
        async move {
            let mut h: Vec<(&str, &[u8])> = vec![TEXT];
            if let Some(t) = ttl {
                h.push(("ttl", t.as_bytes()));
            }
            let res = fx.put_as(fx.user, ns, &key, "1", &h).await;
            assert_eq!(res.status(), 200, "{ns}/{key}");
        }
    };
    put("w9c.inh", "top".into(), None).await;
    put("w9c.inh.a", "mid".into(), None).await;
    put("w9c.inh", "mid".into(), None).await;
    put("w9c.inh.a.b", "own".into(), None).await;
    put("w9c.inh", "own".into(), None).await;
    put("w9c.inh.a.b.c", "child".into(), None).await;
    put("w9c", "root".into(), None).await;
    put("w9c.inh.a", "delmid".into(), None).await;
    assert_eq!(
        ok_json(fx.del("w9c.inh.a", "delmid").await).await,
        json!(true)
    );
    put("w9c.inh", "delmid".into(), None).await;
    for n in 1..=6 {
        put("w9c.inh.a.b", format!("deep{n}"), Some("PT0S")).await;
        put("w9c.inh", format!("deep{n}"), None).await;
    }
    put("w9c.inh.a", "midexp".into(), Some("PT0S")).await;
    put("w9c.inh", "midexp".into(), None).await;
    let cells = fixture()["egret_expected"]["inheritance"]["api_inheritance"].clone();
    let mut examined = 0;
    for (ns, cell) in cells.as_object().unwrap() {
        if ns.starts_with('_') {
            continue;
        }
        examined += 1;
        let got = ok_json(fx.get(ns, "/inheritance").await).await;
        let mut order: Vec<String> = Vec::new();
        let mut entries: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            Default::default();
        for e in got.as_array().unwrap() {
            let from = e["namespace"].as_str().unwrap().to_string();
            if order.last() != Some(&from) {
                assert!(!order.contains(&from), "{ns}: {from} listed in two runs");
                order.push(from.clone());
            }
            let fresh = entries
                .entry(from)
                .or_default()
                .insert(e["key"].as_str().unwrap().to_string());
            assert!(fresh, "{ns}: a key listed twice");
            for f in ["type", "size", "revision", "updated"] {
                assert!(e.get(f).is_some(), "{ns}: no {f}");
            }
        }
        let want_order: Vec<String> =
            serde_json::from_value(cell["namespaces_in_order"].clone()).unwrap();
        assert_eq!(order, want_order, "{ns}");
        let want: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            serde_json::from_value(cell["entries"].clone()).unwrap();
        assert_eq!(entries, want, "{ns}");
        // First wins: no key appears under two namespaces.
        let all: Vec<&String> = entries.values().flatten().collect();
        let distinct: std::collections::BTreeSet<&&String> = all.iter().collect();
        assert_eq!(all.len(), distinct.len(), "{ns}");
    }
    assert_eq!(examined, 5);
}

/// Spec §9, §15 (Kestra's collision too): a key named `inheritance` is
/// written, listed, deleted and has revisions, but `GET …/kv/inheritance`
/// is the inheritance listing, never its value.
#[tokio::test]
async fn a_key_named_inheritance_is_written_and_deleted_and_its_get_is_the_listing() {
    let Some(fx) = fx().await else { return };
    let ns = "w9c.coll.child";
    fx.ok_put("w9c.coll", "p", "1").await;
    fx.ok_put(ns, "inheritance", "\"mine\"").await;
    assert_eq!(fx.latest(ns, "inheritance").await, Some((1, false)));
    assert_eq!(
        keys_of(&ok_json(fx.get(ns, "").await).await),
        ["inheritance"]
    );
    // The GET is the parents' listing.
    let got = ok_json(fx.get(ns, "/inheritance").await).await;
    assert_eq!(keys_of(&got), ["p"]);
    assert_eq!(got[0]["namespace"], json!("w9c.coll"));
    let revisions = ok_json(fx.get(ns, "/inheritance/revisions").await).await;
    assert_eq!(revisions.as_array().unwrap().len(), 1);
    // Its value is read with a letter percent-encoded (fix round 1, review
    // M3; spec §9, §15): the router matches the path as sent, and the key
    // is decoded afterwards.
    let d = ok_json(fx.get(ns, "/inheritanc%65").await).await;
    assert_eq!(
        (d["type"].clone(), d["value"].clone()),
        (json!("STRING"), json!("mine"))
    );
    let revisions = ok_json(fx.get(ns, "/%69nheritance/revisions").await).await;
    assert_eq!(revisions.as_array().unwrap().len(), 1);
    // A second PUT is revision 2; a revision read is still the listing.
    fx.ok_put(ns, "inheritance", "2").await;
    assert_eq!(fx.latest(ns, "inheritance").await, Some((2, false)));
    assert!(ok_json(fx.get(ns, "/inheritance?revision=1").await)
        .await
        .is_array());
    assert_eq!(ok_json(fx.del(ns, "inheritance").await).await, json!(true));
    assert_eq!(ok_json(fx.del(ns, "inheritance").await).await, json!(false));
    assert_eq!(fx.latest(ns, "inheritance").await, Some((2, true)));
    assert_eq!(ok_json(fx.get(ns, "").await).await, json!([]));
}

// ─── DELETE ─────────────────────────────────────────────────────────────────

/// w9c `api.routes.delete_live`, `delete_again`, `delete_missing`,
/// `delete_invalid_key`.
#[tokio::test]
async fn delete_answers_true_then_false() {
    let Some(fx) = fx().await else { return };
    let routes = api("routes");
    fx.ok_put(NS, "r_live", "1").await;
    let res = fx.del(NS, "r_live").await;
    assert_eq!(
        res.status(),
        routes["delete_live"]["status"].as_u64().unwrap() as u16
    );
    assert_eq!(ok_json(res).await, routes["delete_live"]["body"]);
    assert_eq!(
        ok_json(fx.del(NS, "r_live").await).await,
        routes["delete_again"]["body"]
    );
    assert_eq!(
        ok_json(fx.del(NS, "r_never").await).await,
        routes["delete_missing"]["body"]
    );
    refusal(fx.del(NS, "_bad").await, 400).await;
    // An expired key is not live: false, and it stays as it was.
    let res = fx
        .put_as(fx.user, NS, "exp", "1", &[TEXT, ("ttl", b"PT0S")])
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(ok_json(fx.del(NS, "exp").await).await, json!(false));
    assert_eq!(fx.latest(NS, "exp").await, Some((1, false)));
}

/// w9c `api.routes.bulk_*`: the live keys deleted and listed, a body with a
/// bad key refused whole, and all or nothing when a key is busy (mutant f:
/// a commit per key deletes the keys before the busy one).
#[tokio::test(flavor = "multi_thread")]
async fn bulk_delete_is_all_or_nothing_and_lists_the_live_keys() {
    let Some(fx) = fx().await else { return };
    let routes = api("routes");
    fx.ok_put(NS, "r_live", "1").await;
    fx.ok_put(NS, "r_del", "1").await;
    assert_eq!(ok_json(fx.del(NS, "r_del").await).await, json!(true));
    let cell = &routes["bulk_live_missing_deleted"];
    let res = fx
        .bulk_as(fx.user, NS, Some(cell["request"].to_string().as_bytes()))
        .await;
    assert_eq!(ok_json(res).await, cell["body"]);
    assert_eq!(fx.latest(NS, "r_live").await, Some((1, true)));
    assert_eq!(
        ok_json(fx.bulk(NS, &[]).await).await,
        routes["bulk_empty"]["body"]
    );
    // A bad key: 400, nothing deleted (r_live2 still live).
    fx.ok_put(NS, "r_live2", "1").await;
    refusal(fx.bulk(NS, &["r_live2", "_bad"]).await, 400).await;
    assert_eq!(fx.latest(NS, "r_live2").await, Some((1, false)));
    // Duplicates count once in the answer.
    fx.ok_put(NS, "dup", "1").await;
    assert_eq!(
        ok_json(fx.bulk(NS, &["dup", "dup"]).await).await,
        json!({"keys": ["dup"]})
    );
    // All or nothing: k0..k8 live, k9 busy. Keys are locked in ascending
    // order, so k0..k8 are processed before k9's wait runs out.
    let keys: Vec<String> = (0..10).map(|i| format!("k{i}")).collect();
    for k in &keys {
        fx.ok_put(NS, k, "1").await;
    }
    let mut held = fx.db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(lock_text(fx.ws, NS, "k9"))
        .execute(&mut *held)
        .await
        .unwrap();
    let all: Vec<&str> = keys.iter().map(String::as_str).collect();
    let msg = refusal(fx.bulk(NS, &all).await, 409).await;
    assert!(msg.contains("'k9'"), "{msg}");
    for k in &keys {
        assert_eq!(fx.latest(NS, k).await, Some((1, false)), "{k} was deleted");
    }
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut *held)
        .await
        .unwrap();
    drop(held);
    let res = fx.bulk(NS, &all).await;
    assert_eq!(ok_json(res).await, json!({ "keys": keys }));
}

/// Item 6, the user's ruling (2026-10-07): 1,000 live keys on :5440 with the
/// server's default `max_locks_per_transaction` are deleted in one request,
/// all listed; 1,001 are 413 naming 1,000, none deleted (mutant f2: `>=`
/// refuses the 1,000; no cap deletes the 1,001).
#[tokio::test(flavor = "multi_thread")]
async fn bulk_delete_of_1000_keys_is_accepted_and_1001_is_413_naming_the_cap() {
    let Some(fx) = fx().await else { return };
    let thousand: Vec<String> = (0..1000).map(|i| format!("k{i:04}")).collect();
    fx.insert_live(NS, &thousand).await;
    let all: Vec<&str> = thousand.iter().map(String::as_str).collect();
    let got = ok_json(fx.bulk(NS, &all).await).await;
    assert_eq!(got, json!({ "keys": thousand }));
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1 AND NOT deleted",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
    let more: Vec<String> = (0..1001).map(|i| format!("m{i:04}")).collect();
    fx.insert_live("w9c.more", &more).await;
    let all: Vec<&str> = more.iter().map(String::as_str).collect();
    let msg = refusal(fx.bulk("w9c.more", &all).await, 413).await;
    assert!(
        msg.contains("at most 1000 keys") && msg.contains("1001"),
        "{msg}"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.kv_entries \
         WHERE workspace_id = $1 AND namespace = 'w9c.more' AND NOT deleted",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(live, 1001, "a refused bulk delete deleted");
}

// ─── Isolation, caps, reads that never write ────────────────────────────────

/// Item 9, the user's ruling (2026-10-07): a namespace that only ANOTHER
/// workspace holds answers every route byte for byte as a namespace nobody
/// holds (its name substituted): listings `200 []`, key reads 404, deletes
/// `false` / `{keys: []}`; the other workspace's keys are untouched
/// (mutant g: the listing answers 404 for a namespace another workspace
/// holds).
#[tokio::test]
async fn another_workspaces_namespace_answers_byte_for_byte_as_an_empty_one() {
    let Some(fx) = fx().await else { return };
    // Workspace B holds `w9c.shared` and its child.
    let (other, owner) = common::seed_workspace(&fx.db).await;
    common::grant(&fx.db, other, owner, &["workflow:manage", "workflow:read"]).await;
    let b = Fx {
        db: fx.db.clone(),
        ws: other,
        user: owner,
        app: common::app(&fx.db).await,
    };
    for ns in ["w9c.shared", "w9c.shared.child"] {
        b.ok_put(ns, "k", "1").await;
        b.ok_put(ns, "k", "2").await;
    }
    let b_before = b.table_state().await;
    let held = "w9c.shared";
    let empty = format!("w9c.n{}", Uuid::new_v4().simple());
    let answers = |ns: String| {
        let fx = &fx;
        async move {
            let mut out = Vec::new();
            for res in [
                fx.get(&ns, "").await,
                fx.get(&format!("{ns}.child"), "/inheritance").await,
                fx.get(&ns, "/k").await,
                fx.get(&ns, "/k?revision=1").await,
                fx.get(&ns, "/k/revisions").await,
                fx.del(&ns, "k").await,
                fx.bulk(&ns, &["k"]).await,
            ] {
                let status = res.status().as_u16();
                out.push((status, text(res).await.replace(&ns, "<ns>")));
            }
            out
        }
    };
    let foreign = answers(held.to_string()).await;
    let blank = answers(empty).await;
    assert_eq!(foreign, blank);
    assert_eq!(foreign[0], (200, "[]".to_string()));
    assert_eq!(foreign[1], (200, "[]".to_string()));
    assert_eq!(foreign[2].0, 404);
    assert_eq!(foreign[5], (200, "false".to_string()));
    assert_eq!(foreign[6], (200, "{\"keys\":[]}".to_string()));
    assert_eq!(
        b.table_state().await,
        b_before,
        "workspace B's keys changed"
    );
}

/// `max_namespace_files` (16 here) bounds the listing and the inheritance
/// listing, each refused naming it.
#[tokio::test]
async fn listings_past_max_namespace_files_are_413() {
    let Some(fx) = fx().await else { return };
    let ns = "w9c.cap";
    let at_cap: Vec<String> = (0..MAX_FILES).map(|i| format!("k{i:02}")).collect();
    fx.insert_live(ns, &at_cap).await;
    assert_eq!(keys_of(&ok_json(fx.get(ns, "").await).await), at_cap);
    assert_eq!(
        ok_json(fx.get(&format!("{ns}.c"), "/inheritance").await)
            .await
            .as_array()
            .unwrap()
            .len(),
        MAX_FILES as usize
    );
    fx.insert_live(ns, &["over".to_string()]).await;
    let msg = refusal(fx.get(ns, "").await, 413).await;
    assert!(
        msg.contains("max_namespace_files") && msg.contains("more than 16 keys"),
        "{msg}"
    );
    let msg = refusal(fx.get(&format!("{ns}.c"), "/inheritance").await, 413).await;
    assert!(msg.contains("max_namespace_files"), "{msg}");
}

/// §4.5: no GET route writes. The rows' hash, the staging table and the
/// objects are the same before and after every GET, of live, deleted and
/// expired keys, latest and by revision.
#[tokio::test]
async fn no_get_route_writes() {
    let Some(fx) = fx().await else { return };
    let child = format!("{NS}.c");
    fx.ok_put(NS, "live", "1").await;
    fx.ok_put(NS, "gone", "1").await;
    assert_eq!(ok_json(fx.del(NS, "gone").await).await, json!(true));
    let res = fx
        .put_as(fx.user, NS, "exp", "1", &[TEXT, ("ttl", b"PT0S")])
        .await;
    assert_eq!(res.status(), 200);
    let before = (
        fx.table_state().await,
        fx.staging_rows().await,
        fx.objects().await,
    );
    for (ns, rest) in [
        (NS, ""),
        (child.as_str(), "/inheritance"),
        (NS, "/live"),
        (NS, "/gone"),
        (NS, "/gone?revision=1"),
        (NS, "/exp"),
        (NS, "/exp?revision=1"),
        (NS, "/never"),
        (NS, "/live/revisions"),
        (NS, "/gone/revisions"),
        (NS, "/exp/revisions"),
    ] {
        let _ = text(fx.get(ns, rest).await).await;
        let after = (
            fx.table_state().await,
            fx.staging_rows().await,
            fx.objects().await,
        );
        assert_eq!(after, before, "GET {ns}{rest} wrote");
    }
}

/// Plan item 14: one mutation per write, W9b's form; no value in a detail.
#[tokio::test]
async fn every_write_route_emits_one_audit_mutation() {
    let Some(fx) = fx().await else { return };
    let _ = common::audit_mutations(&fx.app);
    let secret = "\"the-value-never-audited\"";
    let res = fx
        .put_as(
            fx.user,
            NS,
            "a",
            secret,
            &[TEXT, ("ttl", b"PT1H"), ("description", b"d")],
        )
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(ok_json(fx.del(NS, "a").await).await, json!(true));
    assert_eq!(ok_json(fx.del(NS, "a").await).await, json!(false));
    fx.ok_put(NS, "b", "1").await;
    let _ = ok_json(fx.bulk(NS, &["b", "c"]).await).await;
    // A refusal is not a mutation.
    refusal(fx.put(NS, "_x", "1").await, 400).await;
    let got = common::audit_mutations(&fx.app);
    let actions: Vec<(&str, Option<&str>)> = got
        .iter()
        .map(|(a, id, _)| (a.as_str(), id.as_deref()))
        .collect();
    let a = format!("{NS}/a");
    let b = format!("{NS}/b");
    assert_eq!(
        actions,
        [
            ("workflow_namespace_kv:put", Some(a.as_str())),
            ("workflow_namespace_kv:delete", Some(a.as_str())),
            ("workflow_namespace_kv:delete", Some(a.as_str())),
            ("workflow_namespace_kv:put", Some(b.as_str())),
            ("workflow_namespace_kv:delete_many", Some(NS)),
        ]
    );
    let put = &got[0].2;
    assert_eq!(put["namespace"], json!(NS));
    assert_eq!(put["key"], json!("a"));
    assert_eq!(put["revision"], json!(1));
    assert_eq!(put["type"], json!("STRING"));
    assert!(put["expiresAt"].is_string());
    assert_eq!(got[1].2["deleted"], json!(true));
    assert_eq!(got[2].2["deleted"], json!(false));
    assert_eq!(got[4].2["keys"], json!(["b"]));
    assert_eq!(got[4].2["requested"], json!(2));
    for (_, _, detail) in &got {
        assert!(
            !detail.to_string().contains("the-value-never-audited"),
            "a value in an audit detail: {detail}"
        );
    }
}

/// An unreachable store is 503 naming the key, never the store's error; a
/// database outage behind an authenticated route is 503 with the fixed
/// message, never the database's error; nothing is written.
#[tokio::test]
async fn a_store_or_database_outage_is_503_and_the_database_error_is_never_sent() {
    let Some(fx) = fx().await else { return };
    // The store: the write's copy fails; a row's object read fails.
    let msg = refusal(fx.put(OUTAGE_NS, "k", "1").await, 503).await;
    assert!(
        msg.contains("'k'") && msg.contains(OUTAGE_NS) && !msg.contains("s3://"),
        "{msg}"
    );
    assert!(
        !msg.contains("dispatch") && !msg.contains("egret-test"),
        "{msg}"
    );
    assert_eq!(fx.latest(OUTAGE_NS, "k").await, None);
    assert_eq!(fx.staging_rows().await, 0, "the staging row is left");
    fx.insert_live(OUTAGE_NS, &["row".to_string()]).await;
    let msg = refusal(fx.get(OUTAGE_NS, "/row").await, 503).await;
    assert!(!msg.contains("dispatch") && !msg.contains("s3://"), "{msg}");
    // The database: the workflow pool is closed after the session resolved.
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let dead = Db::connect(&url, 1).await.unwrap();
    dead.pool.close().await;
    let app = common::app_with_workflow_db(&fx.db, &dead).await;
    let base = fx.base(NS);
    let json_ct: (&str, &[u8]) = ("content-type", b"application/json");
    let requests: Vec<Request> = vec![
        ("GET", base.clone(), vec![], vec![]),
        ("GET", format!("{base}/inheritance"), vec![], vec![]),
        ("GET", format!("{base}/k"), vec![], vec![]),
        ("GET", format!("{base}/k/revisions"), vec![], vec![]),
        ("PUT", format!("{base}/k"), vec![TEXT], b"1".to_vec()),
        ("DELETE", format!("{base}/k"), vec![], vec![]),
        (
            "DELETE",
            base.clone(),
            vec![json_ct],
            br#"{"keys":["k"]}"#.to_vec(),
        ),
    ];
    for (method, uri, headers, body) in requests {
        let res = common::send_with(&app, method, &uri, fx.user, &headers, body.into()).await;
        let msg = refusal(res, 503).await;
        assert_eq!(
            msg, "the KV store's index is unavailable; retry later",
            "{method} {uri}"
        );
    }
    assert_eq!(fx.rows().await, 1, "only the outage row");
}

/// w9c `api.routes`: the cells not asserted above (`get_missing`,
/// `get_missing_namespace`, `bulk_no_body`, `bulk_malformed`,
/// `list_namespace` in its order, `content_type_json`).
#[tokio::test]
async fn the_route_cells_hold() {
    let Some(fx) = fx().await else { return };
    let routes = api("routes");
    let cell = &routes["get_missing"];
    assert_eq!(
        refusal(fx.get(NS, "/r_never").await, 404).await,
        cell["detail"].as_str().unwrap()
    );
    let cell = &routes["get_missing_namespace"];
    assert_eq!(
        refusal(fx.get("w9c.api.nowhere", "/x").await, 404).await,
        cell["detail"].as_str().unwrap()
    );
    refusal(fx.bulk_as(fx.user, NS, None).await, 400).await;
    refusal(fx.bulk_as(fx.user, NS, Some(b"not json")).await, 400).await;
    let cell = &routes["list_namespace"];
    let ns = cell["namespace"].as_str().unwrap();
    let keys: Vec<String> = serde_json::from_value(cell["keys"].clone()).unwrap();
    // Written in the listed order: the listing is in the order the latest
    // revisions were written (w9c `get_keys._order`), not byte order.
    for k in &keys {
        fx.ok_put(ns, k, "1").await;
    }
    let got = keys_of(&ok_json(fx.get(ns, "").await).await);
    assert_eq!(got, keys, "the listing's order");
    let res = fx
        .put_as(
            fx.user,
            NS,
            "j",
            "1",
            &[("content-type", b"application/json")],
        )
        .await;
    assert_eq!(
        res.status().as_u16() as u64,
        routes["content_type_json"]["status"].as_u64().unwrap()
    );
    // `text/plain; charset=utf-8` and no Content-Type are taken.
    let res = fx
        .put_as(
            fx.user,
            NS,
            "j",
            "1",
            &[("content-type", b"text/plain; charset=utf-8")],
        )
        .await;
    assert_eq!(res.status(), 200);
    let res = fx.put_as(fx.user, NS, "j", "2", &[]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(fx.latest(NS, "j").await, Some((2, false)));
}

// ─── Fix round 1 ────────────────────────────────────────────────────────────

/// Review M8: the bulk body has its own cap, `MAX_BULK_BODY_BYTES` (1 MiB),
/// refused 413 with egret's text naming it (not axum's "length limit
/// exceeded"), nothing deleted; a body of exactly the cap is taken.
#[tokio::test]
async fn a_bulk_body_over_its_cap_is_413_naming_it() {
    let Some(fx) = fx().await else { return };
    let cap = egret_workflow::storage::kv::routes::MAX_BULK_BODY_BYTES;
    fx.ok_put(NS, "kept", "1").await;
    // `{"keys":["kept"]}` padded with whitespace to `len` bytes.
    let padded = |len: usize| {
        let head = br#"{"keys":["kept"]"#;
        let mut body = head.to_vec();
        body.resize(len - 1, b' ');
        body.push(b'}');
        body
    };
    let over = padded(cap + 1);
    let msg = refusal(fx.bulk_as(fx.user, NS, Some(over.as_slice())).await, 413).await;
    assert_eq!(
        msg,
        format!(
            "the bulk delete body is over {cap} bytes: it takes at most 1000 keys of at most \
             350 bytes each; nothing was deleted"
        )
    );
    assert_eq!(fx.latest(NS, "kept").await, Some((1, false)));
    let at = padded(cap);
    assert_eq!(at.len(), cap);
    let got = ok_json(fx.bulk_as(fx.user, NS, Some(at.as_slice())).await).await;
    assert_eq!(got, json!({"keys": ["kept"]}));
    assert_eq!(fx.latest(NS, "kept").await, Some((1, true)));
}

/// Review M6: a DATETIME whose offset carries it outside the years 1-9999
/// in UTC is 400 naming the timestamp and the reason, with none of ion-rs's
/// text, and nothing is written; the years' edges are stored.
#[tokio::test]
async fn a_datetime_outside_years_1_to_9999_in_utc_is_400_naming_it() {
    let Some(fx) = fx().await else { return };
    let before = fx.table_state().await;
    for (body, year) in [
        ("0001-01-01T00:00+01:00", 0),
        ("9999-12-31T23:59:59-01:00", 10000),
    ] {
        let msg = refusal(fx.put(NS, "far", body).await, 400).await;
        assert_eq!(
            msg,
            format!(
                "the timestamp {body} is in year {year} in UTC, outside the years 1-9999 a \
                 DATETIME can hold; nothing was stored"
            )
        );
    }
    assert_eq!(
        fx.table_state().await,
        before,
        "a refused timestamp was written"
    );
    for (body, utc) in [
        ("0001-01-01T00:00-01:00", "0001-01-01T01:00:00Z"),
        ("9999-12-31T23:59:59+01:00", "9999-12-31T22:59:59Z"),
    ] {
        fx.ok_put(NS, "edge", body).await;
        let d = ok_json(fx.get(NS, "/edge").await).await;
        assert_eq!(
            (d["type"].clone(), d["value"].clone()),
            (json!("DATETIME"), json!(utc)),
            "{body}"
        );
    }
}

/// Tick until nothing is claimable, with the core and `core.kv` tasks.
async fn drain(pool: &sqlx::PgPool) {
    let mut reg = Handlers::with_core_tasks(storage::current());
    kvt::register(&mut reg, pool.clone(), storage::current());
    for _ in 0..512 {
        let ticked = tick(pool, 16, &reg).await;
        assert!(ticked.is_ok(), "a worker error: {ticked:?}");
        if ticked.unwrap() == 0 {
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

/// Review I1: every w9c `api.api_written_read_by_flow` cell. Its body (the
/// body of `api.put_bodies.<cell>`) is PUT through the route, then one flow,
/// run by the worker, reads each key with `kv()` (a `Return` rendering
/// `{{ kv('<key>') }}`, a real render scope) and with `Get`: the render is
/// `kv_prints` exactly, and `Get`'s output is `{"value": get_value}`, JSON
/// equal. The two decimals (`ion_float` is `1.5`, `dec_150` is `1.50`) are
/// the only API-written decimals any flow reads (mutants I1a, the decimal as
/// `kv()` renders it, and I1b, as `Get` outputs it, die here).
#[tokio::test(flavor = "multi_thread")]
async fn api_written_values_read_back_through_kv_and_get() {
    let Some(fx) = fx().await else { return };
    let ns = "w9c.flowread";
    let cells = api("api_written_read_by_flow");
    let bodies = api("put_bodies");
    let mut names: Vec<String> = cells
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !k.starts_with('_'))
        .cloned()
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "dec_150",
            "ion_float",
            "ion_null",
            "ion_struct",
            "ion_timestamp",
            "ts_time"
        ],
        "the family's cells"
    );
    let mut tasks = String::new();
    for name in &names {
        fx.ok_put(ns, name, bodies[name]["body"].as_str().unwrap())
            .await;
        let format = serde_json::to_string(&format!("{{{{ kv('{name}') }}}}")).unwrap();
        tasks.push_str(&format!(
            "  - id: r_{name}\n    type: io.kestra.plugin.core.debug.Return\n    \
             format: {format}\n  - id: g_{name}\n    type: {}\n    key: {name}\n",
            kvt::GET
        ));
    }
    let source = format!("id: readback\nnamespace: {ns}\ntasks:\n{tasks}");
    let ex = start_execution(&fx.db.pool, fx.ws, None, &source, json!({}))
        .await
        .unwrap();
    drain(&fx.db.pool).await;
    for name in &names {
        let cell = &cells[name];
        let (state, outputs, message) = result_of(&fx.db.pool, ex, &format!("r_{name}")).await;
        assert_eq!(state, "SUCCESS", "kv('{name}'): {message:?}");
        let printed = outputs.unwrap_or_default()["value"].clone();
        assert_eq!(
            printed.as_str(),
            Some(cell["kv_prints"].as_str().unwrap()),
            "kv('{name}') printed {printed}"
        );
        let (state, outputs, message) = result_of(&fx.db.pool, ex, &format!("g_{name}")).await;
        assert_eq!(state, "SUCCESS", "Get {name}: {message:?}");
        assert_eq!(
            outputs,
            Some(json!({"value": cell["get_value"]})),
            "Get {name}"
        );
    }
}

/// The status, then the whole `ErrorBody` (`error` and `code`).
async fn refusal_body(res: axum::response::Response, status: u16) -> Value {
    let got = res.status();
    let body = text(res).await;
    assert_eq!(got, status, "{body}");
    serde_json::from_str(&body).unwrap_or_else(|_| panic!("not JSON: {body}"))
}

const CREATE_ONLY: (&str, &[u8]) = ("if-none-match", b"*");

/// `If-None-Match: *` makes the PUT a create (follow-up to W7d): a key that
/// does not exist is written as revision 1.
#[tokio::test]
async fn a_create_only_put_writes_a_key_that_does_not_exist() {
    let Some(fx) = fx().await else { return };
    let res = fx
        .put_as(fx.user, NS, "c-new", "\"v1\"", &[TEXT, CREATE_ONLY])
        .await;
    assert_eq!(res.status(), 200, "{}", text(res).await);
    assert_eq!(fx.latest(NS, "c-new").await, Some((1, false)));
    let detail = ok_json(fx.get(NS, "/c-new").await).await;
    assert_eq!(detail["value"], json!("v1"));
}

/// A live key is not overwritten: 409 `kv_exists`, naming the key and its
/// revision, and nothing is written (rows hashed, objects listed, staging
/// emptied).
#[tokio::test]
async fn a_create_only_put_over_a_live_key_is_409_kv_exists_and_writes_nothing() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "c-live", "\"old\"").await;
    let (before, objects) = (fx.table_state().await, fx.objects().await);
    let res = fx
        .put_as(fx.user, NS, "c-live", "\"new\"", &[TEXT, CREATE_ONLY])
        .await;
    let body = refusal_body(res, 409).await;
    assert_eq!(body["code"], json!("kv_exists"), "{body}");
    assert_eq!(
        body["error"],
        json!(format!(
            "conflict: key 'c-live' already exists in namespace '{NS}' (revision 1); \
             nothing was written"
        ))
    );
    assert_eq!(
        fx.table_state().await,
        before,
        "a refused create wrote a row"
    );
    assert_eq!(
        fx.objects().await,
        objects,
        "a refused create left an object"
    );
    assert_eq!(
        fx.staging_rows().await,
        0,
        "a refused create left its staging row"
    );
    let detail = ok_json(fx.get(NS, "/c-live").await).await;
    assert_eq!(detail["value"], json!("old"));
}

/// The store's liveness rule: a key whose latest revision is soft-deleted or
/// expired does not exist, so a create succeeds -- as the next revision, the
/// history kept.
#[tokio::test]
async fn a_create_only_put_over_a_deleted_or_expired_key_writes_the_next_revision() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "c-del", "\"v1\"").await;
    assert_eq!(ok_json(fx.del(NS, "c-del").await).await, json!(true));
    let res = fx
        .put_as(fx.user, NS, "c-del", "\"v2\"", &[TEXT, CREATE_ONLY])
        .await;
    assert_eq!(res.status(), 200, "{}", text(res).await);
    assert_eq!(fx.latest(NS, "c-del").await, Some((2, false)));
    assert_eq!(
        ok_json(fx.get(NS, "/c-del").await).await["value"],
        json!("v2")
    );

    let res = fx
        .put_as(fx.user, NS, "c-exp", "\"v1\"", &[TEXT, ("ttl", b"PT0S")])
        .await;
    assert_eq!(res.status(), 200);
    let res = fx
        .put_as(fx.user, NS, "c-exp", "\"v2\"", &[TEXT, CREATE_ONLY])
        .await;
    assert_eq!(res.status(), 200, "{}", text(res).await);
    assert_eq!(fx.latest(NS, "c-exp").await, Some((2, false)));
    assert_eq!(
        ok_json(fx.get(NS, "/c-exp").await).await["value"],
        json!("v2")
    );
}

/// Without the header nothing changes: a live key is overwritten.
#[tokio::test]
async fn a_put_without_if_none_match_still_overwrites() {
    let Some(fx) = fx().await else { return };
    fx.ok_put(NS, "c-over", "\"old\"").await;
    fx.ok_put(NS, "c-over", "\"new\"").await;
    assert_eq!(fx.latest(NS, "c-over").await, Some((2, false)));
    assert_eq!(
        ok_json(fx.get(NS, "/c-over").await).await["value"],
        json!("new")
    );
}

/// KV has no entity tags: any `If-None-Match` but `*` is 400 and writes nothing.
#[tokio::test]
async fn an_if_none_match_other_than_a_star_is_400_and_writes_nothing() {
    let Some(fx) = fx().await else { return };
    for bad in [&b"\"abc\""[..], b"W/\"1\"", b"*, \"x\"", b""] {
        let res = fx
            .put_as(fx.user, NS, "c-bad", "1", &[TEXT, ("if-none-match", bad)])
            .await;
        assert_eq!(
            refusal(res, 400).await,
            "If-None-Match on a KV write takes only \"*\"",
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
    assert_eq!(fx.latest(NS, "c-bad").await, None);
}
