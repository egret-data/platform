//! `WorkingDirectory`'s own claims, end to end through the claim path
//! (spec §5.1, §5.3, plan Task 5 and Task 6 Step 3).
//!
//! No Docker here: the children are `io.kestra.plugin.core.debug.Return`, which
//! has no working directory of its own, so these tests judge the block's
//! prepare and finish claims, the snapshot pointer, and the corrupt-block
//! refusal. The copy of a directory through a container is judged in
//! `working_dir_docker_live.rs`.

mod common;

use egret_workflow::worker::{tick_with, Handlers, WorkerTiming};
use sqlx::PgPool;

/// A block of two `Return` children, `a` then `b`.
const BLOCK: &str = "id: wd-claims\nnamespace: w6.claims\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n";

/// The block's row: `(state, working_dir, working_dir_snapshot)`.
async fn block_row(pool: &PgPool, ex: uuid::Uuid) -> (String, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT state, working_dir, working_dir_snapshot FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .fetch_one(pool)
    .await
    .expect("the block's run")
}

/// `(state, message)` of the child named `task`.
async fn child_row(pool: &PgPool, ex: uuid::Uuid, task: &str) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("the child {task}: {e}"))
}

/// Tick until the block is terminal, or fail after `rounds` ticks.
async fn settle(pool: &PgPool, reg: &Handlers, ex: uuid::Uuid, rounds: usize) {
    for _ in 0..rounds {
        tick_with(pool, 8, reg, WorkerTiming::default())
            .await
            .expect("a tick");
        let (state, _, _) = block_row(pool, ex).await;
        if matches!(state.as_str(), "SUCCESS" | "FAILED" | "WARNING") {
            return;
        }
    }
    panic!("the block did not settle in {rounds} ticks");
}

fn registry() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

/// The block is prepared (a snapshot pointer is written by the prepare claim),
/// both children run in order, and the finish claim ends it `FINISHED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_is_prepared_runs_its_children_and_is_finished(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let reg = registry();

    let ex = egret_workflow::worker::start_execution(&pool, ws, None, BLOCK, serde_json::json!({}))
        .await
        .expect("start");
    settle(&pool, &reg, ex, 40).await;

    let (state, phase, snapshot) = block_row(&pool, ex).await;
    assert_eq!(state, "SUCCESS");
    assert_eq!(phase.as_deref(), Some("FINISHED"), "the finish claim ran");
    assert!(
        snapshot.is_some(),
        "the prepare claim wrote the snapshot the first child starts from"
    );
    assert_eq!(child_row(&pool, ex, "a").await.0, "SUCCESS");
    assert_eq!(child_row(&pool, ex, "b").await.0, "SUCCESS");
}

/// Spec §5.2: a child whose block holds no snapshot is refused as corrupt. It
/// fails by name and never starts on an empty directory. The block is driven
/// to the state where its first child is waiting, and the pointer is removed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_child_whose_block_has_no_snapshot_fails_as_corrupt(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let reg = registry();

    let ex = egret_workflow::worker::start_execution(&pool, ws, None, BLOCK, serde_json::json!({}))
        .await
        .expect("start");
    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the prepare claim");
    let (state, phase, snapshot) = block_row(&pool, ex).await;
    assert_eq!(state, "RUNNING");
    assert_eq!(phase.as_deref(), Some("PREPARED"));
    assert!(snapshot.is_some(), "the prepare claim wrote the pointer");

    sqlx::query(
        "UPDATE workflow.task_runs SET working_dir_snapshot = NULL \
          WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("remove the pointer");

    settle(&pool, &reg, ex, 40).await;

    let (state, message) = child_row(&pool, ex, "a").await;
    assert_eq!(state, "FAILED", "the child is refused, not run");
    let message = message.expect("the reason");
    assert!(message.contains("corrupt"), "{message}");
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'b'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(
        runs, 0,
        "a sibling after the refused child gets no run (V1)"
    );
    assert_eq!(block_row(&pool, ex).await.0, "FAILED");
}

