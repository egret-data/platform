//! W9a Task 13: the retention pruner of execution files and the orphan sweep
//! (`storage::prune`, spec §10). Requires DATABASE_URL.
//!
//! # Serial, on purpose
//!
//! [`prune_pass`] is global: it takes every ended loop family of the
//! database, whatever the workspace. Two tests pruning at once would prune
//! each other's fixtures (and delete their index rows while their objects sit
//! in another test's store), so every test holds [`SERIAL`] for its whole
//! body. Each test has its own [`Fake`] store, so an object of one test is
//! never in another's store.
//!
//! # Nothing here touches a real object store
//!
//! Every store is a [`Fake`]: a `MemStore` that also answers `last_modified`
//! (MemStore answers `None`, which the sweep reads as "age unknown, keep") and
//! fails `delete_prefix` on demand.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_metastore::Db;
use egret_storage::{Listing, MemStore, ObjectStore, StorageError};
use egret_workflow::runners::{FileUnavailable, RenderFiles};
use egret_workflow::storage::prune::{
    prune_pass, spawn_storage_pruner, PruneReport, StoragePruneConfig,
};
use egret_workflow::storage::scope::Snapshot;
use egret_workflow::storage::{
    self, index_task_files, resolve_readable, uri, Caps, FileStore, StoredFile, Unreadable,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const NS: &str = "w9a.t13";
const FLOW: &str = "Prune_Flow";
const BUCKET: &str = "egret-test";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// A `MemStore` with ages and a switch that makes `delete_prefix` fail.
#[derive(Default)]
struct Fake {
    mem: MemStore,
    /// key -> RFC 3339 `last_modified`; an absent key is "just written".
    modified: Mutex<HashMap<String, String>>,
    fail_delete_prefix: AtomicBool,
    /// Every prefix `delete_prefix` was asked for, refused or not.
    prefixes: Mutex<Vec<String>>,
}

fn key_of(object: &str) -> String {
    object
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k.to_string())
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl ObjectStore for Fake {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.mem.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.mem.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.mem.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.mem.copy(src, dst).await
    }
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError> {
        let mut listing = self.mem.list(prefix, delimiter).await?;
        let modified = self.modified.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        for o in &mut listing.objects {
            o.last_modified = Some(modified.get(&o.key).cloned().unwrap_or_else(|| now.clone()));
        }
        Ok(listing)
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.mem.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        self.prefixes.lock().unwrap().push(uri.to_string());
        if self.fail_delete_prefix.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!("{uri}: the store is down")));
        }
        self.mem.delete_prefix(uri).await
    }
}

struct World {
    db: Db,
    ws: Uuid,
    store: Arc<Fake>,
    fs: Arc<FileStore>,
}

impl World {
    async fn new() -> Option<Self> {
        let db = common::db().await?;
        let (ws, _) = common::seed_workspace(&db).await;
        let store = Arc::new(Fake::default());
        let fs = Arc::new(FileStore {
            store: store.clone(),
            bucket: BUCKET.into(),
            prefix: "workflow/".into(),
            caps: Caps::default(),
            retention_days: 30,
        });
        Some(Self { db, ws, store, fs })
    }

