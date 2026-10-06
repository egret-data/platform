//! W9b Task 8: the namespace file API (spec §9) -- nine routes under
//! `/api/ws/{workspace}/workflows/namespaces/{namespace}/files…`. Requires
//! DATABASE_URL.
//!
//! **One store, installed once by this binary** ([`installed`]): a
//! `MemStore` (every delete and move here runs in memory) with
//! `max_file_bytes` = [`CAP`] and `max_namespace_files` = [`MAX_FILES`], so
//! both caps can be crossed, and with [`Faulty`] in front of it: every read
//! and write under the namespace [`OUTAGE_NS`] fails as an unreachable S3
//! does. Each test seeds its own workspace, so no test sees another's rows.
//! The 64 MiB download from MinIO is `nsfiles_api_s3_live` (its own binary:
//! the store is installed once per process).

mod common;

use std::sync::Arc;

use egret_core::UserId;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::{self, FileStore};
use serde_json::{json, Value};
use uuid::Uuid;

const NS: &str = "w9b.api";
const OUTAGE_NS: &str = "w9b.outage";
/// `max_file_bytes` for this binary: the body limit is this + 1 MiB, above
/// the 900 KiB bodies of the header-budget test.
const CAP: u64 = 64 * 1024;
/// `max_namespace_files` for this binary.
const MAX_FILES: u64 = 8;

/// `MemStore`, failing every read and write under [`OUTAGE_NS`].
struct Faulty(egret_storage::MemStore);

fn in_outage(uri: &str) -> bool {
    uri.contains("/w9b/outage/")
}

fn outage(uri: &str) -> StorageError {
    StorageError::S3(format!(
        "{uri}: dispatch failure: connection refused (bucket egret-test)"
    ))
}

#[async_trait::async_trait]
impl ObjectStore for Faulty {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    // A namespace file's body reaches its key by a copy from its staging
    // object (fix round 1): the outage refuses the copy onto its key.
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
                max_file_bytes: CAP,
                max_namespace_files: MAX_FILES,
                ..Default::default()
            },
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
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

/// RFC 3986 query-value encoding: everything but unreserved characters and `/`.
fn enc(v: &str) -> String {
    let mut out = String::new();
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

const BOUNDARY: &str = "w9b-t8-boundary-7a2e";

fn multipart_file(part: &str, file_name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{part}\"; \
         filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(bytes);
    out.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    out
}

fn ct() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

impl Fx {
    fn base(&self, ns: &str) -> String {
        format!("/api/ws/{}/workflows/namespaces/{ns}/files", self.ws)
    }

    async fn upload_as(
        &self,
        user: UserId,
        ns: &str,
        path: &str,
        bytes: &[u8],
    ) -> axum::response::Response {
        let uri = format!("{}?path={}", self.base(ns), enc(path));
        // The part's file name is not the path (`?path` is): any will do.
        common::send_raw(
            &self.app,
            "POST",
            &uri,
            user,
            &ct(),
            multipart_file("fileContent", "upload.bin", bytes),
        )
        .await
    }

    async fn upload(&self, ns: &str, path: &str, bytes: &[u8]) -> axum::response::Response {
        self.upload_as(self.user, ns, path, bytes).await
    }

    async fn ok_upload(&self, ns: &str, path: &str, bytes: &[u8]) {
        let res = self.upload(ns, path, bytes).await;
        assert_eq!(res.status(), 200, "upload {path}: {}", text(res).await);
    }

    async fn get_as(&self, user: UserId, ns: &str, rest: &str) -> axum::response::Response {
        common::get(&self.app, &format!("{}{rest}", self.base(ns)), user).await
    }

    async fn get(&self, ns: &str, rest: &str) -> axum::response::Response {
        self.get_as(self.user, ns, rest).await
    }

    async fn mkdir_as(&self, user: UserId, ns: &str, path: &str) -> axum::response::Response {
        let uri = format!("{}/directory?path={}", self.base(ns), enc(path));
        common::post_no_body(&self.app, &uri, user).await
    }

    async fn mv_as(
        &self,
        user: UserId,
        ns: &str,
        from: &str,
        to: &str,
    ) -> axum::response::Response {
        let uri = format!("{}?from={}&to={}", self.base(ns), enc(from), enc(to));
        common::send_raw(&self.app, "PUT", &uri, user, "", Vec::new()).await
    }

    async fn mv(&self, ns: &str, from: &str, to: &str) -> axum::response::Response {
        self.mv_as(self.user, ns, from, to).await
    }

    async fn rm_as(&self, user: UserId, ns: &str, path: &str) -> axum::response::Response {
        let uri = format!("{}?path={}", self.base(ns), enc(path));
        common::delete(&self.app, &uri, user).await
    }

    async fn rm(&self, ns: &str, path: &str) -> axum::response::Response {
        self.rm_as(self.user, ns, path).await
    }

    async fn download(&self, ns: &str, path: &str) -> axum::response::Response {
        self.get(ns, &format!("?path={}", enc(path))).await
    }

    async fn listing(&self, ns: &str, dir: &str) -> Value {
        let res = self.get(ns, &format!("/directory?path={}", enc(dir))).await;
        assert_eq!(res.status(), 200, "directory {dir}");
        common::json(res).await
    }

    /// Every row of this workspace's namespace files.
    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM workflow.namespace_files WHERE workspace_id = $1")
            .bind(self.ws)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }
}

async fn text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 64 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The status, then the refusal's message.
async fn refusal(res: axum::response::Response, status: u16) -> String {
    let got = res.status().as_u16();
    let body = text(res).await;
    assert_eq!(got, status, "{body}");
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(body)
}