/// A block whose prepare writes an archive (`inputFiles`) and whose NORMAL
/// finish would write `outputs` (`outputFiles`): a discard-only finish writes
/// neither and leaves nothing under `_wd/{block}/`.
const KILLED_BLOCK: &str = "id: wd-reaped\nnamespace: w6.claims\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    inputFiles:\n      a.txt: one\n    outputFiles:\n      - a.txt\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n";

/// Kill Task 5 (spec §4.3, carry 3): **a discard-only finish whose worker died
/// is reaped and claimed again as a discard-only BLOCK claim**, never as a
/// leaf's kill, and it finishes: the block is `KILLED`, its `_wd/{block}/`
/// objects are deleted and no `outputs` are written.
///
/// The kill is planted the way the route and the Exit leave it: the
/// execution `KILLING`, and the block's unclaimed child `KILLING` with no
/// lease (its attempt takes the state as `apply` would give it). The first
/// tick finishes the child's kill through the claim's kill arm, which makes
/// the block ask for its discard-only finish.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reaped_discard_only_block_claim_is_claimed_again_and_discards(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let reg = registry();

    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the prepare claim");
    assert_eq!(block_row(&pool, ex).await.1.as_deref(), Some("PREPARED"));
    let block: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("block id");
    let prefix = fs.working_dir_prefix(ws, ex, block);
    let key = prefix
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k.to_string())
        .expect("an s3:// URI");
    let left = || async { fs.store.list(&key, None).await.expect("list").objects.len() };
    assert!(
        left().await > 0,
        "the prepare wrote under the block's prefix"
    );

    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'KILLING', history = history || '[\"KILLING\"]'::jsonb, \
                attempts = jsonb_set(attempts, ARRAY[(jsonb_array_length(attempts) - 1)::text, 'history'], \
                                     (attempts -> -1 -> 'history') || '[\"KILLING\"]'::jsonb) \
          WHERE execution_id = $1 AND task_id = 'a' AND claimed_at IS NULL AND state = 'RUNNING'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("the child -> KILLING");
    sqlx::query(
        "UPDATE workflow.executions \
            SET state = 'KILLING', history = history || '[\"KILLING\"]'::jsonb WHERE id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("the execution -> KILLING");

    // The child's kill is finished by the claim's kill arm; the block then asks
    // for its discard-only finish.
    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the child's kill");
    assert_eq!(child_row(&pool, ex, "a").await.0, "KILLED");
    let (state, phase, _) = block_row(&pool, ex).await;
    assert_eq!(
        (state.as_str(), phase.as_deref()),
        ("KILLING", Some("FINISHING"))
    );

    // The worker that dies: it takes the discard-only finish and never
    // comes back.
    let dead = egret_workflow::worker::claim(&pool, 8)
        .await
        .expect("the dead worker's claim");
    assert_eq!(dead.len(), 1, "only the block's finish is claimable");
    assert_eq!(dead[0].id, block);
    assert_eq!(dead[0].block_phase.as_deref(), Some("FINISHING"));
    assert!(!dead[0].kill_only, "a block is never a leaf's kill");
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = now() - interval '2 hours' WHERE id = $1",
    )
    .bind(block)
    .execute(&pool)
    .await
    .expect("the lease expires");
    assert_eq!(
        egret_workflow::worker::reap(&pool, std::time::Duration::from_secs(3600))
            .await
            .expect("reap"),
        1,
        "the reaper frees a discard-only block claim's expired lease"
    );
    let lease: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT claimed_at FROM workflow.task_runs WHERE id = $1")
            .bind(block)
            .fetch_one(&pool)
            .await
            .expect("lease");
    assert_eq!(lease, None);
    assert_eq!(
        block_row(&pool, ex).await.0,
        "KILLING",
        "the reap leaves the state"
    );

    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the discard-only finish, claimed again");
    let (state, phase, _) = block_row(&pool, ex).await;
    assert_eq!(state, "KILLED");
    assert_eq!(phase.as_deref(), Some("FINISHED"), "the finish claim ran");
    let outputs: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT outputs FROM workflow.task_runs WHERE id = $1")
            .bind(block)
            .fetch_one(&pool)
            .await
            .expect("outputs");
    assert_eq!(outputs, None, "a discard-only finish writes no outputs");
    assert_eq!(
        left().await,
        0,
        "every object under _wd/{{block}}/ is deleted"
    );
    assert!(
        swept_at(&pool, block).await.is_some(),
        "debt D6: the discard-only finish stamped the block"
    );
    let exec: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(&pool)
        .await
        .expect("execution");
    assert_eq!(exec, "KILLED");
}

