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
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Resource};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{put, soft_delete, Conflict};
use egret_workflow::storage::nsfiles::CreatedBy;
use egret_workflow::storage::prune::{
    prune_pass, prune_pass_with, spawn_storage_pruner, PruneReport, PruneState, StoragePruneConfig,
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
    /// `delete_prefix` and `list` fail for a prefix holding any of these.
    fail_containing: Mutex<Vec<String>>,
    /// Keys every listing also answers, wherever its prefix is (a store that
    /// misbehaves): the sweep must never trust a listing's prefix.
    inject: Mutex<Vec<String>>,
    /// Every prefix `delete_prefix` was asked for, refused or not.
    prefixes: Mutex<Vec<String>>,
    /// `delete_prefix` never answers (a pass to cancel mid-way).
    hang: AtomicBool,
    /// When `delete_prefix` is first asked for a prefix holding `.0`: insert a
    /// RUNNING manual replay of `.3` in workspace `.2` (a dependent that
    /// appears after the pass took its protected set).
    on_delete_prefix: Mutex<Option<(String, sqlx::PgPool, Uuid, Uuid)>>,
}

impl Fake {
    fn failing(&self, prefix: &str) -> bool {
        self.fail_containing
            .lock()
            .unwrap()
            .iter()
            .any(|f| prefix.contains(f.as_str()))
    }
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
        if self.failing(prefix) {
            return Err(StorageError::S3(format!("{prefix}: the store is down")));
        }
        let mut listing = self.mem.list(prefix, delimiter).await?;
        for key in self.inject.lock().unwrap().iter() {
            if !listing.objects.iter().any(|o| &o.key == key) {
                listing.objects.push(egret_storage::ObjectMeta {
                    key: key.clone(),
                    size: 3,
                    last_modified: None,
                });
            }
        }
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
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let hook = {
            let mut h = self.on_delete_prefix.lock().unwrap();
            if h.as_ref().is_some_and(|(c, ..)| uri.contains(c.as_str())) {
                h.take()
            } else {
                None
            }
        };
        if let Some((_, pool, ws, source)) = hook {
            sqlx::query(
                "INSERT INTO workflow.executions (workspace_id, namespace, flow_key, source, \
                   state, history, replayed_from_execution_id) \
                 VALUES ($1, $2, $3, 'id: f\ntasks: []\n', 'RUNNING', '[\"RUNNING\"]'::jsonb, $4)",
            )
            .bind(ws)
            .bind(NS)
            .bind(FLOW)
            .bind(source)
            .execute(&pool)
            .await
            .expect("insert the late dependent");
        }
        if self.fail_delete_prefix.load(Ordering::SeqCst) || self.failing(uri) {
            return Err(StorageError::S3(format!("{uri}: the store is down")));
        }
        self.mem.delete_prefix(uri).await
    }
    // W9b Task 3: namespace files are written through `put_stream`.
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.mem.put_stream(uri, body, size_hint).await
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
        self.exec_unlinked(state, days_ago, iteration_of).await
    }

    /// A manual replay of `source` (`replayed_from_execution_id`).
    async fn replay_of(&self, state: &str, days_ago: i32, source: Uuid) -> Uuid {
        let e = self.exec_unlinked(state, days_ago, None).await;
        self.link(e, "replayed_from_execution_id", source).await;
        e
    }

    /// An execution started by `trigger` (a `Subflow` child or a Flow trigger).
    async fn triggered_by(&self, state: &str, days_ago: i32, trigger: Uuid) -> Uuid {
        let e = self.exec_unlinked(state, days_ago, None).await;
        self.link(e, "trigger_execution_id", trigger).await;
        e
    }

    async fn link(&self, e: Uuid, column: &str, to: Uuid) {
        let sql = match column {
            "replayed_from_execution_id" => {
                "UPDATE workflow.executions SET replayed_from_execution_id = $2 WHERE id = $1"
            }
            "trigger_execution_id" => {
                "UPDATE workflow.executions SET trigger_execution_id = $2 WHERE id = $1"
            }
            other => panic!("{other}"),
        };
        sqlx::query(sql)
            .bind(e)
            .bind(to)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn exec_unlinked(
        &self,
        state: &str,
        days_ago: i32,
        iteration_of: Option<(Uuid, i32)>,
    ) -> Uuid {
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

    /// A FILE input of `execution`: indexed and stored.
    async fn input(&self, execution: Uuid, name: &str) -> String {
        let uri = uri::input_uri(NS, FLOW, execution, "f", name).unwrap();
        self.store
            .put(&self.object(&uri), b"abc".to_vec())
            .await
            .unwrap();
        let mut conn = self.db.pool.acquire().await.unwrap();
        storage::index_input_files(
            &mut conn,
            self.ws,
            execution,
            &[StoredFile {
                uri: uri.clone(),
                size: 3,
            }],
        )
        .await
        .expect("index the input");
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
    // A window that is not the default, so the number can only come from the
    // installed store (`Pruned` names `storage::current()`'s retention).
    let fs17 = Arc::new(FileStore {
        store: w.store.clone(),
        bucket: BUCKET.into(),
        prefix: "workflow/".into(),
        caps: Caps::default(),
        retention_days: 17,
    });
    storage::install(fs17.clone()).expect("no other test of this binary installs a store");
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
    prune_pass(&w.db.pool, &fs17, &StoragePruneConfig::new(17))
        .await
        .unwrap();

    let mut conn = w.db.pool.acquire().await.unwrap();
    let got = resolve_readable(&mut conn, w.ws, e, &a).await.unwrap();
    assert_eq!(
        got,
        Err(Unreadable::Pruned {
            uri: a.clone(),
            retention_days: 17
        })
    );
    let msg = got.unwrap_err().to_string();
    assert!(msg.contains(&a) && msg.contains("17 days"), "{msg}");
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
    let snap = Snapshot::take(
        &mut conn,
        w.fs.clone(),
        w.ws,
        e,
        &[a.as_str()],
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    match snap.lookup(&a) {
        Err(FileUnavailable::Refused(m)) => assert!(m.contains("17 days"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_pass_stops_at_its_budget() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    // Older than any other test's fixture (about 55 years), so these are the
    // first candidates whatever another test left behind.
    let mut files = Vec::new();
    for _ in 0..3 {
        let e = w.exec("SUCCESS", 20_000, None).await;
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

/// The user's ruling of 2026-10-04: a source is kept while a dependent runs.
/// A manual replay reads its source's files (the readable set); while it has
/// not ended, its 40-day-old source keeps them; once it ends, the next pass
/// prunes the source.
#[tokio::test]
async fn a_running_replay_keeps_its_sources_input_files_until_it_ends() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let source = w.exec("SUCCESS", 40, None).await;
    let input = w.input(source, "in.csv").await;
    let replay = w.replay_of("RUNNING", 0, source).await;

    let report = w.pass().await;
    assert!(w.kept(&input).await, "{report:?}");
    assert!(!w.pruned_at(source).await);

    w.set(replay, "SUCCESS", 0).await;
    let report = w.pass().await;
    assert!(w.gone(&input).await, "{report:?}");
    assert!(w.pruned_at(source).await);
    assert!(
        !w.pruned_at(replay).await,
        "the replay is inside the window"
    );
}

/// The same rule through `trigger_execution_id`: a running `Subflow` child or
/// Flow-trigger execution keeps its trigger execution's files.
#[tokio::test]
async fn a_running_child_keeps_its_trigger_executions_files_until_it_ends() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let parent = w.exec("SUCCESS", 40, None).await;
    let file = w.file(parent, "out.txt").await;
    let child = w.triggered_by("PAUSED", 0, parent).await;

    w.pass().await;
    assert!(w.kept(&file).await);

    w.set(child, "KILLED", 0).await;
    w.pass().await;
    assert!(w.gone(&file).await);
}

/// I1: the sweep twin of `a_running_iteration_keeps_its_ended_roots_family`.
/// A FAILED root ended two days ago with an iteration still RUNNING: that
/// iteration's upload in flight is not an orphan, the family is not swept and
/// stays unswept, so a later pass sweeps it once the iteration has ended.
#[tokio::test]
async fn the_sweep_skips_a_family_with_a_running_iteration() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("FAILED", 2, None).await;
    let iteration = w.exec("RUNNING", 2, Some((root, 0))).await;
    let uploading = w.unindexed(iteration, "big.bin").await;
    w.age_object(&uploading, 25);
    let old = w.unindexed(root, "old.txt").await;
    w.age_object(&old, 25);

    let report = w.pass().await;
    assert!(
        w.stored(&uploading),
        "a running iteration's upload was swept ({report:?})"
    );
    assert!(w.stored(&old), "{report:?}");
    assert!(!w.swept_at(root).await && !w.swept_at(iteration).await);

    w.set(iteration, "KILLED", 2).await;
    w.pass().await;
    assert!(!w.stored(&uploading) && !w.stored(&old));
    assert!(w.swept_at(root).await);
}

/// I2: a `Loop` over a file it did not write (a trigger's or a replay
/// source's) leaves `loop_file_values` rows and no `execution_files` row of
/// its own; the family is still a candidate, and its values go.
#[tokio::test]
async fn loop_values_without_files_are_pruned() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("SUCCESS", 40, None).await;
    let iteration = w.exec("SUCCESS", 40, Some((root, 0))).await;
    sqlx::query(
        "INSERT INTO workflow.loop_file_values (execution_id, run_ix, idx, value) \
         VALUES ($1, 0, 0, '\"a\"'), ($1, 0, 1, '\"b\"')",
    )
    .bind(root)
    .execute(&w.db.pool)
    .await
    .unwrap();

    let report = w.pass().await;

    let values: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.loop_file_values WHERE execution_id = $1",
    )
    .bind(root)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(values, 0, "{report:?}");
    assert!(w.pruned_at(root).await && w.pruned_at(iteration).await);
}

/// I3: a family whose store calls always fail is skipped for the rest of the
/// pass; the families after it are still pruned and swept.
#[tokio::test]
async fn a_family_that_always_fails_does_not_stall_the_others() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    // Prune: the older family fails, the younger one is pruned.
    let stuck = w.exec("SUCCESS", 19_000, None).await;
    let stuck_file = w.file(stuck, "a.txt").await;
    let next = w.exec("SUCCESS", 18_000, None).await;
    let next_file = w.file(next, "a.txt").await;
    // Sweep: the older FAILED root's listing fails, the younger one is swept.
    let stuck_failed = w.exec("FAILED", 3, None).await;
    let stuck_orphan = w.unindexed(stuck_failed, "o.txt").await;
    w.age_object(&stuck_orphan, 25);
    let next_failed = w.exec("FAILED", 2, None).await;
    let next_orphan = w.unindexed(next_failed, "o.txt").await;
    w.age_object(&next_orphan, 25);
    w.store
        .fail_containing
        .lock()
        .unwrap()
        .extend([stuck.to_string(), stuck_failed.to_string()]);

    let report = w.pass().await;

    assert!(w.kept(&stuck_file).await && !w.pruned_at(stuck).await);
    assert!(
        w.gone(&next_file).await,
        "a failing family stalled the prune ({report:?})"
    );
    assert!(w.stored(&stuck_orphan) && !w.swept_at(stuck_failed).await);
    assert!(
        !w.stored(&next_orphan),
        "a failing family stalled the sweep ({report:?})"
    );
    assert!(w.swept_at(next_failed).await);
    assert!(report.failed >= 2, "{report:?}");

    // The store recovers: the next pass takes both.
    w.store.fail_containing.lock().unwrap().clear();
    w.pass().await;
    assert!(w.gone(&stuck_file).await && !w.stored(&stuck_orphan));
}

/// One pruner per database: a pass that cannot take the advisory lock does
/// nothing.
#[tokio::test]
async fn a_pass_does_nothing_while_another_pruner_holds_the_lock() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let e = w.exec("SUCCESS", 400, None).await;
    let a = w.file(e, "a.txt").await;
    let mut other = w.db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(egret_workflow::storage::prune::PRUNE_LOCK_KEY)
        .execute(&mut *other)
        .await
        .unwrap();

    let report = w.pass().await;
    assert_eq!(report, PruneReport::default());
    assert!(w.kept(&a).await);

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(egret_workflow::storage::prune::PRUNE_LOCK_KEY)
        .execute(&mut *other)
        .await
        .unwrap();
    drop(other);
    w.pass().await;
    assert!(w.gone(&a).await);
}

/// The sweep deletes a listed key only when it lies under the member's own
/// prefix AND reads back, round trip, as that member's URI -- whatever the
/// store's listing says.
#[tokio::test]
async fn the_sweep_deletes_only_keys_that_are_provably_the_members() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let root = w.exec("FAILED", 2, None).await;
    // The control: an ordinary old orphan goes.
    let orphan = w.unindexed(root, "o.txt").await;
    w.age_object(&orphan, 25);
    // Under the prefix, but no URI: an empty segment does not parse.
    let odd = format!("{}tasks/x//y", w.fs.execution_prefix(w.ws, NS, FLOW, root));
    // A URI of the same execution under ANOTHER namespace's path: outside
    // the member's prefix, answered by a listing that misbehaves.
    let outside = format!(
        "s3://{BUCKET}/workflow/{}/w9a/elsewhere/prune-flow/executions/{root}/tasks/t/r/o.txt",
        w.ws
    );
    for k in [&odd, &outside] {
        w.store.put(k, b"abc".to_vec()).await.unwrap();
        let at = chrono::Utc::now() - chrono::Duration::hours(25);
        w.store
            .modified
            .lock()
            .unwrap()
            .insert(key_of(k), at.to_rfc3339());
    }
    w.store.inject.lock().unwrap().push(key_of(&outside));

    let report = w.pass().await;
    w.store.inject.lock().unwrap().clear();

    assert!(!w.stored(&orphan), "the control was kept ({report:?})");
    let keys = w.store.mem.keys();
    assert!(keys.contains(&odd), "a key that is no URI was deleted");
    assert!(
        keys.contains(&outside),
        "a key outside the prefix was deleted"
    );
}