fn paths(listing: &Value) -> Vec<String> {
    let mut v: Vec<String> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

fn strings(v: &Value) -> Vec<String> {
    let mut v: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

fn fixture() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/oracle-exec/w9b-namespace-files.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn route(name: &str) -> Value {
    fixture()["egret_expected"]["api"]["routes"][name].clone()
}

/// The fixture's namespace as egret builds it (w9b `api.routes`): `/top.zip`
/// stored as one file (never expanded), `/dir1/sub/f.txt` at two revisions.
async fn fixture_namespace(fx: &Fx, ns: &str) -> Vec<u8> {
    fx.ok_upload(ns, "/dir1/sub/f.txt", b"hello").await;
    fx.ok_upload(ns, "/dir1/g.txt", b"gee").await;
    fx.ok_upload(ns, "/Other/CASE.TXT", b"case").await;
    let zip = zip_of(&[("inner.txt", b"inner"), ("zdir/inner2.txt", b"inner2")]);
    fx.ok_upload(ns, "/top.zip", &zip).await;
    fx.ok_upload(ns, "/del/a/b/only.txt", b"only").await;
    fx.ok_upload(ns, "/del2/a/b/only.txt", b"only2").await;
    fx.ok_upload(ns, "/dir1/sub/f.txt", b"hello v2").await;
    zip
}

// ─── A stored zip, built by hand (no zip crate in this workspace) ───────────

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A valid zip archive of `entries`, stored (no compression).
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in entries {
        let offset = out.len() as u32;
        let (crc, len, nlen) = (crc32(data), data.len() as u32, name.len() as u16);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&nlen.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&nlen.to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (cd_offset, cd_len, n) = (out.len() as u32, central.len() as u32, entries.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

// ─── Permissions, workspaces, namespaces, paths ─────────────────────────────

/// The five GETs need `workflow:read` and the four writes `workflow:manage`;
/// a reader's write is a 403 that writes nothing, and `workflow:manage` alone
/// reads nothing (it implies no `:read`).
#[tokio::test]
async fn read_routes_need_workflow_read_and_write_routes_need_workflow_manage() {
    let Some(mut fx) = fx().await else { return };
    fx.ok_upload(NS, "/a.txt", b"a").await;
    let reader = member(&fx.db, fx.ws, &["workflow:read"]).await;
    let manager_only = member(&fx.db, fx.ws, &["workflow:manage"]).await;
    // The PDP reads grants when it is built (`tests/executions_routes.rs`).
    fx.app = common::app(&fx.db).await;
    let gets = [
        "?path=/a.txt",
        "/stats?path=/a.txt",
        "/revisions?path=/a.txt",
        "/search?q=a",
        "/directory",
    ];
    for rest in gets {
        let res = fx.get_as(reader, NS, rest).await;
        assert_eq!(res.status(), 200, "reader GET {rest}");
        let res = fx.get_as(manager_only, NS, rest).await;
        assert_eq!(res.status(), 403, "manage-only GET {rest}");
    }
    let before = fx.rows().await;
    for (what, res) in [
        ("upload", fx.upload_as(reader, NS, "/b.txt", b"b").await),
        ("mkdir", fx.mkdir_as(reader, NS, "/d").await),
        ("move", fx.mv_as(reader, NS, "/a.txt", "/c.txt").await),
        ("delete", fx.rm_as(reader, NS, "/a.txt").await),
    ] {
        assert_eq!(res.status(), 403, "reader {what}");
    }
    assert_eq!(fx.rows().await, before, "a refused write wrote nothing");
    assert!(common::audit_mutations(&fx.app)
        .iter()
        .all(|m| m.0.ends_with(":upload")));
    assert_eq!(text(fx.download(NS, "/a.txt").await).await, "a");

    for (what, res) in [
        (
            "upload",
            fx.upload_as(manager_only, NS, "/b.txt", b"b").await,
        ),
        ("mkdir", fx.mkdir_as(manager_only, NS, "/d").await),
        ("move", fx.mv_as(manager_only, NS, "/a.txt", "/c.txt").await),
        ("delete", fx.rm_as(manager_only, NS, "/b.txt").await),
    ] {
        assert_eq!(res.status(), 200, "manager {what}: {}", text(res).await);
    }
    let root = fx.listing(NS, "/").await;
    assert_eq!(paths(&root), ["/c.txt", "/d/"]);
}

/// The download is W9a's: octet-stream, a sanitised attachment name (a quote
/// and a `;` cannot break out of the header) and nosniff. A line break cannot
/// reach the header at all: the path rule refuses control characters.
#[tokio::test]
async fn a_download_has_the_safe_headers() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/x/q\"a;b é.txt", b"payload").await;
    let res = fx.download(NS, "/x/q\"a;b é.txt").await;
    assert_eq!(res.status(), 200);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"q_a_b__.txt\"; filename*=utf-8''q%22a%3Bb%20%C3%A9.txt"
    );
    assert_eq!(text(res).await, "payload");
    // Without a leading slash, as Kestra (`api.routes.get_file_no_leading_slash`).
    assert_eq!(
        text(fx.download(NS, "x/q\"a;b é.txt").await).await,
        "payload"
    );

    let msg = refusal(fx.upload(NS, "/crlf\r\nX-Evil: 1.txt", b"x").await, 400).await;
    assert!(msg.contains("character"), "{msg}");
}

/// `{workspace}` is the session's: a file of workspace A is not found from
/// workspace B under the same namespace, and B's member cannot address A.
#[tokio::test]
async fn another_workspaces_namespace_file_is_404_and_an_unknown_path_is_404() {
    let Some(a) = fx().await else { return };
    let Some(b) = fx().await else { return };
    a.ok_upload(NS, "/secret.txt", b"a's").await;

    for rest in [
        "?path=/secret.txt",
        "/stats?path=/secret.txt",
        "/revisions?path=/secret.txt",
    ] {
        let msg = refusal(b.get(NS, rest).await, 404).await;
        assert!(!msg.contains(&a.ws.to_string()), "{msg}");
    }
    assert_eq!(
        common::json(b.get(NS, "/search?q=secret").await).await,
        json!([])
    );
    assert_eq!(common::json(b.get(NS, "/directory").await).await, json!([]));
    // B's member, A's workspace in the URL.
    let uri = format!("{}?path=/secret.txt", a.base(NS));
    let res = common::get(&b.app, &uri, b.user).await;
    assert!(
        matches!(res.status().as_u16(), 403 | 404),
        "{}",
        res.status()
    );
    let res = common::delete(&b.app, &uri, b.user).await;
    assert!(
        matches!(res.status().as_u16(), 403 | 404),
        "{}",
        res.status()
    );
    assert_eq!(text(a.download(NS, "/secret.txt").await).await, "a's");

    let msg = refusal(a.download(NS, "/nope.txt").await, 404).await;
    assert!(msg.contains("/nope.txt"), "{msg}");
    let msg = refusal(a.get(NS, "/stats?path=/nope.txt").await, 404).await;
    assert!(msg.contains("/nope.txt"), "{msg}");
}

/// `/_flows` is refused on every route that takes a path (w9b
/// `api.summary.flows_import`, chosen).
#[tokio::test]
async fn flows_paths_are_400_on_every_route() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/g.txt", b"g").await;
    let before = fx.rows().await;
    for (what, res) in [
        ("download", fx.download(NS, "/_flows/x.yml").await),
        ("stats", fx.get(NS, "/stats?path=/_flows/x.yml").await),
        (
            "revisions",
            fx.get(NS, "/revisions?path=/_flows/x.yml").await,
        ),
        ("directory", fx.get(NS, "/directory?path=/_flows").await),
        (
            "upload",
            fx.upload(NS, "/_flows/imported.yml", b"id: x").await,
        ),
        ("mkdir", fx.mkdir_as(fx.user, NS, "/_flows/d").await),
        ("move to", fx.mv(NS, "/g.txt", "/_flows/g.txt").await),
        ("move from", fx.mv(NS, "/_flows/g.txt", "/h.txt").await),
        ("delete", fx.rm(NS, "/_flows/x").await),
    ] {
        let msg = refusal(res, 400).await;
        assert!(msg.contains("_flows"), "{what}: {msg}");
    }
    assert_eq!(fx.rows().await, before);
}