/// **Debt D4, fix round 1: a discard-only finish of a block whose execution
/// can no longer be loaded closes it, instead of failing its commit, being
/// reaped and claimed again for ever.** The kill is planted as in
/// [`a_reaped_discard_only_block_claim_is_claimed_again_and_discards`]; the
/// source is then corrupted (a planner change is the likely cause), and one
/// tick takes the discard-only claim, discards, and closes the block and the
/// execution `KILLED` by force.
#[sqlx::test(migrations = "../../migrations")]
async fn a_discard_only_block_claim_of_an_unloadable_execution_closes_it(pool: PgPool) {
    // Arrange: the block is `KILLING` in `FINISHING`, as the kill leaves it.
    egret_test_db::assert_database_url_is_exported();
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let reg = registry();
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the prepare claim");
    let block: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("block id");
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'KILLING', history = history || '[\"KILLING\"]'::jsonb, \
                attempts = jsonb_set(attempts, ARRAY[(jsonb_array_length(attempts) - 1)::text, 'history'], \
                                     (attempts -> -1 -> 'history') || '[\"KILLING\"]'::jsonb) \
          WHERE execution_id = $1 AND task_id = 'a' AND claimed_at IS NULL AND state = 'RUNNING'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("the child -> KILLING");
    sqlx::query(
        "UPDATE workflow.executions \
            SET state = 'KILLING', history = history || '[\"KILLING\"]'::jsonb WHERE id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect("the execution -> KILLING");
    tick_with(&pool, 8, &reg, WorkerTiming::default())
        .await
        .expect("the child's kill");
    let (state, phase, _) = block_row(&pool, ex).await;
    assert_eq!(
        (state.as_str(), phase.as_deref()),
        ("KILLING", Some("FINISHING"))
    );
    sqlx::query("UPDATE workflow.executions SET source = 'tasks: [' WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .expect("corrupt the source");
    // Act
    let ticked = tick_with(&pool, 8, &reg, WorkerTiming::default()).await;
    // Assert
    assert!(ticked.is_ok(), "{ticked:?}");
    assert_eq!(block_row(&pool, ex).await.0, "KILLED");
    let exec: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(&pool)
        .await
        .expect("state");
    assert_eq!(exec, "KILLED");
    let prefix = fs.working_dir_prefix(ws, ex, block);
    let key = prefix
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k.to_string())
        .expect("an s3:// URI");
    assert_eq!(
        fs.store.list(&key, None).await.expect("list").objects.len(),
        0,
        "the discard ran before the commit"
    );
    let again = egret_workflow::worker::claim(&pool, 8).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

// ── Debt D6: the `_wd` crash sweep ──────────────────────────────────────────

/// The `_wd/{block}/` key prefix of block run `block` (an `s3://` URI's key).
fn wd_key(
    fs: &egret_workflow::storage::FileStore,
    ws: uuid::Uuid,
    ex: uuid::Uuid,
    block: uuid::Uuid,
) -> String {
    fs.working_dir_prefix(ws, ex, block)
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k.to_string())
        .expect("an s3:// URI")
}

/// How many objects are left under `key`.
async fn objects_under(fs: &egret_workflow::storage::FileStore, key: &str) -> usize {
    fs.store.list(key, None).await.expect("list").objects.len()
}

/// The block run's `working_dir_swept_at`.
async fn swept_at(pool: &PgPool, block: uuid::Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT working_dir_swept_at FROM workflow.task_runs WHERE id = $1")
        .bind(block)
        .fetch_one(pool)
        .await
        .expect("working_dir_swept_at")
}

/// The block run's id.
async fn block_id(pool: &PgPool, ex: uuid::Uuid) -> uuid::Uuid {
    sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
    )
    .bind(ex)
    .fetch_one(pool)
    .await
    .expect("block id")
}

