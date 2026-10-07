//! W9c Task 3: the KV expiry sweep (`storage::kv::sweep`, spec §11).
//! Requires DATABASE_URL.
//!
//! # Its own binary, serial
//!
//! A pass is global: it sweeps every expired key of the database, whatever
//! the workspace. So it lives apart from `tests/kv_store.rs`, whose tests
//! hold expired rows of their own (the test database is per process), and
//! every test here holds [`SERIAL`] for its whole body. Each test drains
//! what earlier tests left before it seeds, so its counts are its own.
//!
//! The only object stores touched are `MemStore`s.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::{MemStore, ObjectStore};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::store::{set, KvWritten, NewValue};
use egret_workflow::storage::kv::sweep::{
    sweep_expired, sweep_expired_with_hook, KvSweepConfig, SweepHook,
};
use egret_workflow::storage::kv::value::{KvType, KvValue};
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use egret_workflow::storage::nsfiles::CreatedBy;
use egret_workflow::storage::{self, FileStore};
use serde_json::json;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const USER: CreatedBy = CreatedBy::User(Uuid::nil());

fn mem_fs() -> (FileStore, Arc<MemStore>) {
    let mem = Arc::new(MemStore::new());
    let fs = FileStore {
        store: mem.clone(),
        bucket: "b".into(),
        prefix: "workflow/".into(),
        caps: storage::Caps::default(),
        retention_days: 30,
    };
    (fs, mem)
}

fn wgrant(ws: Uuid, ns: &str) -> Grant {
    authorize(ws, Actor::Api, ns, Resource::Kv, Access::Write).unwrap()
}

fn key(k: &str) -> KvKey {
    KvKey::parse(k).unwrap()
}

fn cfg(batch: i64) -> KvSweepConfig {
    KvSweepConfig {
        batch,
        ..KvSweepConfig::default()
    }
}

/// `revisions` writes of `k`, the latest with a ttl when `ttl`.
async fn write(db: &Db, fs: &FileStore, ws: Uuid, ns: &str, k: &str, revisions: u32, ttl: bool) {
    for r in 1..=revisions {
        let nv = NewValue {
            value: KvValue::from_json(KvType::String, &json!(format!("v{r}"))).unwrap(),
            description: None,
            ttl: (ttl && r == revisions).then(|| chrono::TimeDelta::seconds(60)),
            overwrite: true,
        };
        assert!(matches!(
            set(&db.pool, fs, &wgrant(ws, ns), &key(k), nv, USER).await,
            Ok(KvWritten::Written(_))
        ));
    }
}