/// A namespace with a `_files` segment, an empty segment or an invalid name
/// is refused on all nine routes, naming it (`api.summary.namespace_validation`).
#[tokio::test]
async fn a_files_segment_namespace_is_400_on_every_route_naming_it() {
    let Some(fx) = fx().await else { return };
    for (ns, says) in [
        ("w9b._files", "'_files'"),
        ("w9b..x", "empty segment"),
        ("W9B.Bad_ns", "must match"),
    ] {
        for (what, res) in [
            ("download", fx.download(ns, "/x.txt").await),
            ("stats", fx.get(ns, "/stats").await),
            ("revisions", fx.get(ns, "/revisions?path=/x.txt").await),
            ("search", fx.get(ns, "/search?q=x").await),
            ("directory", fx.get(ns, "/directory").await),
            ("upload", fx.upload(ns, "/x.txt", b"x").await),
            ("mkdir", fx.mkdir_as(fx.user, ns, "/d").await),
            ("move", fx.mv(ns, "/x.txt", "/y.txt").await),
            ("delete", fx.rm(ns, "/x.txt").await),
        ] {
            let msg = refusal(res, 400).await;
            assert!(
                msg.contains(&format!("'{ns}'")) && msg.contains(says),
                "{ns} {what}: {msg}"
            );
        }
    }
    assert_eq!(fx.rows().await, 0);
}

/// Each of `NsPath`'s rules is a 400 naming the path and the rule.
#[tokio::test]
async fn each_path_rule_is_400_naming_the_rule() {
    let Some(fx) = fx().await else { return };
    let long_segment = format!("/{}.txt", "s".repeat(256));
    let long_path = format!("/{}/f.txt", vec!["d".repeat(200); 3].join("/"));
    for (raw, rule) in [
        ("/a//b.txt", "empty segment"),
        ("/a/../b.txt", "'..' segment"),
        ("/a/./b.txt", "'..' segment"),
        (long_segment.as_str(), "longer than 255"),
        (long_path.as_str(), "at most 512"),
        ("/_flows/x", "reserved"),
        ("/a\\b.txt", "character"),
        ("/a?b.txt", "character"),
        ("/a#b.txt", "character"),
        ("/f.txt.v2", "revision"),
        ("/dir/", "names a directory"),
    ] {
        let msg = refusal(fx.upload(NS, raw, b"x").await, 400).await;
        assert!(msg.contains(rule), "{raw}: {msg}");
        assert!(msg.contains("namespace file path"), "{raw}: {msg}");
    }
    assert_eq!(fx.rows().await, 0);
}

// ─── Uploads ────────────────────────────────────────────────────────────────

/// A `.zip` is stored as one file: its bytes come back as uploaded, nothing
/// inside it becomes a namespace file (`api.summary.zip_upload`, chosen).
#[tokio::test]
async fn a_zip_upload_is_stored_as_one_file() {
    let Some(fx) = fx().await else { return };
    let zip = zip_of(&[("inner.txt", b"inner"), ("zdir/inner2.txt", b"inner2")]);
    fx.ok_upload(NS, "/top.zip", &zip).await;
    let res = fx.download(NS, "/top.zip").await;
    assert_eq!(res.status(), 200);
    let got = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(got.as_ref(), zip.as_slice());
    refusal(fx.download(NS, "/inner.txt").await, 404).await;
    refusal(fx.download(NS, "/zdir/inner2.txt").await, 404).await;
    assert_eq!(fx.rows().await, 1, "one row: the zip");
}