/// The block run ended `hours` ago.
async fn ended_hours_ago(pool: &PgPool, block: uuid::Uuid, hours: i32) {
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET started_at = CASE WHEN started_at IS NULL THEN NULL \
                                  ELSE now() - make_interval(hours => $2 + 1) END, \
                ended_at = now() - make_interval(hours => $2) \
          WHERE id = $1",
    )
    .bind(block)
    .bind(hours)
    .execute(pool)
    .await
    .expect("age the block's end");
}

/// "The crash": every run and the execution ended `FAILED` by hand, the runs
/// `hours` ago, and no finish claim ran.
async fn crash(pool: &PgPool, ex: uuid::Uuid, hours: i32) {
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'FAILED', history = history || '[\"FAILED\"]'::jsonb, \
                claimed_at = NULL, \
                started_at = CASE WHEN started_at IS NULL THEN NULL \
                                  ELSE now() - make_interval(hours => $2 + 1) END, \
                ended_at = now() - make_interval(hours => $2) WHERE execution_id = $1",
    )
    .bind(ex)
    .bind(hours)
    .execute(pool)
    .await
    .expect("the runs -> FAILED");
    sqlx::query(
        "UPDATE workflow.executions SET state = 'FAILED', history = history || '[\"FAILED\"]'::jsonb, \
                ended_at = now() WHERE id = $1",
    )
    .bind(ex)
    .execute(pool)
    .await
    .expect("the execution -> FAILED");
}

/// **Debt D6: a crashed block's `_wd/{block}/` is swept 24 h after its run
/// ended, once.** The prepare claim writes the snapshot; the "crash" is the
/// execution ended by hand with no finish claim, so nothing discarded it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_wd_sweep_removes_a_crashed_blocks_snapshots_once(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    tick_with(&pool, 8, &registry(), WorkerTiming::default())
        .await
        .expect("the prepare claim");
    let block = block_id(&pool, ex).await;
    let key = wd_key(&fs, ws, ex, block);
    assert!(
        objects_under(&fs, &key).await > 0,
        "the fixture: a snapshot exists"
    );
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    // A block of a live execution is never swept, however old its row.
    sqlx::query(
        "UPDATE workflow.task_runs SET updated_at = now() - interval '2 days' WHERE id = $1",
    )
    .bind(block)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        0
    );
    assert!(
        objects_under(&fs, &key).await > 0,
        "a running execution's block is kept"
    );
    // The crash: every row terminal, no finish claim ran.
    crash(&pool, ex, 23).await;
    // Act + Assert: young, kept.
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        0
    );
    assert!(
        objects_under(&fs, &key).await > 0,
        "a block ended 23 h ago is not swept"
    );
    // Old: swept, and stamped.
    ended_hours_ago(&pool, block, 25).await;
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        objects_under(&fs, &key).await,
        0,
        "every object under _wd/{{block}}/ is gone"
    );
    assert!(swept_at(&pool, block).await.is_some());
    // Once: a second pass finds nothing.
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        0
    );
}

/// **Debt D6: a block that finishes normally is stamped by its discard**, so
/// the crash sweep never lists it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_normal_finish_stamps_swept_at(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    // Act
    settle(&pool, &registry(), ex, 40).await;
    // Assert
    let block = block_id(&pool, ex).await;
    let (state, phase, _) = block_row(&pool, ex).await;
    assert_eq!(
        (state.as_str(), phase.as_deref()),
        ("SUCCESS", Some("FINISHED"))
    );
    assert_eq!(
        objects_under(&fs, &wd_key(&fs, ws, ex, block)).await,
        0,
        "discarded"
    );
    assert!(
        swept_at(&pool, block).await.is_some(),
        "the discard stamped the block"
    );
    ended_hours_ago(&pool, block, 25).await;
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        0
    );
}

