//! W9a Task 13: the pruner and the orphan sweep against MinIO (a real
//! `S3Store`): `delete_prefix` of an ended execution, and the sweep reading
//! S3's own `last_modified` (MemStore has none, so only a real store proves
//! the age is read at all).
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`) and
//! `DATABASE_URL`. Without the S3 variables every case is reported
//! **ignored**, never passed (`harness = false`, as `storage_render_s3_live`).
//!
//! # It deletes, so it is fenced
//!
//! The `FileStore`'s prefix is a throwaway `w9a-t13-test/<uuid>/`, and the
//! store is wrapped in a [`Fence`] that REFUSES (and records) every `delete`
//! and `delete_prefix` whose key does not start with it. The case fails if
//! anything was refused, and ends by deleting what is left by exact key.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_storage::{Listing, ObjectStore, S3Store, StorageConfig, StorageError};
use egret_workflow::storage::prune::{prune_pass, StoragePruneConfig};
use egret_workflow::storage::{index_task_files, uri, Caps, FileStore, StoredFile};
use tokio::io::{AsyncBufRead, AsyncRead};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(300);
const NS: &str = "w9a.t13";
const FLOW: &str = "S3_Prune";

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

/// The S3 store, refusing any delete outside `allowed` (`s3://bucket/w9a-t13-test/<uuid>/`).
struct Fence {
    s3: S3Store,
    allowed: String,
    refused: Mutex<Vec<String>>,
    prefixes: Mutex<Vec<String>>,
}

impl Fence {
    fn check(&self, uri: &str) -> Result<(), StorageError> {
        if uri.starts_with(&self.allowed) && uri.len() > self.allowed.len() {
            return Ok(());
        }
        self.refused.lock().unwrap().push(uri.to_string());
        Err(StorageError::S3(format!(
            "{uri}: outside the test's prefix, refused"
        )))
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
        self.check(uri)?;
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
        self.prefixes.lock().unwrap().push(uri.to_string());
        self.check(uri)?;
        self.s3.delete_prefix(uri).await
    }
}

