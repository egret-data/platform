//! W9a Task 8: the execution file API (spec §8) -- download, metas and
//! preview. Requires DATABASE_URL.
//!
//! **One store, installed once by this binary** ([`installed`]): a
//! `MemStore` behind [`Counting`], which
//! - counts the bytes every `get_stream` reader hands out ([`pulled`]), so a
//!   test can see that a preview stopped reading;
//! - serves a synthetic 64 MiB object under any key ending [`SYNTHETIC`],
//!   generated as it is read and never held, counting what it generated;
//! - fails every read under the namespace [`OUTAGE_NS`] as an unreachable S3
//!   does, with a message naming the bucket.
//!
//! Executions are bare rows inserted directly (as `tests/storage_index.rs`):
//! which files `{id}` serves is a property of the link columns. Each test
//! seeds its own workspace; nothing here touches MinIO (that is
//! `storage_files_api_s3_live`).

mod common;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use base64::Engine as _;
use egret_core::UserId;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::{self, index_task_files, uri, FileStore, StoredFile};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};
use uuid::Uuid;

const NS: &str = "w9a.t8";
const OUTAGE_NS: &str = "w9a.outage";
const FLOW: &str = "Files_Api";
const SYNTHETIC: &str = "/synthetic-64mib.bin";
const SYNTHETIC_BYTES: u64 = 64 << 20;

fn pattern(i: u64) -> u8 {
    (i % 251) as u8
}

/// A reader that counts what passes through it into a shared counter.
struct Counted {
    inner: Box<dyn AsyncBufRead + Send + Unpin>,
    count: Arc<AtomicU64>,
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        self.count.fetch_add(n as u64, Ordering::SeqCst);
        r
    }
}

impl AsyncBufRead for Counted {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        Pin::new(&mut self.get_mut().inner).poll_fill_buf(cx)
    }
    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        self.count.fetch_add(amt as u64, Ordering::SeqCst);
        Pin::new(&mut self.inner).consume(amt);
    }
}

/// `SYNTHETIC_BYTES` of [`pattern`], made as they are read.
struct Generated {
    at: u64,
    chunk: Vec<u8>,
    start: usize,
    generated: Arc<AtomicU64>,
}

impl AsyncRead for Generated {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let avail = match self.as_mut().poll_fill_buf(cx) {
            Poll::Ready(Ok(a)) => a,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };
        let n = avail.len().min(buf.remaining());
        buf.put_slice(&avail[..n]);
        self.consume(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for Generated {
    fn poll_fill_buf(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.start == this.chunk.len() && this.at < SYNTHETIC_BYTES {
            let n = (SYNTHETIC_BYTES - this.at).min(64 * 1024);
            this.chunk = (this.at..this.at + n).map(pattern).collect();
            this.start = 0;
            this.at += n;
            this.generated.fetch_add(n, Ordering::SeqCst);
        }
        Poll::Ready(Ok(&this.chunk[this.start..]))
    }
    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.get_mut().start += amt;
    }
}

struct Counting {
    mem: egret_storage::MemStore,
    pulled: Mutex<HashMap<String, Arc<AtomicU64>>>,
}

fn counter(object: &str) -> Arc<AtomicU64> {
    let store = COUNTING.get().expect("installed");
    store
        .pulled
        .lock()
        .unwrap()
        .entry(object.to_string())
        .or_default()
        .clone()
}

static COUNTING: std::sync::OnceLock<Arc<Counting>> = std::sync::OnceLock::new();

#[async_trait::async_trait]
impl ObjectStore for ArcCounting {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.mem.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.mem.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.mem.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.mem.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.0.mem.list(prefix, delimiter).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.0.mem.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        if uri.contains("/w9a/outage/") {
            return Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused (bucket egret-test)"
            )));
        }
        let inner: Box<dyn AsyncBufRead + Send + Unpin> = if uri.ends_with(SYNTHETIC) {
            Box::new(Generated {
                at: 0,
                chunk: Vec::new(),
                start: 0,
                generated: counter(&format!("{uri}#generated")),
            })
        } else if let Some(keep) = fail_after(uri) {
            // `….failN.<ext>`: the first N bytes, then an S3 read error.
            let bytes = self.0.mem.get(uri).await?;
            let head = bytes[..keep.min(bytes.len())].to_vec();
            Box::new(tokio::io::BufReader::new(tokio::io::AsyncReadExt::chain(
                std::io::Cursor::new(head),
                FailingRead,
            )))
        } else {
            self.0.mem.get_stream(uri).await?
        };
        Ok(Box::new(Counted {
            inner,
            count: counter(uri),
        }))
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.mem.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        self.0.mem.delete_prefix(uri).await
    }
}