/// A store whose `delete_prefix` fails; everything else is the inner store's.
struct FailingDeletes(std::sync::Arc<dyn egret_storage::ObjectStore>);

#[async_trait::async_trait]
impl egret_storage::ObjectStore for FailingDeletes {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), egret_storage::StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3(format!(
            "{uri}: the store is down"
        )))
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
    async fn delete_prefix(&self, uri: &str) -> Result<u64, egret_storage::StorageError> {
        Err(egret_storage::StorageError::S3(format!(
            "{uri}: the store is down"
        )))
    }
}

/// **Debt D6: a discard whose delete fails leaves `working_dir_swept_at`
/// NULL, so the crash sweep removes the snapshots later**; a sweep whose
/// delete fails also leaves it NULL and ends its pass.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_delete_leaves_swept_at_null_for_the_sweep(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a prepared block, and a store over the same objects whose
    // deletes fail.
    let fs = egret_workflow::storage::install_mem_for_tests();
    let failing = egret_workflow::storage::FileStore {
        store: std::sync::Arc::new(FailingDeletes(fs.store.clone())),
        bucket: fs.bucket.clone(),
        prefix: fs.prefix.clone(),
        caps: fs.caps,
        retention_days: fs.retention_days,
    };
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    tick_with(&pool, 8, &registry(), WorkerTiming::default())
        .await
        .expect("the prepare claim");
    let block = block_id(&pool, ex).await;
    let key = wd_key(&fs, ws, ex, block);
    assert!(
        objects_under(&fs, &key).await > 0,
        "the fixture: a snapshot exists"
    );
    // Act: the discard's delete fails.
    egret_workflow::worker::discard_snapshots(&pool, &failing, ws, ex, block).await;
    // Assert
    assert!(objects_under(&fs, &key).await > 0, "nothing was deleted");
    assert_eq!(
        swept_at(&pool, block).await,
        None,
        "a failed delete is not stamped"
    );
    // The sweep's delete fails too: the pass ends, the row stays unstamped.
    crash(&pool, ex, 25).await;
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &failing, &cfg)
            .await
            .unwrap(),
        0
    );
    assert_eq!(swept_at(&pool, block).await, None);
    // A store that works: swept and stamped.
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        1
    );
    assert_eq!(objects_under(&fs, &key).await, 0);
    assert!(swept_at(&pool, block).await.is_some());
}