    /// An execution row; `iteration_of` makes it a loop iteration.
    async fn exec(&self, state: &str, days_ago: i32, iteration_of: Option<(Uuid, i32)>) -> Uuid {
        let (parent, run_ix, loop_index, loop_value) = match iteration_of {
            Some((p, i)) => (Some(p), Some(0), Some(i), Some(format!("v{i}"))),
            None => (None, None, None, None),
        };
        sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history, parent_execution_id, \
                parent_run_ix, loop_index, loop_value, updated_at) \
             VALUES ($1, $2, $3, 'id: f\ntasks: []\n', $4, jsonb_build_array($4::text), \
                     $5, $6, $7, $8, now() - make_interval(days => $9)) RETURNING id",
        )
        .bind(self.ws)
        .bind(NS)
        .bind(FLOW)
        .bind(state)
        .bind(parent)
        .bind(run_ix)
        .bind(loop_index)
        .bind(loop_value)
        .bind(days_ago)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert execution")
    }

    async fn set(&self, execution: Uuid, state: &str, days_ago: i32) {
        sqlx::query(
            "UPDATE workflow.executions SET state = $2, \
               updated_at = now() - make_interval(days => $3) WHERE id = $1",
        )
        .bind(execution)
        .bind(state)
        .bind(days_ago)
        .execute(&self.db.pool)
        .await
        .unwrap();
    }

    async fn age_hours(&self, execution: Uuid, hours: i32) {
        sqlx::query(
            "UPDATE workflow.executions SET updated_at = now() - make_interval(hours => $2) \
              WHERE id = $1",
        )
        .bind(execution)
        .bind(hours)
        .execute(&self.db.pool)
        .await
        .unwrap();
    }

    /// A file a task run of `execution` wrote: indexed and stored.
    async fn file(&self, execution: Uuid, name: &str) -> String {
        let uri = self.unindexed(execution, name).await;
        let run = self.task_run_of(&uri);
        let mut conn = self.db.pool.acquire().await.unwrap();
        index_task_files(
            &mut conn,
            self.ws,
            run,
            &[StoredFile {
                uri: uri.clone(),
                size: 3,
            }],
        )
        .await
        .expect("index the file");
        uri
    }

    /// A stored object with no index row (an upload never committed).
    async fn unindexed(&self, execution: Uuid, name: &str) -> String {
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
            uri::task_prefix(NS, FLOW, execution, "Write_It", run).unwrap()
        );
        self.store
            .put(&self.object(&uri), b"abc".to_vec())
            .await
            .unwrap();
        uri
    }

    fn task_run_of(&self, u: &str) -> Uuid {
        match uri::parse(u).unwrap().tail {
            uri::Tail::Task { task_run, .. } => task_run.parse().unwrap(),
            other => panic!("{other:?}"),
        }
    }

    fn object(&self, u: &str) -> String {
        self.fs.object_uri(self.ws, u).unwrap()
    }

    fn age_object(&self, u: &str, hours: i64) {
        let at = chrono::Utc::now() - chrono::Duration::hours(hours);
        self.store
            .modified
            .lock()
            .unwrap()
            .insert(key_of(&self.object(u)), at.to_rfc3339());
    }

    fn stored(&self, u: &str) -> bool {
        self.store.mem.keys().contains(&self.object(u))
    }

    async fn indexed(&self, u: &str) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM workflow.execution_files \
              WHERE workspace_id = $1 AND uri = $2)",
        )
        .bind(self.ws)
        .bind(u)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    async fn pruned_at(&self, execution: Uuid) -> bool {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT files_pruned_at FROM workflow.executions WHERE id = $1",
        )
        .bind(execution)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
        .is_some()
    }

    async fn swept_at(&self, execution: Uuid) -> bool {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT files_swept_at FROM workflow.executions WHERE id = $1",
        )
        .bind(execution)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
        .is_some()
    }

    async fn pass(&self) -> PruneReport {
        prune_pass(&self.db.pool, &self.fs, &StoragePruneConfig::new(30))
            .await
            .expect("a pass")
    }

    /// The whole file is still there: its row and its object.
    async fn kept(&self, u: &str) -> bool {
        self.indexed(u).await && self.stored(u)
    }

    /// Neither its row nor its object.
    async fn gone(&self, u: &str) -> bool {
        !self.indexed(u).await && !self.stored(u)
    }

    /// Every prefix `delete_prefix` was handed is one of this workspace's
    /// executions' own (`…/{ws}/w9a/t13/prune_flow/executions/{id}/`).
    fn prefixes_are_own(&self, executions: &[Uuid]) {
        let own: Vec<String> = executions
            .iter()
            .map(|e| self.fs.execution_prefix(self.ws, NS, FLOW, *e))
            .collect();
        for p in self.store.prefixes.lock().unwrap().iter() {
            assert!(
                own.contains(p) || !p.contains(&self.ws.to_string()),
                "{p} is not an execution prefix of this test's"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_execution_that_has_not_ended_is_never_pruned() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let running = w.exec("RUNNING", 400, None).await;
    let kept = w.file(running, "a.txt").await;
    // The control: the same age, ended, is pruned by the same pass.
    let ended = w.exec("SUCCESS", 400, None).await;
    let pruned = w.file(ended, "a.txt").await;

    let report = w.pass().await;

    assert!(w.kept(&kept).await, "a RUNNING execution lost its file");
    assert!(!w.pruned_at(running).await);
    assert!(w.gone(&pruned).await, "the ended control was not pruned");
    assert!(report.executions >= 1, "{report:?}");
    w.prefixes_are_own(&[ended]);
}

#[tokio::test]
async fn an_ended_execution_older_than_the_window_loses_objects_then_rows() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let old = w.exec("FAILED", 31, None).await;
    let a = w.file(old, "a.txt").await;
    let b = w.file(old, "dir/b.ion").await;
    sqlx::query(
        "INSERT INTO workflow.loop_file_values (execution_id, run_ix, idx, value) \
         VALUES ($1, 0, 0, '\"x\"')",
    )
    .bind(old)
    .execute(&w.db.pool)
    .await
    .unwrap();
    // Inside the window, and a neighbour of the same flow: both kept.
    let young = w.exec("SUCCESS", 29, None).await;
    let y = w.file(young, "a.txt").await;
    // Another workspace, the same namespace, flow and file name, also old.
    let other = World {
        db: w.db.clone(),
        ws: common::seed_workspace(&w.db).await.0,
        store: w.store.clone(),
        fs: w.fs.clone(),
    };
    let foreign_young = other.exec("SUCCESS", 1, None).await;
    let f = other.file(foreign_young, "a.txt").await;

    let report = w.pass().await;

    assert!(w.gone(&a).await && w.gone(&b).await, "{report:?}");
    assert!(w.pruned_at(old).await);
    let values: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.loop_file_values WHERE execution_id = $1",
    )
    .bind(old)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(values, 0, "the loop values went with the family's files");
    assert!(
        w.kept(&y).await,
        "an execution inside the window lost its file"
    );
    assert!(!w.pruned_at(young).await);
    assert!(other.kept(&f).await, "another workspace's file was touched");
    assert!(report.executions >= 1, "{report:?}");
    w.prefixes_are_own(&[old]);
}