struct ArcCounting(Arc<Counting>);

/// `N` in a file name `….failN.<ext>`.
fn fail_after(uri: &str) -> Option<usize> {
    let name = uri.rsplit('/').next()?;
    let rest = name.split(".fail").nth(1)?;
    rest.split('.').next()?.parse().ok()
}

/// A read that fails as S3 does mid-body.
struct FailingRead;

impl AsyncRead for FailingRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other(
            "s3://egret-test/…: connection reset",
        )))
    }
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let counting = COUNTING
            .get_or_init(|| {
                Arc::new(Counting {
                    mem: egret_storage::MemStore::new(),
                    pulled: Mutex::new(HashMap::new()),
                })
            })
            .clone();
        storage::install(Arc::new(FileStore {
            store: Arc::new(ArcCounting(counting)),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps::default(),
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

/// Bytes the store has handed out for `object` so far.
fn pulled(object: &str) -> u64 {
    counter(object).load(Ordering::SeqCst)
}

#[derive(Default, Clone, Copy)]
struct Link {
    iteration_of: Option<(Uuid, i32)>,
    trigger: Option<Uuid>,
    replay_of: Option<Uuid>,
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    fs: Arc<FileStore>,
    app: common::App,
}

async fn fx_with(perms: &[&str]) -> Option<Fx> {
    let db = common::db().await?;
    let fs = installed();
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, perms).await;
    let app = common::app(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        fs,
        app,
    })
}

async fn fx() -> Option<Fx> {
    fx_with(&["workflow:read"]).await
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Fx {
    async fn exec_in(&self, ws: Uuid, ns: &str, link: Link) -> Uuid {
        let (parent, run_ix, loop_index, loop_value) = match link.iteration_of {
            Some((p, i)) => (Some(p), Some(0), Some(i), Some(format!("v{i}"))),
            None => (None, None, None, None),
        };
        sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history, parent_execution_id, \
                parent_run_ix, loop_index, loop_value, trigger_execution_id, \
                replayed_from_execution_id) \
             VALUES ($1, $2, $3, 'id: f\ntasks: []\n', 'SUCCESS', \
                     '[\"CREATED\",\"SUCCESS\"]'::jsonb, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(ws)
        .bind(ns)
        .bind(FLOW)
        .bind(parent)
        .bind(run_ix)
        .bind(loop_index)
        .bind(loop_value)
        .bind(link.trigger)
        .bind(link.replay_of)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert execution")
    }

    async fn exec(&self, link: Link) -> Uuid {
        self.exec_in(self.ws, NS, link).await
    }

    /// A file a task run of `execution` (in `ws`) wrote: indexed with
    /// `size`, its object holding `bytes`.
    async fn file_in(
        &self,
        ws: Uuid,
        ns: &str,
        execution: Uuid,
        name: &str,
        bytes: &[u8],
        size: u64,
    ) -> String {
        let run: Uuid = sqlx::query_scalar(
            "INSERT INTO workflow.task_runs \
               (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
             VALUES ($1, (SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1), 0, \
                     'Write_It', 'io.kestra.plugin.core.storage.Write', 'SUCCESS', \
                     '[\"SUCCESS\"]'::jsonb) RETURNING id",
        )
        .bind(execution)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert task run");
        let uri = format!(
            "{}{name}",
            uri::task_prefix(ns, FLOW, execution, "Write_It", run).unwrap()
        );
        let mut conn = self.db.pool.acquire().await.unwrap();
        index_task_files(
            &mut conn,
            ws,
            run,
            &[StoredFile {
                uri: uri.clone(),
                size,
            }],
        )
        .await
        .expect("index the file");
        self.fs
            .store
            .put(&self.fs.object_uri(ws, &uri).unwrap(), bytes.to_vec())
            .await
            .unwrap();
        uri
    }

    async fn file(&self, execution: Uuid, name: &str, bytes: &[u8]) -> String {
        self.file_in(self.ws, NS, execution, name, bytes, bytes.len() as u64)
            .await
    }

    fn path(&self, id: Uuid, route: &str, uri: &str) -> String {
        format!(
            "/api/ws/{}/workflows/executions/{id}/{route}?path={}",
            self.ws,
            enc(uri)
        )
    }

    async fn get(&self, id: Uuid, route: &str, uri: &str) -> axum::response::Response {
        common::get(&self.app, &self.path(id, route, uri), self.user).await
    }

    async fn download(&self, id: Uuid, uri: &str) -> (u16, Vec<u8>) {
        let res = self.get(id, "file", uri).await;
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 16 << 20)
            .await
            .unwrap();
        (status, bytes.to_vec())
    }

    async fn preview(&self, id: Uuid, uri: &str, max_rows: Option<u32>) -> (u16, Value) {
        let mut path = self.path(id, "file/preview", uri);
        if let Some(m) = max_rows {
            path.push_str(&format!("&maxRows={m}"));
        }
        let res = common::get(&self.app, &path, self.user).await;
        let status = res.status().as_u16();
        // Up to 32 MiB: a preview the byte budget ended is about 10 MiB.
        let bytes = axum::body::to_bytes(res.into_body(), 32 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The three routes' statuses for `uri` through `id`.
    async fn statuses(&self, id: Uuid, uri: &str) -> [u16; 3] {
        let mut out = [0; 3];
        for (i, route) in ["file", "file/metas", "file/preview"].iter().enumerate() {
            let res = self.get(id, route, uri).await;
            assert!(
                res.headers().get("location").is_none(),
                "{route}: never a redirect"
            );
            out[i] = res.status().as_u16();
        }
        out
    }
}

fn oracle() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[tokio::test]
async fn workflow_read_downloads_and_the_headers_are_safe() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    // `w9a-storage.json` `file_api.files.jsonl`: Kestra's bytes and headers.
    let o = oracle();
    let jsonl = &o["egret_expected"]["file_api"]["files"]["jsonl"];
    let bytes = b64(jsonl["file"]["bytes_b64"].as_str().unwrap());
    let kestra_name = "2T7PTJb8vvcXwymU2Y73BK-data.jsonl";
    let uri = fx.file(e, kestra_name, &bytes).await;
    let res = fx.get(e, "file", &uri).await;
    assert_eq!(res.status(), 200);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(
        h["content-disposition"],
        jsonl["file"]["content_disposition"].as_str().unwrap()
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(body.to_vec(), bytes);

    // A name holding a quote, a `;` and (through its directory) a `/`.
    let uri = fx.file(e, "dir/x\"y;z.txt", b"hostile").await;
    let res = fx.get(e, "file", &uri).await;
    assert_eq!(res.status(), 200);
    let d = res.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        d,
        "attachment; filename=\"x_y_z.txt\"; filename*=utf-8''x%22y%3Bz.txt"
    );
    let quoted = d
        .strip_prefix("attachment; filename=\"")
        .and_then(|r| r.split_once('"'))
        .map(|(name, _)| name)
        .unwrap();
    for bad in ['"', ';', '/', '\r', '\n', '\\'] {
        assert!(!quoted.contains(bad), "{bad:?} in {quoted:?}");
    }
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn another_executions_file_is_404_with_no_redirect() {
    let Some(fx) = fx().await else { return };
    let a = fx.exec(Link::default()).await;
    let b = fx.exec(Link::default()).await;
    let theirs = fx.file(b, "theirs.txt", b"b's").await;
    let mine = fx.file(a, "mine.txt", b"a's").await;
    // Control: each is served through its own execution.
    assert_eq!(fx.statuses(b, &theirs).await, [200, 200, 200]);
    assert_eq!(fx.statuses(a, &mine).await, [200, 200, 200]);
    assert_eq!(fx.statuses(a, &theirs).await, [404, 404, 404]);
    // One answer for another execution's file, a missing one and a `..` one.
    let missing = mine.replace("mine.txt", "nope.txt");
    let dotdot = mine.replace("mine.txt", "../mine.txt");
    assert_eq!(fx.statuses(a, &missing).await, [404, 404, 404]);
    assert_eq!(fx.statuses(a, &dotdot).await, [404, 404, 404]);
    for u in [&theirs, &missing, &dotdot] {
        let body = common::json(fx.get(a, "file/metas", u).await).await;
        assert_eq!(
            body["error"],
            format!("not found: {u}: no such file of execution {a}"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn another_workspaces_execution_is_404() {
    let Some(fx) = fx().await else { return };
    let (other_ws, _) = common::seed_workspace(&fx.db).await;
    let foreign = fx.exec_in(other_ws, NS, Link::default()).await;
    let theirs = fx
        .file_in(other_ws, NS, foreign, "x.txt", b"foreign", 7)
        .await;
    // The foreign id through this workspace, and this workspace's execution
    // asked for the foreign file: both 404.
    assert_eq!(fx.statuses(foreign, &theirs).await, [404, 404, 404]);
    let mine = fx.exec(Link::default()).await;
    assert_eq!(fx.statuses(mine, &theirs).await, [404, 404, 404]);
    // Control: the same URI shape in this workspace is served.
    let own = fx.file(mine, "x.txt", b"own").await;
    assert_eq!(fx.statuses(mine, &own).await, [200, 200, 200]);
}

#[tokio::test]
async fn without_workflow_read_it_is_403() {
    let Some(fx) = fx_with(&["workflow:manage"]).await else {
        return;
    };
    let e = fx.exec(Link::default()).await;
    let uri = fx.file(e, "a.txt", b"a").await;
    assert_eq!(fx.statuses(e, &uri).await, [403, 403, 403]);
    // Control: another user of the workspace, granted workflow:read.
    let (_, reader) = common::seed_workspace(&fx.db).await;
    common::grant(&fx.db, fx.ws, reader, &["workflow:read"]).await;
    // A fresh app: its PDP loads the grants as they are now.
    let app = common::app(&fx.db).await;
    for route in ["file", "file/metas", "file/preview"] {
        let res = common::get(&app, &fx.path(e, route, &uri), reader).await;
        assert_eq!(res.status(), 200, "{route}");
    }
}

#[tokio::test]
async fn any_id_of_the_loop_family_serves_the_familys_files() {
    let Some(fx) = fx().await else { return };
    let root = fx.exec(Link::default()).await;
    let it0 = fx
        .exec(Link {
            iteration_of: Some((root, 0)),
            ..Link::default()
        })
        .await;
    let it1 = fx
        .exec(Link {
            iteration_of: Some((root, 1)),
            ..Link::default()
        })
        .await;
    let in_root = fx.file(root, "root.txt", b"root").await;
    let in_it0 = fx.file(it0, "it0.txt", b"written in iteration").await;
    let in_it1 = fx.file(it1, "it1.txt", b"sibling").await;
    // `file_api.loop_iteration_file_via_root`: 200, the iteration's bytes.
    assert_eq!(
        fx.download(root, &in_it0).await,
        (200, b"written in iteration".to_vec())
    );
    assert_eq!(fx.statuses(root, &in_it0).await, [200, 200, 200]);
    assert_eq!(fx.statuses(it0, &in_root).await, [200, 200, 200]);
    assert_eq!(fx.statuses(it0, &in_it1).await, [200, 200, 200]);
    assert_eq!(fx.download(it0, &in_it1).await, (200, b"sibling".to_vec()));
    // Control: an unrelated execution's id serves none of them.
    let other = fx.exec(Link::default()).await;
    assert_eq!(fx.statuses(other, &in_it0).await, [404, 404, 404]);
}

#[tokio::test]
async fn a_trigger_executions_file_is_404_through_this_id() {
    let Some(fx) = fx().await else { return };
    let trigger = fx.exec(Link::default()).await;
    let child = fx
        .exec(Link {
            trigger: Some(trigger),
            ..Link::default()
        })
        .await;
    let source = fx.exec(Link::default()).await;
    let replay = fx
        .exec(Link {
            replay_of: Some(source),
            ..Link::default()
        })
        .await;
    let trigger_file = fx.file(trigger, "t.txt", b"trigger's").await;
    let source_file = fx.file(source, "s.txt", b"source's").await;
    // Both ARE readable while the child / the replay renders ...
    let mut conn = fx.db.pool.acquire().await.unwrap();
    assert!(storage::readable_executions(&mut conn, fx.ws, child)
        .await
        .unwrap()
        .contains(&trigger));
    assert!(storage::readable_executions(&mut conn, fx.ws, replay)
        .await
        .unwrap()
        .contains(&source));
    drop(conn);
    // ... and are not served through its id; by their own id they are.
    assert_eq!(fx.statuses(child, &trigger_file).await, [404, 404, 404]);
    assert_eq!(fx.statuses(replay, &source_file).await, [404, 404, 404]);
    assert_eq!(fx.statuses(trigger, &trigger_file).await, [200, 200, 200]);
    assert_eq!(fx.statuses(source, &source_file).await, [200, 200, 200]);
}

#[tokio::test]
async fn metas_is_the_index_size() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    // The index says 12345; the object holds 5 bytes. Metas answers the index.
    let uri = fx.file_in(fx.ws, NS, e, "m.txt", b"12345", 12345).await;
    let res = fx.get(e, "file/metas", &uri).await;
    assert_eq!(res.status(), 200);
    assert_eq!(common::json(res).await, json!({"size": 12345}));
}

#[tokio::test]
async fn ion_preview_rows_match_the_artifact_and_max_rows_caps_at_5000() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    let o = oracle();
    let fa = &o["egret_expected"]["file_api"];
    let text = fa["ion_preview_source_text"].as_str().unwrap();
    let uri = fx.file(e, "6838582355128244508.ion", text.as_bytes()).await;
    let (status, body) = fx.preview(e, &uri, None).await;
    assert_eq!(status, 200);
    assert_eq!(body["previewable"], true);
    assert_eq!(body["rows"], fa["preview_maxRows_99999"]["body"]["content"]);
    assert_eq!(body["truncated"], false);
    let (_, body) = fx.preview(e, &uri, Some(2)).await;
    assert_eq!(
        body["rows"],
        fa["files"]["ion"]["preview_maxRows_2"]["body"]["content"]
    );
    assert_eq!(body["truncated"], true);

    // A binary STORE file (`ion_rows`, every row with its own version marker):
    // the decoded values, never the bytes.
    let mut binary = Vec::new();
    let mut want = Vec::new();
    for r in o["egret_expected"]["ion_rows"].as_array().unwrap() {
        binary.extend(b64(r["bytes_b64"].as_str().unwrap()));
        let (v, _) = storage::ion::values_to_json(
            std::io::Cursor::new(r["ion_text"].as_str().unwrap().to_string()),
            1,
        )
        .unwrap();
        want.extend(v);
    }
    let uri = fx.file(e, "store.ion", &binary).await;
    let (status, body) = fx.preview(e, &uri, None).await;
    assert_eq!(status, 200);
    assert_eq!(body["rows"], Value::Array(want));
    assert_eq!(body["rows"][6], json!({"v": "2026-01-02T03:04:05.123456"}));
    assert_eq!(body["rows"][9], json!({}));

    // 6000 values: the default is 100, and any maxRows above 5000 is 5000.
    let many: String = (0..6000).map(|i| format!("{{i:{i}}}\n")).collect();
    let uri = fx.file(e, "many.ion", many.as_bytes()).await;
    let (_, body) = fx.preview(e, &uri, None).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 100);
    let (_, body) = fx.preview(e, &uri, Some(99999)).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 5000);
    assert_eq!(body["rows"][4999], json!({"i": 4999}));
    assert_eq!(body["truncated"], true);
    let (_, body) = fx.preview(e, &uri, Some(5000)).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 5000);
}

#[tokio::test]
async fn csv_jsonl_and_text_preview_as_lines_and_binary_is_not_previewable() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    let o = oracle();
    let files = &o["egret_expected"]["file_api"]["files"];
    let bytes = |k: &str| b64(files[k]["file"]["bytes_b64"].as_str().unwrap());
    let cases: [(&str, Vec<u8>, Value); 5] = [
        (
            "data.csv",
            bytes("csv"),
            json!(["a,b", "1,x", "2,y", "3,z"]),
        ),
        (
            "data.jsonl",
            bytes("jsonl"),
            json!(["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]),
        ),
        (
            "notes.txt",
            bytes("txt"),
            json!(["line one", "line two", "line three"]),
        ),
        (
            "page.json",
            b"{\"k\":[1,2]}".to_vec(),
            json!(["{\"k\":[1,2]}"]),
        ),
        ("noext", bytes("no_extension"), json!(["no extension here"])),
    ];
    for (name, content, rows) in cases {
        let uri = fx.file(e, name, &content).await;
        let (status, body) = fx.preview(e, &uri, None).await;
        assert_eq!(status, 200, "{name}");
        assert_eq!(
            body,
            json!({"previewable": true, "rows": rows, "truncated": false}),
            "{name}"
        );
    }
    // `files.txt.preview_maxRows_2`: two lines, truncated.
    let uri = fx.file(e, "two.txt", &bytes("txt")).await;
    let (_, body) = fx.preview(e, &uri, Some(2)).await;
    assert_eq!(
        body,
        json!({"previewable": true, "rows": ["line one", "line two"], "truncated": true})
    );
    // CRLF lines lose the CR.
    let uri = fx.file(e, "crlf.csv", b"a\r\nb\r\n").await;
    assert_eq!(fx.preview(e, &uri, None).await.1["rows"], json!(["a", "b"]));
    // `files.bin.preview` (chosen): not previewable.
    let uri = fx.file(e, "blob.bin", &bytes("bin")).await;
    assert_eq!(
        fx.preview(e, &uri, None).await,
        (
            200,
            json!({"previewable": false, "rows": [], "truncated": false})
        )
    );
    // Ion that does not parse is not previewable either.
    let uri = fx.file(e, "bad.ion", b"{a:1} {b:").await;
    assert_eq!(fx.preview(e, &uri, None).await.1["previewable"], false);
    // The download of the binary file is its bytes.
    let uri = fx.file(e, "blob2.bin", &bytes("bin")).await;
    assert_eq!(fx.download(e, &uri).await, (200, bytes("bin")));
}

#[tokio::test]
async fn preview_stops_reading_once_full() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    // About 4 MiB each, as lines and as Ion.
    let lines: String = (0..200_000).map(|i| format!("row {i:014}\n")).collect();
    let ion: String = (0..200_000)
        .map(|i| format!("{{i:{i},p:\"{i:08}\"}}\n"))
        .collect();
    for (name, content) in [("big.txt", lines), ("big.ion", ion)] {
        let uri = fx.file(e, name, content.as_bytes()).await;
        let object = fx.fs.object_uri(fx.ws, &uri).unwrap();
        let (status, body) = fx.preview(e, &uri, Some(2)).await;
        assert_eq!(status, 200, "{name}");
        assert_eq!(body["rows"].as_array().unwrap().len(), 2, "{name}");
        assert_eq!(body["truncated"], true, "{name}");
        let read = pulled(&object);
        println!("{name}: {read} of {} bytes read for 2 rows", content.len());
        assert!(
            read > 0 && read < 256 * 1024,
            "{name}: {read} of {} bytes read",
            content.len()
        );
        // Control: a full download pulls every byte through the same counter.
        let before = pulled(&object);
        let (status, got) = fx.download(e, &uri).await;
        assert_eq!(status, 200);
        assert_eq!(got.len(), content.len());
        assert_eq!(pulled(&object) - before, content.len() as u64, "{name}");
    }
}

/// `workflow.storage.max_read_bytes` (10 MiB here) ends a preview that
/// `maxRows` would not: the rows read stand, `truncated` is set, a line or
/// an Ion value cut by the budget is not a row, and the store gives up no
/// more than the budget.
#[tokio::test]
async fn the_byte_budget_ends_a_preview_and_a_cut_row_is_not_a_row() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    let budget = fx.fs.caps.max_read_bytes;
    let pad = "x".repeat(4090);
    // 3000 rows of about 4 KiB: 12 MiB, so the budget ends the read near row 2560.
    let lines: String = (0..3000).map(|i| format!("{i:04}{pad}\n")).collect();
    let ion: String = (0..3000)
        .map(|i| format!("{{i:{i},p:\"{pad}\"}}\n"))
        .collect();
    for (name, content, row_bytes) in [("wide.txt", lines, 4095u64), ("wide.ion", ion, 0)] {
        let uri = fx.file(e, name, content.as_bytes()).await;
        let object = fx.fs.object_uri(fx.ws, &uri).unwrap();
        let (status, body) = fx.preview(e, &uri, Some(5000)).await;
        assert_eq!(status, 200, "{name}");
        assert_eq!(body["previewable"], true, "{name}");
        assert_eq!(body["truncated"], true, "{name}");
        let rows = body["rows"].as_array().unwrap();
        assert!(
            rows.len() > 2000 && rows.len() < 3000,
            "{name}: {} rows",
            rows.len()
        );
        if let Some(fit) = budget.checked_div(row_bytes) {
            // Every row whole: exactly the lines that fit in the budget.
            assert_eq!(rows.len() as u64, fit, "{name}");
            assert!(rows.iter().all(|r| r.as_str().unwrap().len() == 4094));
        } else {
            let last = rows.last().unwrap();
            assert_eq!(last["i"], json!(rows.len() - 1), "{name}");
            assert_eq!(last["p"].as_str().unwrap().len(), 4090, "{name}");
        }
        assert_eq!(
            pulled(&object),
            budget,
            "{name}: the store gave up the budget, no more"
        );
    }
}

#[tokio::test]
async fn a_store_outage_is_503_and_a_gone_object_is_404_never_naming_internals() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec_in(fx.ws, OUTAGE_NS, Link::default()).await;
    let uri = fx.file_in(fx.ws, OUTAGE_NS, e, "o.txt", b"x", 1).await;
    for route in ["file", "file/preview"] {
        let res = fx.get(e, route, &uri).await;
        assert_eq!(res.status(), 503, "{route}");
        let body = common::json(res).await;
        let msg = body["error"].as_str().unwrap();
        assert!(msg.contains(&uri), "{msg}");
        for internal in ["egret-test", "s3://", "workflow/", "connection refused"] {
            assert!(!msg.contains(internal), "{route}: {msg} names {internal}");
        }
    }
    // Metas reads the index only.
    assert_eq!(fx.get(e, "file/metas", &uri).await.status(), 200);

    // An index row whose object is gone (pruned): 404, not 500.
    let ok = fx.exec(Link::default()).await;
    let uri = fx.file(ok, "gone.txt", b"soon gone").await;
    assert_eq!(fx.statuses(ok, &uri).await, [200, 200, 200]);
    fx.fs
        .store
        .delete(&fx.fs.object_uri(fx.ws, &uri).unwrap())
        .await
        .unwrap();
    assert_eq!(fx.get(ok, "file", &uri).await.status(), 404);
    assert_eq!(fx.get(ok, "file/preview", &uri).await.status(), 404);
}