/// Every row of the workspace's key with a ttl now expired.
async fn expire(db: &Db, ws: Uuid, ns: &str, k: &str) {
    sqlx::query(
        "UPDATE workflow.kv_entries SET expires_at = now() - interval '1 second' \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3 AND expires_at IS NOT NULL",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn revisions(db: &Db, ws: Uuid, ns: &str, k: &str) -> Vec<i32> {
    sqlx::query_scalar(
        "SELECT revision FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3 ORDER BY revision",
    )
    .bind(ws)
    .bind(ns)
    .bind(k)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

fn objects(mem: &MemStore, ws: Uuid) -> Vec<String> {
    let prefix = format!("s3://b/workflow/{ws}/");
    let mut keys: Vec<String> = mem
        .keys()
        .into_iter()
        .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
        .collect();
    keys.sort();
    keys
}

/// What earlier tests left expired is swept first.
async fn drain(db: &Db) {
    let (fs, _) = mem_fs();
    sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap();
}

#[tokio::test]
async fn the_sweep_removes_an_expired_key_its_rows_and_objects() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    let ns = "w9c.sw";
    write(&db, &fs, ws, ns, "gone", 3, true).await;
    write(&db, &fs, ws, ns, "live", 1, true).await;
    write(&db, &fs, ws, ns, "never", 1, false).await;
    // An expired OLDER revision under a live latest: the key stays whole.
    write(&db, &fs, ws, ns, "older", 1, true).await;
    expire(&db, ws, ns, "older").await;
    write(&db, &fs, ws, ns, "older", 1, false).await;
    expire(&db, ws, ns, "gone").await;
    assert_eq!(objects(&mem, ws).len(), 7);

    let removed = sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap();

    assert_eq!(removed, 1);
    assert!(revisions(&db, ws, ns, "gone").await.is_empty());
    assert_eq!(revisions(&db, ws, ns, "live").await, [1]);
    assert_eq!(revisions(&db, ws, ns, "never").await, [1]);
    assert_eq!(revisions(&db, ws, ns, "older").await, [1, 2]);
    assert_eq!(
        objects(&mem, ws),
        [
            "w9c/sw/_kv/live.ion",
            "w9c/sw/_kv/never.ion",
            "w9c/sw/_kv/older.ion",
            "w9c/sw/_kv/older.ion.v2"
        ]
    );
    // A soft-deleted latest that expired is swept too.
    write(&db, &fs, ws, ns, "deleted", 2, true).await;
    egret_workflow::storage::kv::store::delete(&db.pool, &wgrant(ws, ns), &key("deleted"), USER)
        .await
        .unwrap();
    expire(&db, ws, ns, "deleted").await;
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap(), 1);
    assert!(revisions(&db, ws, ns, "deleted").await.is_empty());
}

/// Mutation (g): a key rewritten between the listing and its lock keeps
/// every revision.
#[tokio::test]
async fn the_sweep_keeps_a_key_rewritten_between_listing_and_lock() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    let ns = "w9c.sw";
    write(&db, &fs, ws, ns, "rw", 1, true).await;
    write(&db, &fs, ws, ns, "plain", 1, true).await;
    expire(&db, ws, ns, "rw").await;
    expire(&db, ws, ns, "plain").await;
    let hooked = Arc::new(AtomicUsize::new(0));
    let hook: SweepHook = {
        let (db, fs, hooked) = (db.clone(), fs.clone(), hooked.clone());
        Arc::new(move |w, n, k| {
            let (db, fs, hooked) = (db.clone(), fs.clone(), hooked.clone());
            Box::pin(async move {
                if w == ws && k == "rw" {
                    hooked.fetch_add(1, Ordering::SeqCst);
                    write(&db, &fs, ws, &n, "rw", 1, false).await;
                }
            })
        })
    };
    let removed = sweep_expired_with_hook(&db.pool, &fs, &cfg(1000), hook)
        .await
        .unwrap();
    assert_eq!(hooked.load(Ordering::SeqCst), 1);
    assert_eq!(removed, 1, "only `plain`");
    assert_eq!(revisions(&db, ws, ns, "rw").await, [1, 2]);
    assert_eq!(
        objects(&mem, ws),
        ["w9c/sw/_kv/rw.ion", "w9c/sw/_kv/rw.ion.v2"]
    );
}