/// The multipart bounds are W9a's, reused: a file over `max_file_bytes` is a
/// 413 naming the cap; a header that never ends (and a body with no
/// boundary) is a 400 after a bounded read; the part must be `fileContent`.
#[tokio::test]
async fn an_upload_over_the_body_limit_is_413_and_an_unterminated_header_is_400() {
    let Some(fx) = fx().await else { return };
    let over = vec![b'x'; CAP as usize + 1];
    let msg = refusal(fx.upload(NS, "/big.bin", &over).await, 413).await;
    assert!(
        msg.contains(&CAP.to_string()) && msg.contains("max_file_bytes"),
        "{msg}"
    );
    assert_eq!(fx.rows().await, 0);
    fx.ok_upload(NS, "/at.bin", &vec![b'x'; CAP as usize]).await;

    let header_cap = egret_workflow::routes::MAX_MULTIPART_HEADER_BYTES;
    let uri = format!("{}?path=/h.txt", fx.base(NS));
    let mut no_crlfcrlf =
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"fileContent\"; X-Pad: ")
            .into_bytes();
    no_crlfcrlf.resize(900 * 1024, b'a');
    for (what, body) in [
        ("an unterminated header", no_crlfcrlf),
        ("no boundary", vec![b'x'; 900 * 1024]),
    ] {
        let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let p = pulled.clone();
        let chunks: Vec<Vec<u8>> = body.chunks(1024).map(<[u8]>::to_vec).collect();
        let stream = futures_util::StreamExt::map(futures_util::stream::iter(chunks), move |c| {
            p.fetch_add(c.len(), std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from(c))
        });
        let res = common::send_body(
            &fx.app,
            "POST",
            &uri,
            fx.user,
            &ct(),
            axum::body::Body::from_stream(stream),
        )
        .await;
        let msg = refusal(res, 400).await;
        assert!(msg.contains(&header_cap.to_string()), "{what}: {msg}");
        let read = pulled.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            read <= header_cap + 16 * 1024,
            "{what}: {read} bytes pulled"
        );
    }

    let wrong = multipart_file("file", "w.txt", b"w");
    let res = common::send_raw(&fx.app, "POST", &uri, fx.user, &ct(), wrong).await;
    let msg = refusal(res, 400).await;
    assert!(msg.contains("fileContent"), "{msg}");
    let res = common::send_raw(&fx.app, "POST", &uri, fx.user, "text/plain", b"x".to_vec()).await;
    assert_eq!(res.status(), 415);
    assert_eq!(fx.rows().await, 1, "only /at.bin");
}

// ─── Delete and move ────────────────────────────────────────────────────────

/// `DELETE` soft-deletes the path and its children only; a parent left empty
/// stays (`api.summary.delete`, chosen; Kestra walks up).
#[tokio::test]
async fn delete_removes_the_path_and_its_children_and_not_an_emptied_parent() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/del/a/b/only.txt", b"only").await;
    fx.ok_upload(NS, "/del/a/b/c/x.txt", b"x").await;
    fx.ok_upload(NS, "/del2/a/b/only.txt", b"only2").await;

    assert_eq!(fx.rm(NS, "/del/a/b").await.status(), 200);
    assert_eq!(paths(&fx.listing(NS, "/del/a").await), Vec::<String>::new());
    refusal(fx.get(NS, "/directory?path=/del/a/b").await, 404).await;
    refusal(fx.download(NS, "/del/a/b/c/x.txt").await, 404).await;
    assert_eq!(paths(&fx.listing(NS, "/del").await), ["/del/a/"]);

    assert_eq!(fx.rm(NS, "/del2/a/b/only.txt").await.status(), 200);
    assert_eq!(
        paths(&fx.listing(NS, "/del2/a/b").await),
        Vec::<String>::new()
    );
    assert_eq!(paths(&fx.listing(NS, "/").await), ["/del/", "/del2/"]);
}

/// A move onto a live target, or onto a deleted path's history, is a 409
/// naming the target; nothing moves. The history 409 names `PurgeFiles` as
/// the tool, since the API has no purge route.
#[tokio::test]
async fn move_onto_a_live_target_is_409() {
    let Some(fx) = fx().await else { return };
    for b in [b"m1", b"m2", b"m3"] {
        fx.ok_upload(NS, "/mv/three.txt", b).await;
    }
    fx.ok_upload(NS, "/mv/b.txt", b"b").await;
    let msg = refusal(fx.mv(NS, "/mv/three.txt", "/mv/b.txt").await, 409).await;
    assert!(
        msg.contains("/mv/b.txt") && msg.contains("already exists"),
        "{msg}"
    );
    assert_eq!(text(fx.download(NS, "/mv/b.txt").await).await, "b");
    assert_eq!(text(fx.download(NS, "/mv/three.txt").await).await, "m3");

    assert_eq!(fx.rm(NS, "/mv/b.txt").await.status(), 200);
    let msg = refusal(fx.mv(NS, "/mv/three.txt", "/mv/b.txt").await, 409).await;
    assert!(
        msg.contains("/mv/b.txt") && msg.contains("PurgeFiles") && msg.contains("no purge route"),
        "{msg}"
    );
    // A file over a deleted file's history, too.
    let msg = refusal(fx.mkdir_as(fx.user, NS, "/mv/b.txt").await, 409).await;
    assert!(
        msg.contains("PurgeFiles") && msg.contains("no purge route"),
        "{msg}"
    );

    // The move itself (`api.supplements.move_3_revisions`): numbers kept.
    assert_eq!(
        fx.mv(NS, "/mv/three.txt", "/mv/moved.txt").await.status(),
        200
    );
    let revs = common::json(fx.get(NS, "/revisions?path=/mv/moved.txt").await).await;
    let nums: Vec<u64> = revs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["revision"].as_u64().unwrap())
        .collect();
    assert_eq!(nums, [1, 2, 3]);
    for (n, want) in [(1, "m1"), (2, "m2"), (3, "m3")] {
        let res = fx
            .get(NS, &format!("?path=/mv/moved.txt&revision={n}"))
            .await;
        assert_eq!(text(res).await, want);
    }
    refusal(fx.get(NS, "/revisions?path=/mv/three.txt").await, 404).await;
    // A directory moves too (`api.routes.move_dir`).
    fx.ok_upload(NS, "/d1/sub/f.txt", b"f").await;
    assert_eq!(fx.mv(NS, "/d1", "/dirmoved").await.status(), 200);
    assert_eq!(
        paths(&fx.listing(NS, "/dirmoved/sub").await),
        ["/dirmoved/sub/f.txt"]
    );
}

/// w9b `api.routes.move_missing_source` (chosen: 404 naming it) and
/// `delete_missing` (200, as Kestra): neither writes anything.
#[tokio::test]
async fn move_of_a_missing_source_is_404_and_delete_of_a_missing_path_is_200() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/keep.txt", b"k").await;
    common::audit_mutations(&fx.app);
    let before = fx.rows().await;
    let cell = route("move_missing_source");
    let res = fx.mv(NS, "/nope.txt", "/nope2.txt").await;
    let msg = refusal(res, cell["status"].as_u64().unwrap() as u16).await;
    assert!(msg.contains(cell["error_names"].as_str().unwrap()), "{msg}");
    let cell = route("delete_missing");
    let res = fx.rm(NS, "/nope.txt").await;
    assert_eq!(
        res.status().as_u16() as u64,
        cell["status"].as_u64().unwrap()
    );
    assert_eq!(text(res).await, cell["body"]["text"].as_str().unwrap());
    assert_eq!(fx.rows().await, before);
    refusal(fx.download(NS, "/nope2.txt").await, 404).await;
}

