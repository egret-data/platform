//! W9b Task 3: namespace file writes against MinIO (a real `S3Store`): a file
//! larger than one multipart part, and a move and a purge of names holding
//! spaces and non-ASCII characters (the keys S3 must take as they are).
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`) and
//! `DATABASE_URL`. Without the S3 variables every case is reported
//! **ignored**, never passed (`harness = false`, as `storage_prune_s3_live`).
//!
//! # It deletes, so it is fenced
//!
//! The `FileStore`'s prefix is a throwaway `w9b-t3-test/<uuid>/`, and the
//! store is wrapped in a [`Fence`] that REFUSES (and records) every `delete`
//! outside it and every `delete_prefix` but of exactly it. Each case fails if
//! anything was refused, and ends with `delete_prefix` of that prefix.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_storage::{Listing, ObjectStore, S3Store, StorageConfig, StorageError};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{
    move_path, open, purge_revisions, put, resolve, revisions, Conflict, Want, Written,
};
use egret_workflow::storage::nsfiles::CreatedBy;
use egret_workflow::storage::{Caps, FileStore};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(300);
const NS: &str = "w9b.t3s3";

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "minioadmin".into()),
        path_style: true,
    })
}

/// The S3 store, refusing any delete outside `allowed`
/// (`s3://bucket/w9b-t3-test/<uuid>/`) and any `delete_prefix` but of it.
struct Fence {
    s3: S3Store,
    allowed: String,
    refused: Mutex<Vec<String>>,
}

impl Fence {
    fn refuse(&self, uri: &str) -> StorageError {
        self.refused.lock().unwrap().push(uri.to_string());
        StorageError::S3(format!("{uri}: outside the test's prefix, refused"))
    }
}

#[async_trait::async_trait]
impl ObjectStore for Fence {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.s3.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.s3.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        if !(uri.starts_with(&self.allowed) && uri.len() > self.allowed.len()) {
            return Err(self.refuse(uri));
        }
        self.s3.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.s3.copy(src, dst).await
    }
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError> {
        self.s3.list(prefix, delimiter).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.s3.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, StorageError> {
        self.s3.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.s3.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        if uri != self.allowed {
            return Err(self.refuse(uri));
        }
        self.s3.delete_prefix(uri).await
    }
}

/// Runs `case` with a fenced `FileStore` under a fresh throwaway prefix and
/// a fresh workspace, then deletes exactly that prefix.
fn fenced<F, Fut>(cfg: StorageConfig, case: F) -> Result<(), String>
where
    F: FnOnce(egret_metastore::Db, Arc<FileStore>, Grant) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let Some(db) = common::db().await else {
            return Err("DATABASE_URL is required".into());
        };
        let throwaway = format!("w9b-t3-test/{}/", Uuid::new_v4());
        let bucket = cfg.bucket.clone();
        let fence = Arc::new(Fence {
            s3: S3Store::new(&cfg).await.map_err(|e| e.to_string())?,
            allowed: format!("s3://{bucket}/{throwaway}"),
            refused: Mutex::new(Vec::new()),
        });
        let fs = Arc::new(FileStore {
            store: fence.clone(),
            bucket,
            prefix: throwaway.clone(),
            caps: Caps::default(),
            retention_days: 30,
        });
        let (ws, _) = common::seed_workspace(&db).await;
        let g = authorize(ws, Actor::Api, NS, Access::Write).map_err(|e| e.to_string())?;
        let result = case(db, fs, g).await;
        let removed = fence
            .delete_prefix(&fence.allowed)
            .await
            .map_err(|e| e.to_string())?;
        println!("    delete_prefix {}: {removed} object(s)", fence.allowed);
        let refused = fence.refused.lock().unwrap().clone();
        result?;
        if !refused.is_empty() {
            return Err(format!("deletes outside {throwaway}: {refused:?}"));
        }
        Ok(())
    })
}

fn file(p: &str) -> NsPath {
    NsPath::parse_file(p).unwrap()
}

async fn write(
    db: &egret_metastore::Db,
    fs: &FileStore,
    g: &Grant,
    p: &str,
    bytes: &[u8],
) -> Result<u32, String> {
    let mut body = bytes;
    match put(
        &db.pool,
        fs,
        g,
        &file(p),
        &mut body,
        Conflict::Overwrite,
        CreatedBy::User(Uuid::nil()),
    )
    .await
    {
        Ok(Written::Written(e)) => Ok(e.revision),
        other => Err(format!("{p}: {other:?}")),
    }
}

