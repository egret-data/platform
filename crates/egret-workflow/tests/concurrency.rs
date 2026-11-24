//! Flow concurrency, package 8a Task 2: the race-safe admission gate. A start
//! over its flow's `concurrency.limit` (behavior QUEUE) is born `QUEUED` and
//! starts no task; the gate's count and the row's INSERT are one transaction
//! under a per-flow advisory lock.
//!
//! Every test is `#[sqlx::test]`: each owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5467/egret_test
//!   cargo test -p egret-workflow --test concurrency

use std::sync::Arc;

use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

mod common;

fn flow_src(limit: Option<(i32, &str)>) -> String {
    let conc = limit
        .map(|(n, b)| format!("concurrency:\n  limit: {n}\n  behavior: {b}\n"))
        .unwrap_or_default();
    format!(
        "id: c\nnamespace: t\n{conc}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

async fn setup(pool: &sqlx::PgPool, src: &str) -> (Uuid, Uuid) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = egret_workflow::spec::parse(src).expect("parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: src.to_string(),
        plan: serde_json::to_value(&flow).expect("serialises"),
        folder_id: None,
    };
    let flow_id = egret_workflow::FlowRepo::new(db)
        .create(ws, user, &input)
        .await
        .expect("file the flow")
        .id;
    (ws, flow_id)
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, flow: Option<Uuid>, src: &str) -> Uuid {
    egret_workflow::worker::start_execution(pool, ws, flow, src, json!({}))
        .await
        .expect("start")
}

async fn state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("state")
}

async fn run_count(pool: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("runs")
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_start_over_the_limit_is_queued_and_starts_no_task(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src(Some((2, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let first = start(&pool, ws, Some(flow), &src).await;
    let second = start(&pool, ws, Some(flow), &src).await;

    // Act
    let third = start(&pool, ws, Some(flow), &src).await;

    // Assert
    assert_eq!(state(&pool, first).await, "RUNNING");
    assert_eq!(state(&pool, second).await, "RUNNING");
    assert_eq!(state(&pool, third).await, "QUEUED");
    assert_eq!(
        run_count(&pool, third).await,
        0,
        "a queued start runs no task"
    );
    assert_eq!(run_count(&pool, first).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_without_concurrency_is_never_queued(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src(None);
    let (ws, flow) = setup(&pool, &src).await;

    // Act
    let mut states = Vec::new();
    for _ in 0..4 {
        let id = start(&pool, ws, Some(flow), &src).await;
        states.push(state(&pool, id).await);
    }

    // Assert
    assert!(states.iter().all(|s| s == "RUNNING"), "{states:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_start_with_no_flow_id_is_never_gated(pool: sqlx::PgPool) {
    // Arrange: a NULL flow_id (the flow was deleted) has no scope to count in.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, _flow) = setup(&pool, &src).await;

    // Act
    let a = start(&pool, ws, None, &src).await;
    let b = start(&pool, ws, None, &src).await;

    // Assert
    assert_eq!(state(&pool, a).await, "RUNNING");
    assert_eq!(state(&pool, b).await, "RUNNING");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_terminal_execution_frees_its_slot_and_queued_ones_do_not_count(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let first = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, queued).await, "QUEUED");
    sqlx::query(
        "UPDATE workflow.executions SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb, ended_at = now() WHERE id = $1",
    )
    .bind(first)
    .execute(&pool)
    .await
    .expect("finish the live one");

    // Act: nothing live (the QUEUED row is not live), so the next start runs.
    let next = start(&pool, ws, Some(flow), &src).await;

    // Assert
    assert_eq!(state(&pool, next).await, "RUNNING");
}

/// The critical one. Limit 2, one live: two starts RACE for the last slot,
/// released together by a barrier, each on its own pool connection. The
/// advisory lock must let exactly one run and queue the other. Repeated, so a
/// missing lock shows up as a flake-free failure rather than a lucky pass.
#[sqlx::test(migrations = "../../migrations")]
async fn two_racing_starts_at_the_boundary_admit_exactly_one(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src(Some((2, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let live = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, live).await, "RUNNING");

    for round in 0..8 {
        // Reset to exactly one live execution: finish everything else.
        sqlx::query(
            "UPDATE workflow.executions SET state = 'SUCCESS', \
             history = history || '[\"SUCCESS\"]'::jsonb, ended_at = now() \
             WHERE flow_id = $1 AND id <> $2 AND NOT workflow.state_is_terminal(state)",
        )
        .bind(flow)
        .bind(live)
        .execute(&pool)
        .await
        .expect("reset");

        // Act
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let (pool, src, barrier) = (pool.clone(), src.clone(), barrier.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                start(&pool, ws, Some(flow), &src).await
            }));
        }
        let mut ids = Vec::new();
        for t in tasks {
            ids.push(t.await.expect("join"));
        }

        // Assert
        let mut states = vec![state(&pool, ids[0]).await, state(&pool, ids[1]).await];
        states.sort();
        assert_eq!(
            states,
            vec!["QUEUED".to_string(), "RUNNING".to_string()],
            "round {round}: exactly one of the two racing starts is admitted"
        );
    }
}