#[tokio::test]
async fn an_ended_iteration_of_a_running_root_keeps_its_files() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("RUNNING", 0, None).await;
    let iteration = w.exec("SUCCESS", 400, Some((root, 0))).await;
    let file = w.file(iteration, "out.txt").await;
    let input = w.file(root, "in.txt").await;

    w.pass().await;
    assert!(
        w.kept(&file).await,
        "an iteration of a running root lost its file"
    );
    assert!(w.kept(&input).await);
    // A task after the loop (the root) still reads it.
    let mut conn = w.db.pool.acquire().await.unwrap();
    assert!(resolve_readable(&mut conn, w.ws, root, &file)
        .await
        .unwrap()
        .is_ok());
    drop(conn);

    // The root ends, inside the window: still kept.
    w.set(root, "SUCCESS", 1).await;
    w.pass().await;
    assert!(
        w.kept(&file).await,
        "the family was pruned on the iteration's end"
    );
    assert!(w.kept(&input).await);

    // The root ages past the window: root and iteration go together.
    w.set(root, "SUCCESS", 31).await;
    let report = w.pass().await;
    assert!(w.gone(&file).await && w.gone(&input).await, "{report:?}");
    assert!(w.pruned_at(root).await && w.pruned_at(iteration).await);
    w.prefixes_are_own(&[root, iteration]);
}

#[tokio::test]
async fn a_running_iteration_keeps_its_ended_roots_family() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("KILLED", 400, None).await;
    let iteration = w.exec("RUNNING", 400, Some((root, 0))).await;
    let a = w.file(root, "a.txt").await;
    let b = w.file(iteration, "b.txt").await;

    w.pass().await;
    assert!(
        w.kept(&a).await && w.kept(&b).await,
        "a family with a running iteration was pruned"
    );

    w.set(iteration, "KILLED", 400).await;
    w.pass().await;
    assert!(w.gone(&a).await && w.gone(&b).await);
}