#[tokio::test]
async fn the_sweep_skips_a_busy_key_and_the_next_pass_takes_it() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    let ns = "w9c.sw";
    write(&db, &fs, ws, ns, "busy", 2, true).await;
    expire(&db, ws, ns, "busy").await;
    let text = format!("w9c:{}:{ns}:busy", ws.hyphenated());
    let mut holder = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(&text)
        .execute(&mut *holder)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap(), 0);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "it waited"
    );
    assert_eq!(revisions(&db, ws, ns, "busy").await, [1, 2]);
    sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(&text)
        .execute(&mut *holder)
        .await
        .unwrap();
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap(), 1);
    assert!(revisions(&db, ws, ns, "busy").await.is_empty());
    assert!(objects(&mem, ws).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sweeps_in_parallel_remove_each_key_once() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    let ns = "w9c.sw2";
    for i in 0..40 {
        write(&db, &fs, ws, ns, &format!("k{i}"), 2, true).await;
    }
    sqlx::query(
        "UPDATE workflow.kv_entries SET expires_at = now() - interval '1 second' \
          WHERE workspace_id = $1 AND expires_at IS NOT NULL",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
    let pass = || {
        let (pool, fs) = (db.pool.clone(), fs.clone());
        tokio::spawn(async move { sweep_expired(&pool, &fs, &cfg(7)).await.unwrap() })
    };
    let (a, b) = (pass(), pass());
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    eprintln!("the two passes removed {a} and {b} keys");
    assert_eq!(a + b, 40);
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
    assert!(objects(&mem, ws).is_empty());
}

#[tokio::test]
async fn the_sweep_covers_every_workspace_and_flowless_namespaces() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (w1, _) = common::seed_workspace(&db).await;
    let (w2, _) = common::seed_workspace(&db).await;
    let (fs, mem) = mem_fs();
    // No flow exists in any of these namespaces.
    for (ws, ns) in [
        (w1, "w9c.noflow"),
        (w1, "other.deep.ns"),
        (w2, "w9c.noflow"),
    ] {
        write(&db, &fs, ws, ns, "k", 1, true).await;
        expire(&db, ws, ns, "k").await;
    }
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap(), 3);
    for (ws, ns) in [
        (w1, "w9c.noflow"),
        (w1, "other.deep.ns"),
        (w2, "w9c.noflow"),
    ] {
        assert!(revisions(&db, ws, ns, "k").await.is_empty(), "{ns}");
    }
    assert!(objects(&mem, w1).is_empty() && objects(&mem, w2).is_empty());
}

#[tokio::test]
async fn the_sweep_pages_past_one_batch() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let (fs, _) = mem_fs();
    let ns = "w9c.page";
    for i in 0..7 {
        write(&db, &fs, ws, ns, &format!("k{i}"), 1, true).await;
        expire(&db, ws, ns, &format!("k{i}")).await;
    }
    write(&db, &fs, ws, ns, "live", 1, true).await;
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(3)).await.unwrap(), 7);
    assert_eq!(revisions(&db, ws, ns, "live").await, [1]);
}

/// A failed object delete is a WARN orphan: the rows go, the pass goes on.
#[tokio::test]
async fn a_failed_object_delete_leaves_an_orphan_and_the_rows_go() {
    let _g = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    drain(&db).await;
    let (ws, _) = common::seed_workspace(&db).await;
    let mem = Arc::new(MemStore::new());
    struct NoDelete(Arc<MemStore>);
    #[async_trait::async_trait]
    impl ObjectStore for NoDelete {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), egret_storage::StorageError> {
            self.0.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
            self.0.get(uri).await
        }
        async fn delete(&self, uri: &str) -> Result<(), egret_storage::StorageError> {
            if uri.contains("/_kv/") {
                return Err(egret_storage::StorageError::S3("down".into()));
            }
            self.0.delete(uri).await
        }
        async fn copy(&self, src: &str, dst: &str) -> Result<(), egret_storage::StorageError> {
            self.0.copy(src, dst).await
        }
        async fn list(
            &self,
            prefix: &str,
            delimiter: Option<&str>,
        ) -> Result<egret_storage::Listing, egret_storage::StorageError> {
            self.0.list(prefix, delimiter).await
        }
        async fn head(&self, uri: &str) -> Result<Option<u64>, egret_storage::StorageError> {
            self.0.head(uri).await
        }
    }
    let fs = FileStore {
        store: Arc::new(NoDelete(mem.clone())),
        ..mem_fs().0
    };
    let ns = "w9c.orphan";
    for k in ["a", "b"] {
        write(&db, &fs, ws, ns, k, 1, true).await;
        expire(&db, ws, ns, k).await;
    }
    assert_eq!(sweep_expired(&db.pool, &fs, &cfg(1000)).await.unwrap(), 2);
    assert!(revisions(&db, ws, ns, "a").await.is_empty());
    assert!(revisions(&db, ws, ns, "b").await.is_empty());
    assert_eq!(objects(&mem, ws).len(), 2, "the orphans");
}