/// N1: families that failed at the store in an earlier pass are not outage
/// evidence. Three stuck families at the head of the oldest-first order stop
/// the first pass (it cannot tell them from an outage), but the second pass,
/// remembering them, reaches the healthy family behind them -- in the prune
/// and in the sweep.
#[tokio::test]
async fn families_that_failed_before_do_not_keep_the_pruner_from_a_healthy_one() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let mut stuck = Vec::new();
    for days in [19_900, 19_800, 19_700] {
        let e = w.exec("SUCCESS", days, None).await;
        w.file(e, "a.txt").await;
        stuck.push(e);
    }
    let healthy = w.exec("SUCCESS", 19_600, None).await;
    let healthy_file = w.file(healthy, "a.txt").await;
    let mut stuck_failed = Vec::new();
    for days in [5, 4, 3] {
        stuck_failed.push(w.exec("FAILED", days, None).await);
    }
    let healthy_failed = w.exec("FAILED", 2, None).await;
    let orphan = w.unindexed(healthy_failed, "o.txt").await;
    w.age_object(&orphan, 25);
    w.store
        .fail_containing
        .lock()
        .unwrap()
        .extend(stuck.iter().chain(&stuck_failed).map(Uuid::to_string));
    let mut state = PruneState::default();
    let cfg = StoragePruneConfig::new(30);

    let first = prune_pass_with(&w.db.pool, &w.fs, &cfg, &mut state)
        .await
        .unwrap();
    let second = prune_pass_with(&w.db.pool, &w.fs, &cfg, &mut state)
        .await
        .unwrap();

    assert!(w.gone(&healthy_file).await, "{first:?} {second:?}");
    assert!(!w.stored(&orphan), "{first:?} {second:?}");
    assert!(w.swept_at(healthy_failed).await);
    for e in stuck.iter().chain(&stuck_failed) {
        assert!(!w.pruned_at(*e).await && !w.swept_at(*e).await);
    }
    // Clean up: the stuck families go once the store lets them.
    w.store.fail_containing.lock().unwrap().clear();
    prune_pass_with(&w.db.pool, &w.fs, &cfg, &mut state)
        .await
        .unwrap();
    for e in &stuck {
        assert!(w.pruned_at(*e).await);
    }
}

