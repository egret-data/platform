//! W9c Task 3: the KV writes, purge and expiry sweep against MinIO (a real
//! `S3Store`): a value of exactly `max_read_bytes` (10 MiB) written through
//! staging and read back, one byte over refused, `Put`'s merge, a delete, a
//! purge and the sweep deleting real objects.
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`) and
//! `DATABASE_URL`. Without the S3 variables every case is reported
//! **ignored**, never passed (`harness = false`, as `nsfiles_s3_live`).
//!
//! # It deletes, so it is fenced
//!
//! The `FileStore`'s prefix is a throwaway `w9c-t3-test/<uuid>/`, and the
//! store is wrapped in a [`Fence`] that REFUSES (and records) every `delete`
//! outside it and every `delete_prefix` but of exactly it. Each case fails if
//! anything was refused, and ends with `delete_prefix` of that prefix. The
//! sweep is global over this binary's own test database (one per process),
//! which holds only this case's rows.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_storage::{Listing, ObjectStore, S3Store, StorageConfig, StorageError};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::store::{
    delete, purge_key, put_merge, read_value, resolve, set, Deleted, KvWritten, NewValue,
};
use egret_workflow::storage::kv::sweep::{sweep_expired, KvSweepConfig};
use egret_workflow::storage::kv::value::{KvData, KvType, KvValue};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::store::Want;
use egret_workflow::storage::nsfiles::{CreatedBy, NsError};
use egret_workflow::storage::{Caps, FileStore};
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncRead};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(300);
const NS: &str = "w9c.t3s3";

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
/// (`s3://bucket/w9c-t3-test/<uuid>/`) and any `delete_prefix` but of it.
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
/// a fresh workspace, then deletes exactly that prefix -- also when the case
/// panics.
fn fenced<F, Fut>(cfg: StorageConfig, case: F) -> Result<(), String>
where
    F: FnOnce(egret_metastore::Db, Arc<FileStore>, Grant) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
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
        let throwaway = format!("w9c-t3-test/{}/", Uuid::new_v4());
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
        let g = authorize(ws, Actor::Api, NS, Resource::Kv, Access::Write)
            .map_err(|e| e.to_string())?;
        // On its own task: a panic in the case is a JoinError here, and the
        // cleanup below still runs (fix round 1, M8).
        let result = match tokio::spawn(case(db, fs, g)).await {
            Ok(r) => r,
            Err(e) => Err(format!("the case panicked: {e}")),
        };
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

fn key(k: &str) -> KvKey {
    KvKey::parse(k).unwrap()
}

const USER: CreatedBy = CreatedBy::User(Uuid::nil());

fn value(v: serde_json::Value) -> NewValue {
    NewValue {
        value: KvValue::from_json(KvType::of(&v), &v).unwrap(),
        description: None,
        ttl: None,
        overwrite: true,
    }
}

async fn latest(
    db: &egret_metastore::Db,
    fs: &FileStore,
    g: &Grant,
    k: &str,
) -> Result<Option<KvData>, String> {
    let mut c = db.pool.acquire().await.map_err(|e| e.to_string())?;
    let e = match resolve(&mut c, g, &key(k), Want::Latest).await {
        Ok(e) => e,
        Err(NsError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    drop(c);
    read_value(&db.pool, fs, g, &e)
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

async fn head(fs: &FileStore, g: &Grant, k: &str, rev: u32) -> Result<Option<u64>, String> {
    fs.store
        .head(&fs.namespace_kv_uri(g, &key(k), rev))
        .await
        .map_err(|e| e.to_string())
}

fn everything() -> Result<(), String> {
    let Some(cfg) = cfg() else { unreachable!() };
    fenced(cfg, |db, fs, g| async move {
        let cap = fs.caps.max_read_bytes;
        // A STRING at exactly the cap: `"` + cap - 2 bytes + `"`.
        let text = "k".repeat(cap as usize - 2);
        match set(&db.pool, &fs, &g, &key("big"), value(json!(text)), USER).await {
            Ok(KvWritten::Written(e)) if e.size == cap => {}
            other => return Err(format!("the value at the cap: {other:?}")),
        }
        if head(&fs, &g, "big", 1).await? != Some(cap) {
            return Err("the object is not the cap's size".into());
        }
        match &latest(&db, &fs, &g, "big").await? {
            Some(KvData::String(s)) if *s == text => {}
            other => return Err(format!("read back {:?}", other.as_ref().map(|d| d.shape()))),
        }
        match set(
            &db.pool,
            &fs,
            &g,
            &key("big"),
            value(json!(format!("{text}k"))),
            USER,
        )
        .await
        {
            Err(NsError::TooLarge(m)) if m.contains("workflow.storage.max_read_bytes") => {}
            other => return Err(format!("one byte over: {other:?}")),
        }
        // Put merges on S3.
        set(&db.pool, &fs, &g, &key("m"), value(json!({"a": 1})), USER)
            .await
            .map_err(|e| e.to_string())?;
        let patch = KvData::from_json(&json!({"b": {"c": 2}})).unwrap();
        put_merge(&db.pool, &fs, &g, &key("m"), patch, false, USER)
            .await
            .map_err(|e| e.to_string())?;
        match latest(&db, &fs, &g, "m").await? {
            Some(d) if d.to_json() == json!({"a": 1, "b": {"c": 2}}) => {}
            other => return Err(format!("merged {other:?}")),
        }
        if head(&fs, &g, "m", 2).await?.is_none() {
            return Err("no m.ion.v2".into());
        }
        // Delete: flagged, no object removed.
        if delete(&db.pool, &g, &key("m"), USER)
            .await
            .map_err(|e| e.to_string())?
            != Deleted::Deleted
        {
            return Err("delete".into());
        }
        if latest(&db, &fs, &g, "m").await?.is_some() || head(&fs, &g, "m", 2).await?.is_none() {
            return Err("after delete".into());
        }
        // Purge every revision of `m`: rows and objects.
        let purge = authorize(g.workspace(), Actor::Api, NS, Resource::Kv, Access::Purge).unwrap();
        let n = purge_key(&db.pool, &fs, &purge, &key("m"), 100, |rows| {
            rows.iter().map(|r| r.entry.id).collect()
        })
        .await
        .map_err(|e| e.to_string())?;
        if (
            n,
            head(&fs, &g, "m", 1).await?,
            head(&fs, &g, "m", 2).await?,
        ) != (2, None, None)
        {
            return Err(format!("purge: {n}"));
        }
        // The sweep: an expired key's rows and objects.
        let mut nv = value(json!("bye"));
        nv.ttl = Some(chrono::TimeDelta::seconds(60));
        set(&db.pool, &fs, &g, &key("ttl"), nv, USER)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("UPDATE workflow.kv_entries SET expires_at = now() - interval '1 second' WHERE workspace_id = $1 AND key = 'ttl'")
            .bind(g.workspace())
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;
        let swept = sweep_expired(&db.pool, &fs, &KvSweepConfig::default())
            .await
            .map_err(|e| e.to_string())?;
        if swept != 1 || head(&fs, &g, "ttl", 1).await?.is_some() {
            return Err(format!("sweep: {swept}"));
        }
        if head(&fs, &g, "big", 1).await? != Some(cap) {
            return Err("the sweep touched a live key".into());
        }
        println!("    a {cap}-byte value, a merge, a delete, a purge and a sweep on S3");
        Ok(())
    })
}

type Case = fn() -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![("set_get_put_delete_purge_and_sweep_on_minio", everything)]
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