/// An index row whose URI is not under its execution's prefix (here: the
/// row names another namespace than the execution's) is a row `delete_prefix`
/// would not reach: the family is kept, counted as failed, and logged.
#[tokio::test]
async fn index_rows_outside_the_executions_prefix_keep_the_family() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let e = w.exec("SUCCESS", 400, None).await;
    let own = w.file(e, "a.txt").await;
    let run: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, 99, 0, 'Write_It', 'io.kestra.plugin.core.storage.Write', 'SUCCESS', \
                 '[\"SUCCESS\"]'::jsonb) RETURNING id",
    )
    .bind(e)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    let elsewhere = format!(
        "{}b.txt",
        uri::task_prefix("w9a.elsewhere", FLOW, e, "Write_It", run).unwrap()
    );
    let mut conn = w.db.pool.acquire().await.unwrap();
    index_task_files(
        &mut conn,
        w.ws,
        run,
        &[StoredFile {
            uri: elsewhere.clone(),
            size: 1,
        }],
    )
    .await
    .unwrap();
    drop(conn);

    let report = w.pass().await;

    assert!(report.failed >= 1, "{report:?}");
    assert!(w.kept(&own).await, "{report:?}");
    assert!(w.indexed(&elsewhere).await);
    assert!(!w.pruned_at(e).await);
    // Clean up, so later tests' passes do not count it.
    sqlx::query("DELETE FROM workflow.execution_files WHERE execution_id = $1")
        .bind(e)
        .execute(&w.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_failed_store_delete_keeps_the_rows_and_the_next_pass_retries() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let e = w.exec("SUCCESS", 400, None).await;
    let a = w.file(e, "a.txt").await;

    w.store.fail_delete_prefix.store(true, Ordering::SeqCst);
    let report = w.pass().await;
    assert!(report.failed >= 1, "{report:?}");
    assert!(
        w.kept(&a).await,
        "the rows went although the store kept the object"
    );
    assert!(!w.pruned_at(e).await);

    w.store.fail_delete_prefix.store(false, Ordering::SeqCst);
    let report = w.pass().await;
    assert_eq!(report.failed, 0, "{report:?}");
    assert!(w.gone(&a).await, "the next pass did not retry");
    assert!(w.pruned_at(e).await);
}

#[tokio::test]
async fn orphans_older_than_a_day_are_deleted_and_younger_ones_and_indexed_ones_are_kept() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    // FAILED two days ago: inside the retention window, past the orphan age.
    let root = w.exec("FAILED", 2, None).await;
    let iteration = w.exec("FAILED", 2, Some((root, 0))).await;
    let indexed = w.file(root, "kept.txt").await;
    w.age_object(&indexed, 48);
    let old = w.unindexed(root, "old.txt").await;
    w.age_object(&old, 25);
    let young = w.unindexed(root, "young.txt").await;
    w.age_object(&young, 23);
    let old_in_iteration = w.unindexed(iteration, "old.txt").await;
    w.age_object(&old_in_iteration, 25);
    // A SUCCESS execution's old orphan is not swept (spec §10: pruned and
    // FAILED/KILLED executions only).
    let succeeded = w.exec("SUCCESS", 2, None).await;
    let not_swept = w.unindexed(succeeded, "old.txt").await;
    w.age_object(&not_swept, 25);

    let report = w.pass().await;

    assert!(!w.stored(&old), "an orphan older than a day was kept");
    assert!(
        !w.stored(&old_in_iteration),
        "an iteration's orphan was kept"
    );
    assert!(w.stored(&young), "an orphan younger than a day was deleted");
    assert!(w.kept(&indexed).await, "an indexed file was deleted");
    assert!(w.stored(&not_swept), "a SUCCESS execution was swept");
    assert_eq!(report.orphans, 2, "{report:?}");
    assert!(w.swept_at(root).await && w.swept_at(iteration).await);
    assert!(!w.pruned_at(root).await, "a sweep is not a prune");
    assert!(
        w.store.prefixes.lock().unwrap().is_empty(),
        "a sweep deletes by key, never by prefix"
    );

    // Swept once: an orphan that appears later is left for the prune.
    let later = w.unindexed(root, "later.txt").await;
    w.age_object(&later, 25);
    let report = w.pass().await;
    assert!(w.stored(&later), "{report:?}");
}