async fn read(
    db: &egret_metastore::Db,
    fs: &FileStore,
    g: &Grant,
    p: &str,
    want: Want,
) -> Result<Vec<u8>, String> {
    let mut c = db.pool.acquire().await.map_err(|e| e.to_string())?;
    let e = resolve(&mut c, g, &file(p), want)
        .await
        .map_err(|e| format!("{p}: {e}"))?;
    let mut out = Vec::new();
    open(fs, g, &e)
        .await
        .map_err(|e| format!("{p}: {e}"))?
        .read_to_end(&mut out)
        .await
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn large_part() -> Result<(), String> {
    let Some(cfg) = cfg() else { unreachable!() };
    fenced(cfg, |db, fs, g| async move {
        let n = 20 * 1024 * 1024 + 3;
        assert!(n > egret_storage::PART_BYTES * 2);
        let bytes: Vec<u8> = (0..n).map(|i| ((i * 31 + 7) % 251) as u8).collect();
        let rev = write(&db, &fs, &g, "/big/blob.bin", &bytes).await?;
        if rev != 1 {
            return Err(format!("revision {rev}"));
        }
        let key = fs.namespace_object_uri(&g, &file("/big/blob.bin"), 1);
        let head = fs.store.head(&key).await.map_err(|e| e.to_string())?;
        if head != Some(n as u64) {
            return Err(format!("head {head:?}, want {n}"));
        }
        let mut c = db.pool.acquire().await.map_err(|e| e.to_string())?;
        let row = resolve(&mut c, &g, &file("/big/blob.bin"), Want::Latest)
            .await
            .map_err(|e| e.to_string())?;
        if row.size != n as u64 {
            return Err(format!("row size {}", row.size));
        }
        let back = read(&db, &fs, &g, "/big/blob.bin", Want::Latest).await?;
        if back != bytes {
            return Err(format!(
                "{} bytes read back, not the {n} written",
                back.len()
            ));
        }
        // A second revision lands at `.v2` on S3 too.
        let rev = write(&db, &fs, &g, "/big/blob.bin", b"small").await?;
        let v2 = fs.namespace_object_uri(&g, &file("/big/blob.bin"), 2);
        if rev != 2 || !v2.ends_with("/_files/big/blob.bin.v2") {
            return Err(format!("revision {rev} at {v2}"));
        }
        if read(&db, &fs, &g, "/big/blob.bin", Want::Latest).await? != b"small" {
            return Err("revision 2 reads back wrong".into());
        }
        println!("    {n} bytes round-tripped through a multipart upload");
        Ok(())
    })
}

fn move_and_purge() -> Result<(), String> {
    let Some(cfg) = cfg() else { unreachable!() };
    fenced(cfg, |db, fs, g| async move {
        let from = "/dir é/a b+c ü.txt";
        write(&db, &fs, &g, from, b"first").await?;
        write(&db, &fs, &g, from, b"second").await?;
        let src_dir = NsPath::parse_dir("/dir é").unwrap();
        let dst_dir = NsPath::parse_dir("/ünï cödé #2x").ok();
        // `#` is refused by the path rule: the control that the rule holds.
        if dst_dir.is_some() {
            return Err("'#' was accepted".into());
        }
        let dst_dir = NsPath::parse_dir("/ünï cödé %20 x").unwrap();
        let moved = move_path(
            &db.pool,
            &fs,
            &g,
            &src_dir,
            &dst_dir,
            CreatedBy::User(Uuid::nil()),
        )
        .await
        .map_err(|e| e.to_string())?;
        if moved.len() != 3 {
            return Err(format!("{} rows moved: {moved:?}", moved.len()));
        }
        let to = "/ünï cödé %20 x/a b+c ü.txt";
        for (n, body) in [(1, &b"first"[..]), (2, &b"second"[..])] {
            if read(&db, &fs, &g, to, Want::Revision(n)).await? != body {
                return Err(format!("revision {n} of {to} reads back wrong"));
            }
            let old = fs.namespace_object_uri(&g, &file(from), n);
            if fs
                .store
                .head(&old)
                .await
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err(format!("the source's revision {n} object is still there"));
            }
        }
        let purge = authorize(g.workspace(), Actor::Api, NS, Access::Purge).unwrap();
        let purged = purge_revisions(&db.pool, &fs, &purge, &file(to), &[1])
            .await
            .map_err(|e| e.to_string())?;
        let v1 = fs.namespace_object_uri(&g, &file(to), 1);
        let v2 = fs.namespace_object_uri(&g, &file(to), 2);
        let h1 = fs.store.head(&v1).await.map_err(|e| e.to_string())?;
        let h2 = fs.store.head(&v2).await.map_err(|e| e.to_string())?;
        let mut c = db.pool.acquire().await.map_err(|e| e.to_string())?;
        let left: Vec<u32> = revisions(&mut c, &g, &file(to))
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .map(|e| e.revision)
            .collect();
        if (purged, h1, h2, left.as_slice()) != (1, None, Some(6), &[2][..]) {
            return Err(format!(
                "purge: {purged} purged, v1 {h1:?}, v2 {h2:?}, rows {left:?}"
            ));
        }
        println!("    moved and purged {to:?} on S3");
        Ok(())
    })
}

type Case = fn() -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![
        (
            "a_namespace_file_larger_than_one_part_round_trips",
            large_part,
        ),
        (
            "move_and_purge_on_minio_with_a_name_holding_spaces_and_unicode",
            move_and_purge,
        ),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let all = cases();
    let selected: Vec<_> = all
        .iter()
        .filter(|(n, _)| filter.as_ref().is_none_or(|f| n.contains(f.as_str())))
        .collect();
    let filtered_out = all.len() - selected.len();
    let started = std::time::Instant::now();
    println!("\nrunning {} tests", selected.len());
    let (mut passed, mut ignored, mut failed) = (0, 0, Vec::new());
    for (name, case) in &selected {
        if cfg().is_none() {
            println!("test {name} ... ignored, EGRET_TEST_S3_ENDPOINT unset");
            ignored += 1;
            continue;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let case = *case;
        std::thread::spawn(move || {
            let _ = tx.send(case());
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(Ok(())) => {
                println!("test {name} ... ok");
                passed += 1;
            }
            Ok(Err(why)) => {
                println!("test {name} ... FAILED\n    {why}");
                failed.push(name.to_string());
            }
            Err(_) => {
                println!("test {name} ... FAILED\n    no answer in {DEADLINE:?}");
                failed.push(name.to_string());
            }
        }
    }
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {passed} passed; {} failed; {ignored} ignored; 0 measured; \
         {filtered_out} filtered out; finished in {:.2}s\n",
        failed.len(),
        started.elapsed().as_secs_f64()
    );
    std::process::exit(if failed.is_empty() { 0 } else { 101 });
}