/// N1's other half: a store that fails for every family it has never failed
/// for before is an outage, and the phase still ends after three.
#[tokio::test]
async fn a_real_outage_still_ends_the_phase_after_three_families() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let mut files = Vec::new();
    for _ in 0..5 {
        let e = w.exec("SUCCESS", 400, None).await;
        files.push(w.file(e, "a.txt").await);
    }
    w.store.fail_delete_prefix.store(true, Ordering::SeqCst);
    w.store.prefixes.lock().unwrap().clear();

    let report = w.pass().await;

    assert_eq!(w.store.prefixes.lock().unwrap().len(), 3, "{report:?}");
    assert_eq!(report.failed, 3, "{report:?}");
    w.store.fail_delete_prefix.store(false, Ordering::SeqCst);
    w.pass().await;
    for f in &files {
        assert!(w.gone(f).await);
    }
}

/// m1: a pass dropped mid-way (cancelled while the store hangs) does not hand
/// the pool a session that still holds the advisory lock.
#[tokio::test]
async fn a_pass_dropped_mid_way_leaves_the_lock_free() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let e = w.exec("SUCCESS", 400, None).await;
    let a = w.file(e, "a.txt").await;
    w.store.hang.store(true, Ordering::SeqCst);

    let dropped = tokio::time::timeout(Duration::from_secs(2), w.pass()).await;
    assert!(dropped.is_err(), "the pass did not hang");
    w.store.hang.store(false, Ordering::SeqCst);

    // A session of its own, not the pool's: is the lock free?
    let url = egret_test_db::url().await.expect("the test database");
    let mut other = <sqlx::PgConnection as sqlx::Connection>::connect(&url)
        .await
        .unwrap();
    let mut free = false;
    for _ in 0..50 {
        free = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(egret_workflow::storage::prune::PRUNE_LOCK_KEY)
            .fetch_one(&mut other)
            .await
            .unwrap();
        if free {
            break;
        }
        // The dropped session's close reaches the server asynchronously.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(free, "a dropped pass left the lock held");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(egret_workflow::storage::prune::PRUNE_LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();
    drop(other);
    w.pass().await;
    assert!(w.gone(&a).await);
}