// ─── Wrong-kind requests (R3) ───────────────────────────────────────────────

/// w9b `api.routes.get_file_directory` (chosen): 400 naming the directory.
#[tokio::test]
async fn a_download_of_a_directory_is_400_naming_it() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/dir1/g.txt", b"gee").await;
    let cell = route("get_file_directory");
    for raw in ["/dir1", "/dir1/"] {
        let msg = refusal(
            fx.download(NS, raw).await,
            cell["status"].as_u64().unwrap() as u16,
        )
        .await;
        assert!(
            msg.contains(cell["error_names"].as_str().unwrap()),
            "{raw}: {msg}"
        );
        assert!(msg.contains("directory"), "{msg}");
    }
}

/// w9b `api.routes.revisions_dir` (chosen): 400 naming the directory.
#[tokio::test]
async fn revisions_of_a_directory_is_400_naming_it() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/dir1/g.txt", b"gee").await;
    let cell = route("revisions_dir");
    let res = fx.get(NS, "/revisions?path=/dir1").await;
    let msg = refusal(res, cell["status"].as_u64().unwrap() as u16).await;
    assert!(msg.contains(cell["error_names"].as_str().unwrap()), "{msg}");
    assert!(msg.contains("no revisions"), "{msg}");
    refusal(fx.get(NS, "/revisions?path=/nodir").await, 404).await;
}

/// w9b `api.routes.directory_of_a_file` (chosen; Kestra `[]`).
#[tokio::test]
async fn directory_on_a_file_path_is_400_naming_it() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/dir1/g.txt", b"gee").await;
    let cell = route("directory_of_a_file");
    let res = fx.get(NS, "/directory?path=/dir1/g.txt").await;
    let msg = refusal(res, cell["status"].as_u64().unwrap() as u16).await;
    assert!(msg.contains(cell["error_names"].as_str().unwrap()), "{msg}");
    let msg = refusal(fx.get(NS, "/directory?path=/nodir").await, 404).await;
    assert!(msg.contains("/nodir"), "{msg}");
}

// ─── The fixture's cells ────────────────────────────────────────────────────

/// w9b `api.routes.directory_root` and `search q='.txt'` (both chosen):
/// `/top.zip` is one file of the size uploaded, and nothing from inside it.
#[tokio::test]
async fn a_zip_upload_leaves_one_file_in_the_listing_and_the_search() {
    let Some(fx) = fx().await else { return };
    let zip = fixture_namespace(&fx, NS).await;
    let cell = route("directory_root");
    let root = fx.listing(NS, "/").await;
    let mut got: Vec<Value> = root
        .as_array()
        .unwrap()
        .iter()
        .map(|e| json!({"path": e["path"], "type": e["type"], "size": e["size"], "revision": e["revision"]}))
        .collect();
    let mut want: Vec<Value> = cell["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let mut e = e.clone();
            if e["path"] == "/top.zip" {
                e["size"] = json!(zip.len());
            }
            e
        })
        .collect();
    let key = |v: &Value| v["path"].as_str().unwrap().to_string();
    got.sort_by_key(key);
    want.sort_by_key(key);
    assert_eq!(got, want);
    assert!(root
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["created"].is_string()));

    let cell = route("search q='.txt'");
    let found = common::json(fx.get(NS, "/search?q=.txt").await).await;
    assert_eq!(strings(&found), strings(&cell["set"]));
    assert!(!strings(&found).contains(&"/inner.txt".to_string()));
}

/// w9b `api.summary` (stats_body, revisions_body, directory_listing,
/// search_rule) over the fixture's namespace: egret's DTOs, Kestra's search.
#[tokio::test]
async fn stats_revisions_search_and_directory_match_the_fixtures_summary_cells() {
    let Some(fx) = fx().await else { return };
    fixture_namespace(&fx, NS).await;

    let stats = common::json(fx.get(NS, "/stats?path=/dir1/sub/f.txt").await).await;
    assert_eq!(
        (
            &stats["path"],
            &stats["type"],
            &stats["size"],
            &stats["revision"]
        ),
        (
            &json!("/dir1/sub/f.txt"),
            &json!("File"),
            &json!(8),
            &json!(2)
        )
    );
    assert!(stats["created"].is_string());
    let stats = common::json(fx.get(NS, "/stats?path=/dir1").await).await;
    assert_eq!(
        (
            &stats["path"],
            &stats["type"],
            &stats["size"],
            &stats["revision"]
        ),
        (&json!("/dir1/"), &json!("Directory"), &json!(0), &json!(1))
    );
    let root = common::json(fx.get(NS, "/stats").await).await;
    assert_eq!(root, route("stats_root_no_path")["body"]);
    refusal(fx.get(NS, "/stats?path=/nope.txt").await, 404).await;

    let revs = common::json(fx.get(NS, "/revisions?path=/dir1/sub/f.txt").await).await;
    let got: Vec<Value> = revs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!([r["revision"], r["deleted"], r["size"]]))
        .collect();
    assert_eq!(got, [json!([1, false, 5]), json!([2, false, 8])]);
    // `revisions` without a path is a 400 (the query is required).
    assert_eq!(fx.get(NS, "/revisions").await.status(), 400);

    // Every measured search cell (the chosen one is the zip test's).
    let routes = fixture()["egret_expected"]["api"]["routes"].clone();
    let mut checked = 0;
    for (name, cell) in routes.as_object().unwrap() {
        if !name.starts_with("search q=") || cell["basis"] == "chosen" {
            continue;
        }
        let q = cell["request"]
            .as_str()
            .unwrap()
            .split("?q=")
            .nth(1)
            .unwrap();
        let res = fx.get(NS, &format!("/search?q={}", enc(q))).await;
        assert_eq!(res.status(), 200, "{name}");
        let found = common::json(res).await;
        assert_eq!(strings(&found), strings(&cell["body"]["json"]), "{name}");
        checked += 1;
    }
    assert_eq!(checked, 11, "the measured search cells");
    assert_eq!(fx.get(NS, "/search").await.status(), 400, "no q");

    // `directory_dir1` / `_trailing` / `_sub`: name, kind and size of each child.
    for (name, dir) in [
        ("directory_dir1", "/dir1"),
        ("directory_dir1_trailing", "/dir1/"),
        ("directory_dir1_sub", "/dir1/sub"),
    ] {
        let mut want: Vec<Value> = route(name)["body"]["json"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!([e["fileName"], e["type"], e["size"]]))
            .collect();
        let mut got: Vec<Value> = fx
            .listing(NS, dir)
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let p = e["path"].as_str().unwrap().trim_end_matches('/');
                json!([p.rsplit('/').next().unwrap(), e["type"], e["size"]])
            })
            .collect();
        want.sort_by_key(Value::to_string);
        got.sort_by_key(Value::to_string);
        assert_eq!(got, want, "{name}");
    }
    let msg = refusal(fx.get(NS, "/directory?path=/nodir").await, 404).await;
    assert!(msg.contains("/nodir"), "{msg}");
}