/// **Debt D6 with D4: a block a kill closed by force keeps its `_wd`
/// snapshots (no discard ran), and the crash sweep removes them.** The
/// selection reads the execution's terminal state and the block's
/// `working_dir`, never the run's state, so a `KILLED` block is listed.
#[sqlx::test(migrations = "../../migrations")]
async fn the_wd_sweep_removes_a_force_closed_blocks_snapshots(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a prepared block, its execution made unloadable and killed.
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        KILLED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    tick_with(&pool, 8, &registry(), WorkerTiming::default())
        .await
        .expect("the prepare claim");
    let block = block_id(&pool, ex).await;
    let key = wd_key(&fs, ws, ex, block);
    sqlx::query("UPDATE workflow.executions SET source = 'tasks: [' WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .expect("corrupt the source");
    let killed = egret_workflow::worker::kill_execution(&pool, ws, ex, true)
        .await
        .expect("the kill");
    assert_eq!(killed, egret_workflow::worker::KillOutcome::Killed);
    let (state, phase, _) = block_row(&pool, ex).await;
    assert_eq!(
        (state.as_str(), phase.as_deref()),
        ("KILLED", Some("PREPARED"))
    );
    assert!(
        objects_under(&fs, &key).await > 0,
        "the force-close discards nothing"
    );
    assert_eq!(swept_at(&pool, block).await, None);
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    assert_eq!(
        egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
            .await
            .unwrap(),
        0,
        "ended just now: kept"
    );
    // Act
    ended_hours_ago(&pool, block, 25).await;
    let swept = egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
        .await
        .unwrap();
    // Assert
    assert_eq!(swept, 1);
    assert_eq!(objects_under(&fs, &key).await, 0);
    assert!(swept_at(&pool, block).await.is_some());
}

// ── Task 2.3b: a block inside a `LoopUntil` ─────────────────────────────────

/// A `WaitFor` of two iterations (`maxIterations: 1`, as Kestra counts them)
/// over a block with an input file, whose one child `a` returns its own
/// `taskrun.id`.
const LOOPED_BLOCK: &str = "id: wd-loop\nnamespace: w6.claims\ntasks:\n  - id: until\n    type: io.kestra.plugin.core.flow.WaitFor\n    condition: \"false\"\n    checkFrequency:\n      maxIterations: 1\n      interval: PT0S\n    failOnMaxReached: false\n    tasks:\n      - id: block\n        type: io.kestra.plugin.core.flow.WorkingDirectory\n        inputFiles:\n          a.txt: one\n        tasks:\n          - id: a\n            type: io.kestra.plugin.core.debug.Return\n            format: \"{{ taskrun.id }}\"\n";

/// The child `a`'s row: `(id, state, history)`, or `None` before it exists.
async fn a_row(pool: &PgPool, ex: uuid::Uuid) -> Option<(uuid::Uuid, String, serde_json::Value)> {
    sqlx::query_as(
        "SELECT id, state, history FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .fetch_optional(pool)
    .await
    .expect("the child a")
}

async fn execution_state(pool: &PgPool, ex: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("the execution")
}

/// Drive `LOOPED_BLOCK` one claim at a time until the execution ends,
/// calling `each` after every tick with the block's current run id. Answers
/// every id `a` had while `SUCCESS`, in order, without repeats.
async fn drive_looped_block(
    pool: &PgPool,
    ex: uuid::Uuid,
    mut each: impl FnMut(uuid::Uuid) -> Option<uuid::Uuid>,
    fs: &egret_workflow::storage::FileStore,
    ws: uuid::Uuid,
) -> Vec<uuid::Uuid> {
    let reg = registry();
    let mut done = Vec::new();
    for _ in 0..200 {
        let claimed = tick_with(pool, 1, &reg, WorkerTiming::default())
            .await
            .expect("a tick");
        // A loop with an `interval:` waits `PAUSED` until it is due, and the
        // waker resumes it, as the runtime's tick does.
        if claimed == 0 {
            egret_workflow::worker::wake(pool, 8).await.expect("wake");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if let Some(stale) = each(block_id(pool, ex).await) {
            // The caller asked for an object under `_wd/{stale}/`, as a
            // discard whose delete failed would leave it.
            let uri = format!("{}left-behind", fs.working_dir_prefix(ws, ex, stale));
            fs.store.put(&uri, b"x".to_vec()).await.expect("put");
        }
        if let Some((id, state, _)) = a_row(pool, ex).await {
            if state == "SUCCESS" && done.last() != Some(&id) {
                done.push(id);
            }
        }
        if matches!(
            execution_state(pool, ex).await.as_str(),
            "SUCCESS" | "FAILED" | "WARNING" | "KILLED"
        ) {
            return done;
        }
    }
    panic!("the execution did not end in 200 ticks");
}

/// **A block inside a `LoopUntil` is prepared again each iteration, and its
/// child runs in both.** `Command::RestartLoop` left the block's
/// `working_dir` phase at `FINISHED`, so iteration 2's block took its
/// aggregate at once and `a` never ran (found by task 2.3, item (b)).
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_inside_a_loop_until_is_prepared_again_each_iteration(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        LOOPED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    // Act
    let ran = drive_looped_block(&pool, ex, |_| None, &fs, ws).await;
    // Assert
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    assert_eq!(
        ran.len(),
        2,
        "`a` ran in both iterations, with its own id each: {ran:?}"
    );
    let (_, state, history) = a_row(&pool, ex).await.expect("a");
    assert_eq!(state, "SUCCESS", "the last iteration's `a` ran: {history}");
    let block = block_id(&pool, ex).await;
    let (bstate, phase, snapshot) = block_row(&pool, ex).await;
    assert_eq!(
        (bstate.as_str(), phase.as_deref()),
        ("SUCCESS", Some("FINISHED"))
    );
    assert!(
        snapshot
            .as_deref()
            .is_some_and(|s| s.contains(&block.to_string())),
        "the last iteration prepared its own snapshot: {snapshot:?}"
    );
    assert_eq!(
        objects_under(&fs, &wd_key(&fs, ws, ex, block)).await,
        0,
        "discarded"
    );
    assert!(
        swept_at(&pool, block).await.is_some(),
        "the discard stamped it"
    );
}

/// **The same with an `interval:`, so the first iteration's discard stamps
/// its row before the restart.** The restart takes the block back to no
/// phase, and `working_dir_swept_at` must go back to NULL with it. Measured:
/// the restart and the next `BeginWorkingDirectory` land in one settle, so
/// the row goes from `FINISHED` to `PREPARING` and 0197's CHECK never sees
/// the NULL phase; without the reset the first iteration's stamp is carried
/// onto the second, which then reads as swept before its own discard ran (a
/// failed delete there would never be swept).
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_inside_a_waiting_loop_is_unstamped_when_it_restarts(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let source = LOOPED_BLOCK.replace("interval: PT0S", "interval: PT0.2S");
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    // Act
    let ran = drive_looped_block(&pool, ex, |_| None, &fs, ws).await;
    // Assert
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    assert_eq!(ran.len(), 2, "`a` ran in both iterations: {ran:?}");
    let block = block_id(&pool, ex).await;
    let ended: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT ended_at FROM workflow.task_runs WHERE id = $1")
            .bind(block)
            .fetch_one(&pool)
            .await
            .expect("the block's end");
    let stamp = swept_at(&pool, block).await.expect("stamped");
    assert!(
        stamp >= ended,
        "the stamp is the last discard's, after the last block ended ({ended}), not the \
         first iteration's carried over ({stamp})"
    );
}

/// **A superseded iteration's `_wd/{old id}/` is removed by a later
/// discard.** Debt D12 renames the block's row each iteration, so a discard
/// whose delete failed leaves objects under an id no row holds; nothing
/// would sweep them. The next discard in the execution removes every `_wd/`
/// prefix whose block id has no row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_later_discard_removes_a_superseded_iterations_snapshots(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: once the row is renamed, an object is put back under the
    // first iteration's prefix, as its failed delete would have left it.
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        LOOPED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let mut first: Option<uuid::Uuid> = None;
    let mut planted: Option<uuid::Uuid> = None;
    // Act
    drive_looped_block(
        &pool,
        ex,
        |id| match first {
            None => {
                first = Some(id);
                None
            }
            Some(f) if f != id && planted.is_none() => {
                planted = Some(f);
                Some(f)
            }
            _ => None,
        },
        &fs,
        ws,
    )
    .await;
    // Assert
    let stale = planted.expect("the row was renamed, and the object planted");
    assert_eq!(
        objects_under(&fs, &wd_key(&fs, ws, ex, stale)).await,
        0,
        "the superseded iteration's prefix was removed"
    );
}

/// **The `_wd` sweep removes a superseded iteration's prefix too**, for the
/// case where the last discard also failed: its block row is unstamped, and
/// the sweep, 24 h after the end, removes that row's prefix and every
/// superseded one of the execution.
#[sqlx::test(migrations = "../../migrations")]
async fn the_wd_sweep_removes_a_superseded_iterations_snapshots(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a finished loop; then objects left under the first
    // iteration's prefix and the last one's, the last row unstamped.
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        LOOPED_BLOCK,
        serde_json::json!({}),
    )
    .await
    .expect("start");
    let mut ids = Vec::new();
    drive_looped_block(
        &pool,
        ex,
        |id| {
            if ids.last() != Some(&id) {
                ids.push(id);
            }
            None
        },
        &fs,
        ws,
    )
    .await;
    let (stale, block) = (ids[0], *ids.last().unwrap());
    assert_ne!(stale, block, "the fixture: the row was renamed");
    for id in [stale, block] {
        let uri = format!("{}left-behind", fs.working_dir_prefix(ws, ex, id));
        fs.store.put(&uri, b"x".to_vec()).await.expect("put");
    }
    sqlx::query("UPDATE workflow.task_runs SET working_dir_swept_at = NULL WHERE id = $1")
        .bind(block)
        .execute(&pool)
        .await
        .expect("unstamp");
    ended_hours_ago(&pool, block, 25).await;
    // Act
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    let swept = egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg)
        .await
        .unwrap();
    // Assert
    assert_eq!(swept, 1);
    assert_eq!(objects_under(&fs, &wd_key(&fs, ws, ex, block)).await, 0);
    assert_eq!(
        objects_under(&fs, &wd_key(&fs, ws, ex, stale)).await,
        0,
        "the superseded prefix went with it"
    );
    assert!(swept_at(&pool, block).await.is_some());
}