/// m2: a dependent that appears after the pass took its protected set (here,
/// a replay started while the pass deletes an older family) still keeps its
/// source: the family is checked again, directly, just before its delete.
#[tokio::test]
async fn a_dependent_started_during_the_pass_keeps_its_source() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let first = w.exec("SUCCESS", 19_990, None).await;
    let first_file = w.file(first, "a.txt").await;
    let source = w.exec("SUCCESS", 19_980, None).await;
    let source_file = w.input(source, "in.csv").await;
    *w.store.on_delete_prefix.lock().unwrap() =
        Some((first.to_string(), w.db.pool.clone(), w.ws, source));

    let report = w.pass().await;

    assert!(w.gone(&first_file).await, "{report:?}");
    assert!(
        w.kept(&source_file).await,
        "a late dependent's source was pruned"
    );
    let replay: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE replayed_from_execution_id = $1",
    )
    .bind(source)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    w.set(replay, "SUCCESS", 0).await;
    w.pass().await;
    assert!(w.gone(&source_file).await);
}

/// W9b spec §11: the pruner deletes under execution prefixes only. Namespace
/// files of the flow's own namespace -- the same `{ws}/w9a/t13/` directory
/// its executions live under -- written by `put`, one soft-deleted, rows and
/// objects aged far past every window, survive a pass that prunes an old
/// execution and sweeps an old orphan of that namespace (the controls).
#[tokio::test]
async fn a_prune_pass_leaves_namespace_files_untouched() {
    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let g = authorize(
        w.ws,
        Actor::Api,
        NS,
        Resource::NamespaceFiles,
        Access::Write,
    )
    .unwrap();
    for (p, body) in [
        ("/a.txt", "one"),
        ("/a.txt", "two"),
        ("/d/b.txt", "b"),
        ("/gone.txt", "g"),
        ("/executions/x.txt", "x"),
    ] {
        let mut body: &[u8] = body.as_bytes();
        put(
            &w.db.pool,
            &w.fs,
            &g,
            &NsPath::parse_file(p).unwrap(),
            &mut body,
            Conflict::Overwrite,
            CreatedBy::User(Uuid::nil()),
        )
        .await
        .unwrap();
    }
    soft_delete(&w.db.pool, &g, &NsPath::parse_file("/gone.txt").unwrap())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow.namespace_files SET created_at = now() - make_interval(days => 400) \
          WHERE workspace_id = $1",
    )
    .bind(w.ws)
    .execute(&w.db.pool)
    .await
    .unwrap();
    let ns_prefix = format!("{}/w9a/t13/_files/", w.ws);
    let ns_keys = || -> Vec<String> {
        w.store
            .mem
            .keys()
            .into_iter()
            .filter(|k| k.contains(&ns_prefix))
            .collect()
    };
    let old = (chrono::Utc::now() - chrono::Duration::days(400)).to_rfc3339();
    for k in ns_keys() {
        w.store
            .modified
            .lock()
            .unwrap()
            .insert(key_of(&k), old.clone());
    }
    let rows = || async {
        sqlx::query_as::<_, (String, i32, bool)>(
            "SELECT path, revision, deleted FROM workflow.namespace_files \
              WHERE workspace_id = $1 ORDER BY path, revision",
        )
        .bind(w.ws)
        .fetch_all(&w.db.pool)
        .await
        .unwrap()
    };
    let (rows_before, keys_before) = (rows().await, ns_keys());
    assert_eq!(
        (rows_before.len(), keys_before.len()),
        (7, 5),
        "{rows_before:?}"
    );
    // The controls: an ended execution past the window, and a FAILED one's
    // old orphan, both of this namespace.
    let ended = w.exec("SUCCESS", 400, None).await;
    let pruned = w.file(ended, "a.txt").await;
    let failed = w.exec("FAILED", 2, None).await;
    let orphan = w.unindexed(failed, "old.txt").await;
    w.age_object(&orphan, 25);

    let report = w.pass().await;

    assert!(w.gone(&pruned).await, "the prune control: {report:?}");
    assert!(!w.stored(&orphan), "the sweep control: {report:?}");
    assert_eq!(
        rows().await,
        rows_before,
        "a namespace file row was touched"
    );
    assert_eq!(
        ns_keys(),
        keys_before,
        "a namespace file object was deleted"
    );
    for p in w.store.prefixes.lock().unwrap().iter() {
        assert!(!p.contains("/_files"), "delete_prefix {p}");
    }
}