/// R1: w9b `api.routes.revisions_after_delete` (200, [1] deleted) and
/// `get_after_delete_rev1` (200 "gee"); `get_after_delete` stays 404.
#[tokio::test]
async fn a_soft_deleted_paths_revisions_are_listed_flagged_and_a_given_revision_is_served() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/dir1/g.txt", b"gee").await;
    assert_eq!(fx.mv(NS, "/dir1/g.txt", "/moved/g.txt").await.status(), 200);
    assert_eq!(fx.rm(NS, "moved/g.txt").await.status(), 200);

    let cell = route("revisions_after_delete");
    let res = fx.get(NS, "/revisions?path=/moved/g.txt").await;
    assert_eq!(
        res.status().as_u16() as u64,
        cell["status"].as_u64().unwrap()
    );
    let revs = common::json(res).await;
    let nums: Vec<Value> = revs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["revision"].clone())
        .collect();
    let deleted: Vec<Value> = revs
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["deleted"] == true)
        .map(|r| r["revision"].clone())
        .collect();
    assert_eq!(Value::from(nums), cell["revisions"]);
    assert_eq!(Value::from(deleted), cell["deleted"]);

    let cell = route("get_after_delete_rev1");
    let res = fx.get(NS, "?path=/moved/g.txt&revision=1").await;
    assert_eq!(
        res.status().as_u16() as u64,
        cell["status"].as_u64().unwrap()
    );
    assert_eq!(text(res).await, cell["body"]["text"].as_str().unwrap());
    let cell = route("get_after_delete");
    assert_eq!(cell["status"], 404);
    refusal(fx.download(NS, "/moved/g.txt").await, 404).await;
    refusal(fx.get(NS, "/stats?path=/moved/g.txt").await, 404).await;
}

/// R3: no GET creates anything. A namespace with no rows answers the virtual
/// root (`stats_root_no_path`, `stats_fresh_namespace_root`) and an empty
/// root listing; the row count is the same after every GET route.
#[tokio::test]
async fn no_get_route_creates_anything() {
    let Some(fx) = fx().await else { return };
    let fresh = "w9b.fresh";
    let gets = [
        "/stats",
        "/stats?path=/",
        "/directory",
        "/search?q=x",
        "?path=/x.txt",
        "/revisions?path=/x.txt",
        "/stats?path=/x.txt",
        "/directory?path=/d",
    ];
    for rest in gets {
        let res = fx.get(fresh, rest).await;
        assert!(res.status().as_u16() < 500, "{rest}: {}", res.status());
        assert_eq!(fx.rows().await, 0, "after GET {rest}");
    }
    let root = common::json(fx.get(fresh, "/stats").await).await;
    assert_eq!(root, route("stats_fresh_namespace_root")["body"]);
    assert_eq!(
        common::json(fx.get(fresh, "/directory").await).await,
        json!([])
    );

    fixture_namespace(&fx, NS).await;
    let before = fx.rows().await;
    for rest in gets
        .iter()
        .chain(&["?path=/dir1/g.txt", "/directory?path=/dir1"])
    {
        fx.get(NS, rest).await;
        assert_eq!(fx.rows().await, before, "after GET {rest}");
    }
}

/// R3: as Kestra's search (w9b `api.routes` search q='F.TXT', 'CASE',
/// 'dir1'): a case-insensitive substring, files only.
#[tokio::test]
async fn search_is_case_insensitive_and_lists_files_only() {
    let Some(fx) = fx().await else { return };
    fixture_namespace(&fx, NS).await;
    for name in [
        "search q='F.TXT'",
        "search q='CASE'",
        "search q='case'",
        "search q='dir1'",
    ] {
        let cell = route(name);
        let q = cell["request"]
            .as_str()
            .unwrap()
            .split("?q=")
            .nth(1)
            .unwrap();
        let found = common::json(fx.get(NS, &format!("/search?q={}", enc(q))).await).await;
        assert_eq!(strings(&found), strings(&cell["body"]["json"]), "{name}");
    }
    // `dir1` names a directory too; only files are listed.
    let found = common::json(fx.get(NS, "/search?q=dir1").await).await;
    assert!(strings(&found).iter().all(|p| !p.ends_with('/')), "{found}");
    assert_eq!(
        common::json(fx.get(NS, "/search?q=").await).await,
        json!([])
    );
}

/// The listing and the search fetch at most `max_namespace_files + 1` rows;
/// the extra one is a 413 naming the cap. At the cap, both answer.
#[tokio::test]
async fn search_and_directory_past_the_cap_are_refused_naming_it() {
    let Some(fx) = fx().await else { return };
    for i in 0..MAX_FILES {
        fx.ok_upload(NS, &format!("/many/f{i}.txt"), b"x").await;
    }
    assert_eq!(
        fx.listing(NS, "/many").await.as_array().unwrap().len(),
        MAX_FILES as usize
    );
    let found = common::json(fx.get(NS, "/search?q=many").await).await;
    assert_eq!(found.as_array().unwrap().len(), MAX_FILES as usize);
    fx.ok_upload(NS, "/many/one-more.txt", b"x").await;
    for rest in ["/directory?path=/many", "/search?q=many"] {
        let msg = refusal(fx.get(NS, rest).await, 413).await;
        assert!(
            msg.contains("max_namespace_files") && msg.contains(&MAX_FILES.to_string()),
            "{rest}: {msg}"
        );
    }
}