/// **A block that is not the body's first task is restarted with no phase
/// and no snapshot.** Here the restart persists the block before the next
/// prepare (`pre` runs first), so the row is written with no phase, and
/// migration 0194's CHECK (a snapshot only beside a phase) and 0197's (a
/// stamp only beside a phase) both apply: the restart clears the previous
/// iteration's snapshot pointer and stamp with the phase.
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_after_another_task_in_a_loop_restarts_with_no_snapshot(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let source = LOOPED_BLOCK.replace(
        "      - id: block\n",
        "      - id: pre\n        type: io.kestra.plugin.core.debug.Return\n        format: p\n      - id: block\n",
    );
    let ex =
        egret_workflow::worker::start_execution(&pool, ws, None, &source, serde_json::json!({}))
            .await
            .expect("start");
    // Act
    let ran = drive_looped_block(&pool, ex, |_| None, &fs, ws).await;
    // Assert
    assert_eq!(execution_state(&pool, ex).await, "SUCCESS");
    assert_eq!(ran.len(), 2, "`a` ran in both iterations: {ran:?}");
    let block = block_id(&pool, ex).await;
    let (_, phase, snapshot) = block_row(&pool, ex).await;
    assert_eq!(phase.as_deref(), Some("FINISHED"));
    assert!(
        snapshot
            .as_deref()
            .is_some_and(|s| s.contains(&block.to_string())),
        "the last iteration prepared its own snapshot: {snapshot:?}"
    );
    assert!(swept_at(&pool, block).await.is_some());
}