/// Fix round 1, C1: a file nested past `ion::MAX_DEPTH` aborted the whole
/// server on one preview. It is now `previewable: false`, text and binary,
/// and the server answers the next request.
#[tokio::test(flavor = "multi_thread")]
async fn a_deeply_nested_ion_file_is_not_previewable_and_the_server_lives() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    let text = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let mut binary = vec![0xE0, 0x01, 0x00, 0xEA];
    // 100_000 lists, each holding the next; sizes from the inside out.
    let mut headers: Vec<Vec<u8>> = vec![vec![0xB0]];
    let mut size = 1usize;
    for _ in 1..100_000 {
        let h = if size < 14 {
            vec![0xB0 | size as u8]
        } else {
            let mut g = Vec::new();
            let mut n = size;
            while n > 0 {
                g.push((n & 0x7F) as u8);
                n >>= 7;
            }
            g.reverse();
            *g.last_mut().unwrap() |= 0x80;
            let mut h = vec![0xBE];
            h.extend(g);
            h
        };
        size += h.len();
        headers.push(h);
    }
    for h in headers.iter().rev() {
        binary.extend(h);
    }
    for (name, bytes) in [("deep.ion", text.into_bytes()), ("deep-bin.ion", binary)] {
        let uri = fx.file(e, name, &bytes).await;
        assert_eq!(
            fx.preview(e, &uri, None).await,
            (
                200,
                json!({"previewable": false, "rows": [], "truncated": false})
            ),
            "{name}"
        );
    }
    // Fix round 3: an unterminated `/*` in an s-expression hides nothing.
    let unterminated = format!("(/* {}{})", "[".repeat(5000), "]".repeat(5000));
    let uri = fx
        .file(e, "unterminated.ion", unterminated.as_bytes())
        .await;
    assert_eq!(
        fx.preview(e, &uri, None).await,
        (
            200,
            json!({"previewable": false, "rows": [], "truncated": false})
        )
    );
    // Still serving.
    let ok = fx.file(e, "after.ion", b"{a:1}").await;
    assert_eq!(fx.preview(e, &ok, None).await.1["rows"], json!([{"a": 1}]));
}