/// W9c Task 3: KV values of the flow's own namespace -- under the same
/// `{ws}/w9a/t13/` directory its executions live under, at `_kv/` -- three
/// revisions, one key soft-deleted, one expired, rows and objects aged far
/// past every window, survive a pass that prunes an old execution and
/// sweeps an old orphan of that namespace (the controls). The pruner never
/// reads `workflow.kv_entries`; the KV expiry sweep is the only thing that
/// removes KV values.
#[tokio::test]
async fn a_prune_pass_leaves_kv_untouched() {
    use egret_workflow::storage::kv::key::KvKey;
    use egret_workflow::storage::kv::store::{delete, set, NewValue};
    use egret_workflow::storage::kv::value::{KvType, KvValue};

    let _g = SERIAL.lock().await;
    let Some(w) = World::new().await else { return };
    let g = authorize(w.ws, Actor::Api, NS, Resource::Kv, Access::Write).unwrap();
    for (k, v, ttl) in [
        ("a", "one", None),
        ("a", "two", None),
        ("a", "three", None),
        ("gone", "g", None),
        ("old", "o", Some(chrono::TimeDelta::seconds(60))),
    ] {
        set(
            &w.db.pool,
            &w.fs,
            &g,
            &KvKey::parse(k).unwrap(),
            NewValue {
                value: KvValue::from_json(KvType::String, &serde_json::json!(v)).unwrap(),
                description: None,
                ttl,
                overwrite: true,
            },
            CreatedBy::User(Uuid::nil()),
        )
        .await
        .unwrap();
    }
    delete(
        &w.db.pool,
        &g,
        &KvKey::parse("gone").unwrap(),
        CreatedBy::User(Uuid::nil()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow.kv_entries SET created_at = now() - make_interval(days => 400), \
           expires_at = CASE WHEN expires_at IS NULL THEN NULL \
                             ELSE now() - make_interval(days => 399) END \
          WHERE workspace_id = $1",
    )
    .bind(w.ws)
    .execute(&w.db.pool)
    .await
    .unwrap();
    let kv_prefix = format!("{}/w9a/t13/_kv/", w.ws);
    let kv_keys = || -> Vec<String> {
        let mut keys: Vec<String> = w
            .store
            .mem
            .keys()
            .into_iter()
            .filter(|k| k.contains(&kv_prefix))
            .collect();
        keys.sort();
        keys
    };
    let old = (chrono::Utc::now() - chrono::Duration::days(400)).to_rfc3339();
    for k in kv_keys() {
        w.store
            .modified
            .lock()
            .unwrap()
            .insert(key_of(&k), old.clone());
    }
    let rows = || async {
        sqlx::query_as::<_, (String, i32, bool)>(
            "SELECT key, revision, deleted FROM workflow.kv_entries \
              WHERE workspace_id = $1 ORDER BY key, revision",
        )
        .bind(w.ws)
        .fetch_all(&w.db.pool)
        .await
        .unwrap()
    };
    let (rows_before, keys_before) = (rows().await, kv_keys());
    assert_eq!(
        (rows_before.len(), keys_before.len()),
        (5, 5),
        "{rows_before:?}"
    );
    let ended = w.exec("SUCCESS", 400, None).await;
    let pruned = w.file(ended, "a.txt").await;
    let failed = w.exec("FAILED", 2, None).await;
    let orphan = w.unindexed(failed, "old.txt").await;
    w.age_object(&orphan, 25);

    let report = w.pass().await;

    assert!(w.gone(&pruned).await, "the prune control: {report:?}");
    assert!(!w.stored(&orphan), "the sweep control: {report:?}");
    assert_eq!(rows().await, rows_before, "a KV row was touched");
    assert_eq!(kv_keys(), keys_before, "a KV object was deleted");
    for p in w.store.prefixes.lock().unwrap().iter() {
        assert!(!p.contains("/_kv"), "delete_prefix {p}");
    }
}