/// An unreachable store is a 503 that names neither the bucket nor a key;
/// the upload writes no row.
#[tokio::test]
async fn a_store_outage_is_503() {
    let Some(fx) = fx().await else { return };
    let msg = refusal(fx.upload(OUTAGE_NS, "/x.txt", b"x").await, 503).await;
    assert!(msg.contains("/x.txt"), "{msg}");
    assert!(
        !msg.contains("s3://") && !msg.contains("egret-test") && !msg.contains("workflow/"),
        "{msg}"
    );
    assert_eq!(fx.rows().await, 0);
    // A row whose object cannot be read.
    sqlx::query(
        "INSERT INTO workflow.namespace_files \
           (workspace_id, namespace, path, revision, is_directory, size, deleted) \
         VALUES ($1, $2, '/y.txt', 1, false, 1, false)",
    )
    .bind(fx.ws)
    .bind(OUTAGE_NS)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let msg = refusal(fx.download(OUTAGE_NS, "/y.txt").await, 503).await;
    assert!(msg.contains("/y.txt") && !msg.contains("s3://"), "{msg}");
}

/// Each write route emits exactly one audit mutation, with the namespace and
/// the path (and `to` for a move); a refused write emits none.
#[tokio::test]
async fn every_write_route_emits_one_audit_mutation() {
    let Some(fx) = fx().await else { return };
    common::audit_mutations(&fx.app);
    let one = |action: &str, path: &str, detail: Value| {
        let got = common::audit_mutations(&fx.app);
        assert_eq!(got.len(), 1, "{action}: {got:?}");
        let (a, id, d) = &got[0];
        assert_eq!(a, &format!("workflow_namespace_file:{action}"));
        assert_eq!(id.as_deref(), Some(format!("{NS}:{path}").as_str()));
        for (k, v) in detail.as_object().unwrap() {
            assert_eq!(&d[k], v, "{action}: {d}");
        }
    };
    assert_eq!(fx.upload(NS, "/a.txt", b"a").await.status(), 200);
    one(
        "upload",
        "/a.txt",
        json!({ "namespace": NS, "path": "/a.txt" }),
    );
    assert_eq!(fx.mkdir_as(fx.user, NS, "/d").await.status(), 200);
    one(
        "create_directory",
        "/d/",
        json!({ "namespace": NS, "path": "/d/" }),
    );
    assert_eq!(fx.mv(NS, "/a.txt", "/d/b.txt").await.status(), 200);
    one(
        "move",
        "/a.txt",
        json!({ "namespace": NS, "path": "/a.txt", "to": "/d/b.txt" }),
    );
    assert_eq!(fx.rm(NS, "/d").await.status(), 200);
    one("delete", "/d/", json!({ "namespace": NS, "path": "/d/" }));
    refusal(fx.mv(NS, "/nope", "/x").await, 404).await;
    refusal(fx.upload(NS, "/_flows/x", b"x").await, 400).await;
    assert!(common::audit_mutations(&fx.app).is_empty());
}

/// No flow exists in `w9b.noflow`: its files are stored and served all the
/// same (spec §4.1: no namespace table, no FK to a flow).
#[tokio::test]
async fn a_namespace_with_no_flow_can_hold_files() {
    let Some(fx) = fx().await else { return };
    let flows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.flows WHERE workspace_id = $1")
            .bind(fx.ws)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(flows, 0);
    fx.ok_upload("w9b.noflow", "/cfg/app.yml", b"k: v").await;
    assert_eq!(
        text(fx.download("w9b.noflow", "/cfg/app.yml").await).await,
        "k: v"
    );
    assert_eq!(paths(&fx.listing("w9b.noflow", "/").await), ["/cfg/"]);
}

// ─── Fix round 1 (2026-10-06) ───────────────────────────────────────────────

/// A multipart upload body that hands over its headers and three bytes of
/// the file, counts itself in `stalled`, and then sends nothing more, ever.
fn stalling_body(stalled: Arc<std::sync::atomic::AtomicUsize>) -> axum::body::Body {
    use futures_util::StreamExt as _;
    let head = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"fileContent\"; \
         filename=\"s.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nabc"
    );
    let first = futures_util::stream::once(async move {
        Ok::<_, std::io::Error>(axum::body::Bytes::from(head))
    });
    let stall = futures_util::stream::once(async move {
        stalled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
    });
    axum::body::Body::from_stream(first.chain(stall))
}

/// This workspace's staging rows.
async fn staging_rows(fx: &Fx) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.namespace_file_staging WHERE workspace_id = $1",
    )
    .bind(fx.ws)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap()
}

/// I-1 (the user's ruling of 2026-10-06: the body is uploaded before any lock
/// is taken). Five uploads whose bodies stall, on this binary's 5-connection
/// pool, hold no connection: the pool keeps an idle one, and an unrelated GET
/// (its session alone needs the database) answers at once. Before staging,
/// each held a connection and its path's lock until its idle timeout (30 s).
#[tokio::test(flavor = "multi_thread")]
async fn stalled_uploads_hold_no_pool_connection_and_block_no_other_request() {
    let Some(fx) = fx().await else { return };
    assert_eq!(fx.db.pool.options().get_max_connections(), 5);
    fx.ok_upload(NS, "/stall/seen.txt", b"seen").await;
    let before = fx.rows().await;
    let stalled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let uploads = futures_util::future::join_all((0..5).map(|i| {
        let uri = format!("{}?path=/stall/{i}.txt", fx.base(NS));
        let body = stalling_body(stalled.clone());
        let app = &fx.app;
        async move { common::send_body(app, "POST", &uri, fx.user, &ct(), body).await }
    }));
    let probe = async {
        // Guards against a hang, not measurements.
        let all_stalled = async {
            while stalled.load(std::sync::atomic::Ordering::SeqCst) < 5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), all_stalled)
            .await
            .expect("five uploads reached their stalled bodies");
        let idle = || fx.db.pool.num_idle();
        assert!(idle() > 0, "the stalled uploads hold every pool connection");
        let get = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fx.get(NS, "/stats?path=/stall/seen.txt"),
        )
        .await
        .expect("an unrelated GET waited on the stalled uploads");
        assert_eq!(get.status(), 200, "{}", text(get).await);
        assert!(idle() > 0, "the pool ran dry while the uploads stalled");
    };
    tokio::select! {
        _ = uploads => panic!("a stalled upload ended"),
        () = probe => {}
    }
    // The uploads were dropped mid-body: nothing was written.
    assert_eq!(fx.rows().await, before);
}