fn prune_and_sweep(cfg: StorageConfig) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let Some(db) = common::db().await else {
            return Err("DATABASE_URL is required".into());
        };
        let throwaway = format!("w9a-t13-test/{}/", Uuid::new_v4());
        let bucket = cfg.bucket.clone();
        let fence = Arc::new(Fence {
            s3: S3Store::new(&cfg).await.map_err(|e| e.to_string())?,
            allowed: format!("s3://{bucket}/{throwaway}"),
            refused: Mutex::new(Vec::new()),
            prefixes: Mutex::new(Vec::new()),
        });
        let fs = FileStore {
            store: fence.clone(),
            bucket: bucket.clone(),
            prefix: throwaway.clone(),
            caps: Caps::default(),
            retention_days: 30,
        };
        let (ws, _) = common::seed_workspace(&db).await;
        let exec = |state: &'static str, days: i32| {
            let db = db.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO workflow.executions (workspace_id, namespace, flow_key, source, \
                       state, history, updated_at) VALUES ($1, $2, $3, 'id: f\ntasks: []\n', $4, \
                       jsonb_build_array($4::text), now() - make_interval(days => $5)) \
                     RETURNING id",
                )
                .bind(ws)
                .bind(NS)
                .bind(FLOW)
                .bind(state)
                .bind(days)
                .fetch_one(&db.pool)
                .await
                .map_err(|e| e.to_string())
            }
        };
        let put = |execution: Uuid, name: &'static str, index: bool| {
            let db = db.clone();
            let fs = &fs;
            async move {
                let run: Uuid = sqlx::query_scalar(
                    "INSERT INTO workflow.task_runs (execution_id, run_ix, task_ix, task_id, \
                       type_id, state, history) VALUES ($1, (SELECT count(*) FROM \
                       workflow.task_runs WHERE execution_id = $1), 0, 'Write_It', \
                       'io.kestra.plugin.core.storage.Write', 'SUCCESS', '[\"SUCCESS\"]'::jsonb) \
                     RETURNING id",
                )
                .bind(execution)
                .fetch_one(&db.pool)
                .await
                .map_err(|e| e.to_string())?;
                let file = format!(
                    "{}{name}",
                    uri::task_prefix(NS, FLOW, execution, "Write_It", run)?
                );
                let object = fs.object_uri(ws, &file)?;
                fs.store
                    .put(&object, b"abc".to_vec())
                    .await
                    .map_err(|e| e.to_string())?;
                if index {
                    let mut conn = db.pool.acquire().await.map_err(|e| e.to_string())?;
                    index_task_files(
                        &mut conn,
                        ws,
                        run,
                        &[StoredFile {
                            uri: file.clone(),
                            size: 3,
                        }],
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok::<_, String>(object)
            }
        };

        let result = async {
            let old = exec("SUCCESS", 400).await?;
            let pruned = put(old, "a.txt", true).await?;
            let failed = exec("FAILED", 2).await?;
            let indexed = put(failed, "kept.txt", true).await?;
            let orphan = put(failed, "orphan.txt", false).await?;
            let exists = |o: String| {
                let fence = fence.clone();
                async move { fence.head(&o).await.map(|h| h.is_some()) }
            };

            // Pass 1, orphans older than an hour: the old execution is pruned
            // by `delete_prefix` on S3; the just-written orphan is too young.
            let hour = StoragePruneConfig {
                orphan_age: Duration::from_secs(3600),
                ..StoragePruneConfig::new(30)
            };
            let r1 = prune_pass(&db.pool, &fs, &hour)
                .await
                .map_err(|e| e.to_string())?;
            println!("    pass 1: {r1:?}");
            if exists(pruned.clone()).await.map_err(|e| e.to_string())? {
                return Err(format!("{pruned} survived the prune ({r1:?})"));
            }
            if !exists(orphan.clone()).await.map_err(|e| e.to_string())? {
                return Err(format!("a young orphan was deleted ({r1:?})"));
            }

            // Pass 2, orphans older than 2 s, once 3 s have passed: S3's
            // `last_modified` is read, the orphan goes, the indexed file stays.
            tokio::time::sleep(Duration::from_secs(3)).await;
            sqlx::query("UPDATE workflow.executions SET files_swept_at = NULL WHERE id = $1")
                .bind(failed)
                .execute(&db.pool)
                .await
                .map_err(|e| e.to_string())?;
            let seconds = StoragePruneConfig {
                orphan_age: Duration::from_secs(2),
                ..StoragePruneConfig::new(30)
            };
            let r2 = prune_pass(&db.pool, &fs, &seconds)
                .await
                .map_err(|e| e.to_string())?;
            println!("    pass 2: {r2:?}");
            if exists(orphan.clone()).await.map_err(|e| e.to_string())? {
                return Err(format!(
                    "the orphan survived: S3's last_modified was not read ({r2:?})"
                ));
            }
            if !exists(indexed.clone()).await.map_err(|e| e.to_string())? {
                return Err("an indexed file was swept".into());
            }
            if r2.orphans != 1 {
                return Err(format!("{r2:?}: one orphan expected"));
            }
            Ok(())
        }
        .await;

        // Every prefix handed to delete_prefix was under the throwaway prefix.
        let prefixes = fence.prefixes.lock().unwrap().clone();
        println!("    delete_prefix calls: {prefixes:?}");
        let outside: Vec<&String> = prefixes
            .iter()
            .filter(|p| !p.starts_with(&fence.allowed))
            .collect();
        // Clean up by exact key, under the throwaway prefix only.
        let left = fence
            .list(&throwaway, None)
            .await
            .map_err(|e| e.to_string())?;
        for o in &left.objects {
            fence
                .delete(&format!("s3://{bucket}/{}", o.key))
                .await
                .map_err(|e| e.to_string())?;
        }
        let refused = fence.refused.lock().unwrap().clone();
        result?;
        if !outside.is_empty() || !refused.is_empty() {
            return Err(format!(
                "deletes outside {throwaway}: {outside:?}, refused {refused:?}"
            ));
        }
        if prefixes.is_empty() {
            return Err("no delete_prefix was made: the prune did not reach S3".into());
        }
        Ok(())
    })
}

type Case = fn(StorageConfig) -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![(
        "an_ended_execution_is_pruned_and_an_old_orphan_swept_on_minio",
        prune_and_sweep,
    )]
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
    let cfg = cfg();
    for (name, case) in &selected {
        let Some(cfg) = cfg.clone() else {
            println!("test {name} ... ignored, EGRET_TEST_S3_ENDPOINT unset");
            ignored += 1;
            continue;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let case = *case;
        std::thread::spawn(move || {
            let _ = tx.send(case(cfg));
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