/// End hooks A4 fix round: a block in the flow's `finally:` (and one in its
/// `errors:`) is found by the worker. `block_of` and `parent_block_of` used to
/// walk `flow.tasks` only, so a block outside it was refused as corrupt.
#[sqlx::test(migrations = "../../migrations")]
async fn a_block_in_the_flows_finally_or_errors_is_prepared_and_finished(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let reg = registry();
    let block = "  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n";
    for (list, first) in [
        (
            "finally",
            "  - id: ok\n    type: io.kestra.plugin.core.debug.Return\n    format: ok\n",
        ),
        (
            "errors",
            "  - id: ok\n    type: io.kestra.plugin.core.execution.Fail\n",
        ),
    ] {
        let src = format!("id: wd-end\nnamespace: w6.claims\ntasks:\n{first}{list}:\n{block}");
        let ex =
            egret_workflow::worker::start_execution(&pool, ws, None, &src, serde_json::json!({}))
                .await
                .expect("start");
        settle(&pool, &reg, ex, 40).await;
        let (state, phase, _) = block_row(&pool, ex).await;
        assert_eq!(state, "SUCCESS", "the block in the flow's `{list}:`");
        assert_eq!(phase.as_deref(), Some("FINISHED"), "`{list}:`");
        assert_eq!(child_row(&pool, ex, "a").await.0, "SUCCESS", "`{list}:`");
    }
}
