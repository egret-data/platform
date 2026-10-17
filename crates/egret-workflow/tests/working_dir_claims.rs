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