/// Fix round 2: valid rows whose blobs hold `//` (a quarter of 1 KiB blobs
/// do) preview whole; the round-1 depth scan took `//` for a comment and
/// refused this file as too deep after 24 rows.
#[tokio::test]
async fn rows_with_slashes_in_their_blobs_preview_whole() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    let rows = "{a:{{ab//cA==}}, b:[1]}\n".repeat(60);
    let uri = fx.file(e, "blobs.ion", rows.as_bytes()).await;
    let (status, body) = fx.preview(e, &uri, None).await;
    assert_eq!(status, 200);
    assert_eq!(body["previewable"], true, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 60);
    assert_eq!(body["rows"][59], json!({"a": "ab//cA==", "b": [1]}));
}

/// Fix round 1, M5: once the page is full, a store error on the one-past
/// peek is not a 503; before the page is full it still is.
#[tokio::test]
async fn a_store_error_after_a_full_page_keeps_the_page() {
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    // Each file yields its first bytes, then the store fails: "a\nb\n", and
    // "1 2 3" (the Ion reader yields a value once it has seen the next one's
    // first byte; the third never completes).
    for (name, content, rows) in [
        ("lines.fail4.txt", &b"a\nb\nc\n"[..], json!(["a", "b"])),
        ("values.fail5.ion", &b"1 2 345"[..], json!([1, 2])),
    ] {
        let uri = fx.file(e, name, content).await;
        assert_eq!(
            fx.preview(e, &uri, Some(2)).await,
            (
                200,
                json!({"previewable": true, "rows": rows, "truncated": true})
            ),
            "{name}"
        );
        // Control: a page of 3 needs the bytes the store failed to give.
        assert_eq!(fx.preview(e, &uri, Some(3)).await.0, 503, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_larger_than_memory_budget_streams() {
    use http_body_util::BodyExt as _;
    let Some(fx) = fx().await else { return };
    let e = fx.exec(Link::default()).await;
    // Indexed at 64 MiB; the store generates the bytes as they are read.
    let uri = fx
        .file_in(fx.ws, NS, e, &SYNTHETIC[1..], b"", SYNTHETIC_BYTES)
        .await;
    let object = fx.fs.object_uri(fx.ws, &uri).unwrap();
    let generated = format!("{object}#generated");
    let res = fx.get(e, "file", &uri).await;
    assert_eq!(res.status(), 200);
    let mut body = res.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(!first.is_empty());
    // The response started after a sliver of the object was made.
    let at_first = pulled(&generated);
    println!("{at_first} of {SYNTHETIC_BYTES} bytes generated at the first frame");
    assert!(
        at_first < 1 << 20,
        "{at_first} bytes generated before the first frame"
    );
    let mut total = first.len() as u64;
    let mut ok = first
        .iter()
        .enumerate()
        .all(|(i, b)| *b == pattern(i as u64));
    while let Some(frame) = body.frame().await {
        let data = frame.unwrap().into_data().unwrap();
        ok &= data
            .iter()
            .enumerate()
            .all(|(i, b)| *b == pattern(total + i as u64));
        total += data.len() as u64;
    }
    assert_eq!(total, SYNTHETIC_BYTES);
    assert!(ok, "the bytes are the object's, in order");
    assert_eq!(pulled(&generated), SYNTHETIC_BYTES);
}