/// I-2: a body that stops arriving is refused 408 once the (injected, 300 ms)
/// idle timeout passes; nothing is written, its staging object is gone, the
/// pool has its connections back, and the same path is written at once
/// afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_upload_is_408_writes_nothing_and_leaves_the_path_writable() {
    let Some(fx) = fx().await else { return };
    let app = common::app_with_upload_idle(&fx.db, std::time::Duration::from_millis(300)).await;
    let before = fx.rows().await;
    let uri = format!("{}?path=/idle/x.txt", fx.base(NS));
    let stalled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        common::send_body(&app, "POST", &uri, fx.user, &ct(), stalling_body(stalled)),
    )
    .await
    .expect("the stalled upload was never refused");
    let msg = refusal(res, 408).await;
    assert!(
        msg.contains("no byte of the request body arrived for 300 ms")
            && msg.contains("nothing was stored"),
        "{msg}"
    );
    assert_eq!(fx.rows().await, before, "a row was written");
    assert_eq!(staging_rows(&fx).await, 0, "the staging row is left");
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        common::send_raw(
            &app,
            "POST",
            &uri,
            fx.user,
            &ct(),
            multipart_file("fileContent", "x", b"after"),
        ),
    )
    .await
    .expect("the same path waited after the 408");
    assert_eq!(res.status(), 200, "{}", text(res).await);
    assert_eq!(text(fx.download(NS, "/idle/x.txt").await).await, "after");
    let recovered = async {
        while fx.db.pool.num_idle() != fx.db.pool.size() as usize {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), recovered)
        .await
        .expect("the pool did not get its connections back");
}

/// M-2: a soft-deleted directory is nothing there on every read route (404),
/// where a live one is the wrong kind (400) on the download and `revisions`.
#[tokio::test]
async fn a_soft_deleted_directory_is_404_on_every_read_route() {
    let Some(fx) = fx().await else { return };
    fx.ok_upload(NS, "/gone/a.txt", b"a").await;
    assert_eq!(
        fx.download(NS, "/gone").await.status(),
        400,
        "live: the control"
    );
    assert_eq!(fx.rm(NS, "/gone").await.status(), 200);
    for rest in [
        "?path=/gone",
        "/revisions?path=/gone",
        "/stats?path=/gone/",
        "/directory?path=/gone/",
    ] {
        let res = fx.get(NS, rest).await;
        assert_eq!(res.status(), 404, "{rest}: {}", text(res).await);
    }
    // Spelled as a directory, the file routes answer what they answer for a
    // directory that never existed: the path rule's 400, not "is a directory".
    for route in ["?path=", "/revisions?path="] {
        let gone = fx.get(NS, &format!("{route}/gone/")).await;
        let never = fx.get(NS, &format!("{route}/never/")).await;
        assert_eq!(gone.status(), never.status(), "{route}");
        let (gone, never) = (text(gone).await, text(never).await);
        assert_eq!(gone.replace("gone", "never"), never, "{route}");
        assert!(!gone.contains("is a directory"), "{route}: {gone}");
    }
}

/// M-3: a DELETE of nothing audits the path as a real delete would name it.
#[tokio::test]
async fn a_no_op_delete_audits_the_normalised_path() {
    let Some(fx) = fx().await else { return };
    common::audit_mutations(&fx.app);
    for (raw, normal) in [("nope.txt", "/nope.txt"), ("/d/", "/d/"), ("d/e", "/d/e")] {
        assert_eq!(fx.rm(NS, raw).await.status(), 200, "{raw}");
        let got = common::audit_mutations(&fx.app);
        assert_eq!(got.len(), 1, "{raw}: {got:?}");
        let (action, id, detail) = &got[0];
        assert_eq!(action, "workflow_namespace_file:delete");
        assert_eq!(
            id.as_deref(),
            Some(format!("{NS}:{normal}").as_str()),
            "{raw}"
        );
        assert_eq!(detail["path"], normal, "{raw}");
        assert_eq!(detail["deleted"], 0, "{raw}");
    }
}

/// M-5: a missing or malformed query parameter is the documented `ErrorBody`
/// 400 (JSON naming it), not axum's `text/plain`; a write's 200 has no
/// content and no content type, as openapi now says.
#[tokio::test]
async fn query_refusals_are_error_bodies_and_write_answers_have_no_content() {
    let Some(fx) = fx().await else { return };
    for rest in [
        "/search",
        "?path=/x&revision=abc",
        "/revisions",
        "?revision=1",
    ] {
        let res = fx.get(NS, rest).await;
        assert_eq!(res.status(), 400, "{rest}");
        assert_eq!(res.headers()["content-type"], "application/json", "{rest}");
        let body: Value = serde_json::from_str(&text(res).await).unwrap();
        assert!(
            body["error"].as_str().is_some_and(|m| !m.is_empty()),
            "{rest}: {body}"
        );
    }
    let writes = [
        fx.upload(NS, "/w/a.txt", b"a").await,
        fx.mkdir_as(fx.user, NS, "/w/d").await,
        fx.mv(NS, "/w/a.txt", "/w/b.txt").await,
        fx.rm(NS, "/w/b.txt").await,
    ];
    for res in writes {
        assert_eq!(res.status(), 200);
        assert!(
            res.headers().get("content-type").is_none(),
            "{:?}",
            res.headers()
        );
        assert_eq!(text(res).await, "");
    }
}