#[tokio::test]
async fn a_failed_or_killed_execution_ended_less_than_a_day_ago_is_not_swept() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("KILLED", 0, None).await;
    w.age_hours(root, 23).await;
    let old = w.unindexed(root, "old.txt").await;
    w.age_object(&old, 25);

    w.pass().await;
    assert!(w.stored(&old));
    assert!(!w.swept_at(root).await);

    w.age_hours(root, 25).await;
    w.pass().await;
    assert!(!w.stored(&old));
    assert!(w.swept_at(root).await);
}

#[tokio::test]
async fn zero_retention_starts_no_pruner() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let cancel = CancellationToken::new();
    assert!(spawn_storage_pruner(
        w.db.pool.clone(),
        w.fs.clone(),
        StoragePruneConfig::new(0),
        cancel.clone()
    )
    .is_none());

    // The control: a retention starts one, which prunes, and stops on cancel.
    let e = w.exec("SUCCESS", 400, None).await;
    let a = w.file(e, "a.txt").await;
    let handle = spawn_storage_pruner(
        w.db.pool.clone(),
        w.fs.clone(),
        StoragePruneConfig::new(30),
        cancel.clone(),
    )
    .expect("a pruner");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !w.gone(&a).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pruner never pruned"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("the pruner stops on cancel")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pruned_uri_is_an_error_naming_it_and_the_window() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    storage::install_mem_for_tests();
    let e = w.exec("SUCCESS", 400, None).await;
    let a = w.file(e, "a.txt").await;
    let never = format!(
        "{}never.txt",
        uri::task_prefix(NS, FLOW, e, "Write_It", Uuid::new_v4()).unwrap()
    );
    // The control: an execution that was not pruned answers Missing.
    let live = w.exec("SUCCESS", 1, None).await;
    let live_missing = format!(
        "{}never.txt",
        uri::task_prefix(NS, FLOW, live, "Write_It", Uuid::new_v4()).unwrap()
    );
    w.pass().await;

    let mut conn = w.db.pool.acquire().await.unwrap();
    let got = resolve_readable(&mut conn, w.ws, e, &a).await.unwrap();
    assert_eq!(
        got,
        Err(Unreadable::Pruned {
            uri: a.clone(),
            retention_days: 30
        })
    );
    let msg = got.unwrap_err().to_string();
    assert!(msg.contains(&a) && msg.contains("30 days"), "{msg}");
    assert!(matches!(
        resolve_readable(&mut conn, w.ws, e, &never).await.unwrap(),
        Err(Unreadable::Pruned { .. })
    ));
    assert_eq!(
        resolve_readable(&mut conn, w.ws, live, &live_missing)
            .await
            .unwrap(),
        Err(Unreadable::Missing {
            uri: live_missing.clone()
        })
    );

    // `fileExists` fails on it (Refused), where a Missing file answers false.
    let snap = Snapshot::take(&mut conn, w.fs.clone(), w.ws, e, &[a.as_str()], &[], None)
        .await
        .unwrap();
    match snap.lookup(&a) {
        Err(FileUnavailable::Refused(m)) => assert!(m.contains("30 days"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_pass_stops_at_its_budget() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let mut files = Vec::new();
    for _ in 0..3 {
        let e = w.exec("SUCCESS", 400, None).await;
        files.push(w.file(e, "a.txt").await);
    }
    let zero = StoragePruneConfig {
        pass_budget: Duration::ZERO,
        ..StoragePruneConfig::new(30)
    };

    let report = prune_pass(&w.db.pool, &w.fs, &zero).await.unwrap();

    let mut gone = 0;
    for f in &files {
        gone += usize::from(w.gone(f).await);
    }
    assert_eq!(gone, 1, "{report:?}");
    assert_eq!(report.executions, 1, "{report:?}");
    // The next passes take the rest.
    prune_pass(&w.db.pool, &w.fs, &zero).await.unwrap();
    prune_pass(&w.db.pool, &w.fs, &zero).await.unwrap();
    for f in &files {
        assert!(w.gone(f).await);
    }
}
