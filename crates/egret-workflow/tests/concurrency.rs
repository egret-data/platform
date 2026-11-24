//! Flow concurrency, package 8a Task 2: the race-safe admission gate. A start
//! over its flow's `concurrency.limit` (behavior QUEUE) is born `QUEUED` and
//! starts no task; the gate's count and the row's INSERT are one transaction
//! under a per-flow advisory lock.
//!
//! Every test is `#[sqlx::test]`: each owns its database.
//!
//! Task 3 adds the drain (a terminal frees a slot, the oldest QUEUED runs) and
//! the safety sweep.
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

// ---- Task 3: the drain and the safety sweep ----------------------------------

async fn history(pool: &sqlx::PgPool, id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("history")
}

/// Run `n` task runs through the REAL worker tick (so through the real
/// `settle`, where an execution terminalizes and the drain is called).
async fn tick(pool: &sqlx::PgPool, n: usize) {
    egret_workflow::worker::tick(pool, n, &common::Answering(true))
        .await
        .expect("tick");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_terminal_promotes_the_queued_execution_and_it_runs_like_a_fresh_start(
    pool: sqlx::PgPool,
) {
    // Arrange: limit 1, one RUNNING, one QUEUED.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let running = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, queued).await, "QUEUED");
    let fresh_runs = run_count(&pool, running).await;

    // Act: the running one finishes (its only task completes).
    tick(&pool, 1).await;

    // Assert
    assert_eq!(state(&pool, running).await, "SUCCESS");
    assert_eq!(
        state(&pool, queued).await,
        "RUNNING",
        "the freed slot drains"
    );
    assert_eq!(
        history(&pool, queued).await,
        json!(["CREATED", "RUNNING"]),
        "the promoted row's history is a fresh start's"
    );
    assert_eq!(
        run_count(&pool, queued).await,
        fresh_runs,
        "its first settle created the same task runs a fresh start does"
    );
    let started: Option<bool> =
        sqlx::query_scalar("SELECT started_at IS NOT NULL FROM workflow.executions WHERE id = $1")
            .bind(queued)
            .fetch_one(&pool)
            .await
            .expect("started_at");
    assert_eq!(started, Some(true));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_oldest_queued_execution_is_promoted_first(pool: sqlx::PgPool) {
    // Arrange: two QUEUED; the later-inserted one is made the OLDER.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let _running = start(&pool, ws, Some(flow), &src).await;
    let first = start(&pool, ws, Some(flow), &src).await;
    let second = start(&pool, ws, Some(flow), &src).await;
    sqlx::query(
        "UPDATE workflow.executions SET created_at = created_at - interval '1 hour' WHERE id = $1",
    )
    .bind(second)
    .execute(&pool)
    .await
    .expect("age the second");

    // Act
    tick(&pool, 1).await;

    // Assert
    assert_eq!(state(&pool, second).await, "RUNNING", "the older one goes");
    assert_eq!(state(&pool, first).await, "QUEUED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_freed_slot_promotes_exactly_one(pool: sqlx::PgPool) {
    // Arrange: limit 2, two RUNNING, two QUEUED.
    let src = flow_src(Some((2, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let a = start(&pool, ws, Some(flow), &src).await;
    let b = start(&pool, ws, Some(flow), &src).await;
    let q1 = start(&pool, ws, Some(flow), &src).await;
    let q2 = start(&pool, ws, Some(flow), &src).await;

    // Act: ONE run completes, so ONE execution terminalizes.
    tick(&pool, 1).await;

    // Assert
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE flow_id = $1 \
         AND NOT workflow.state_is_terminal(state) AND state <> 'QUEUED'",
    )
    .bind(flow)
    .fetch_one(&pool)
    .await
    .expect("live");
    assert_eq!(live, 2, "never above the limit");
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE flow_id = $1 AND state = 'QUEUED'",
    )
    .bind(flow)
    .fetch_one(&pool)
    .await
    .expect("queued");
    assert_eq!(queued, 1, "exactly one promoted");
    let _ = (a, b, q1, q2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_sweep_recovers_a_missed_drain(pool: sqlx::PgPool) {
    // Arrange: a terminal that never drained (a crash between the two).
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let running = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    sqlx::query(
        "UPDATE workflow.executions SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb, ended_at = now() WHERE id = $1",
    )
    .bind(running)
    .execute(&pool)
    .await
    .expect("terminalize without a drain");
    assert_eq!(state(&pool, queued).await, "QUEUED");

    // Act
    let promoted = egret_workflow::worker::concurrency::concurrency_sweep(&pool)
        .await
        .expect("sweep");

    // Assert
    assert_eq!(promoted, 1);
    assert_eq!(state(&pool, queued).await, "RUNNING");
    assert_eq!(history(&pool, queued).await, json!(["CREATED", "RUNNING"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_queued_execution_is_not_reaped_and_the_sweep_leaves_a_full_flow_alone(
    pool: sqlx::PgPool,
) {
    // Arrange: limit 1 held by a live execution; the QUEUED one is ancient.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let _running = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    sqlx::query(
        "UPDATE workflow.executions SET created_at = created_at - interval '30 days' WHERE id = $1",
    )
    .bind(queued)
    .execute(&pool)
    .await
    .expect("age it");

    // Act: every periodic pass, with a lease timeout that expires at once.
    egret_workflow::runtime::reaper_pass(&pool, std::time::Duration::from_millis(1), None).await;
    egret_workflow::worker::reap(&pool, std::time::Duration::from_millis(1))
        .await
        .expect("reap");
    egret_workflow::worker::concurrency::concurrency_sweep(&pool)
        .await
        .expect("sweep");

    // Assert
    assert_eq!(state(&pool, queued).await, "QUEUED");
    assert_eq!(history(&pool, queued).await, json!(["CREATED", "QUEUED"]));
    assert_eq!(run_count(&pool, queued).await, 0);
}

/// A backlog of QUEUED executions that each complete INSIDE their first
/// settle (their only task is skipped) must drain iteratively, not by one
/// nested settle per execution: 40 of them overflow the default test-thread
/// stack if each promotion recurses.
#[sqlx::test(migrations = "../../migrations")]
async fn an_inline_completing_backlog_drains_with_bounded_recursion(pool: sqlx::PgPool) {
    // Arrange: limit 1, one RUNNING, 40 QUEUED rows whose source completes
    // inline (same flow_id, as a republished version would be).
    const BACKLOG: i64 = 40;
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let _running = start(&pool, ws, Some(flow), &src).await;
    for _ in 0..BACKLOG {
        start(&pool, ws, Some(flow), &src).await;
    }
    let inline = src.replace(
        "    message: hi\n",
        "    message: hi\n    runIf: \"{{ false }}\"\n",
    );
    assert_ne!(inline, src);
    sqlx::query(
        "UPDATE workflow.executions SET source = $2 WHERE flow_id = $1 AND state = 'QUEUED'",
    )
    .bind(flow)
    .bind(&inline)
    .execute(&pool)
    .await
    .expect("make the backlog inline-completing");
    let queued = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.executions WHERE flow_id = $1 AND state = 'QUEUED'",
        )
        .bind(flow)
        .fetch_one(&pool)
        .await
        .expect("queued")
    };

    // Act: the running one finishes; its drain starts the backlog.
    tick(&pool, 1).await;

    // Assert: the whole backlog drained, without overflowing the stack.
    assert_eq!(queued().await, 0, "the drain starts the whole backlog");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE flow_id = $1 \
         AND NOT workflow.state_is_terminal(state)",
    )
    .bind(flow)
    .fetch_one(&pool)
    .await
    .expect("live");
    assert_eq!(live, 0, "every inline-completing execution ended");
}

// ---- Package 8b Task 1: CANCEL and FAIL at the gate ---------------------------

async fn has_ended_at(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT ended_at >= created_at FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("ended_at")
}

/// Limit 1, one RUNNING; the over-limit start is born `terminal`, holds no
/// slot, and after the live one finishes a fresh start runs.
async fn over_limit_start_is_born_terminal(pool: sqlx::PgPool, behavior: &str, terminal: &str) {
    // Arrange
    let src = flow_src(Some((1, behavior)));
    let (ws, flow) = setup(&pool, &src).await;
    let live = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, live).await, "RUNNING");

    // Act
    let over = start(&pool, ws, Some(flow), &src).await;

    // Assert: terminal at birth, no task, ended_at stamped.
    assert_eq!(state(&pool, over).await, terminal);
    assert_eq!(history(&pool, over).await, json!(["CREATED", terminal]));
    assert_eq!(run_count(&pool, over).await, 0, "no task run");
    assert!(has_ended_at(&pool, over).await, "ended_at >= created_at");
    // Nothing was promoted or disturbed by the terminalization's drain.
    assert_eq!(state(&pool, live).await, "RUNNING");

    // The terminal row holds no slot: while #1 is live, another start is
    // still over the limit (terminal again, not queued)...
    let again = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, again).await, terminal);
    // ...and once #1 finishes, a fresh start runs.
    sqlx::query(
        "UPDATE workflow.executions SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb, ended_at = now() WHERE id = $1",
    )
    .bind(live)
    .execute(&pool)
    .await
    .expect("finish the live one");
    let fresh = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, fresh).await, "RUNNING");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_over_limit_start_with_cancel_is_born_cancelled_and_holds_no_slot(pool: sqlx::PgPool) {
    over_limit_start_is_born_terminal(pool, "CANCEL", "CANCELLED").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_over_limit_start_with_fail_is_born_failed_and_holds_no_slot(pool: sqlx::PgPool) {
    over_limit_start_is_born_terminal(pool, "FAIL", "FAILED").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_over_limit_start_with_queue_is_still_queued(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    start(&pool, ws, Some(flow), &src).await;

    // Act
    let over = start(&pool, ws, Some(flow), &src).await;

    // Assert
    assert_eq!(state(&pool, over).await, "QUEUED");
    assert_eq!(history(&pool, over).await, json!(["CREATED", "QUEUED"]));
}

// -- force-run (package 8b) --

#[sqlx::test(migrations = "../../migrations")]
async fn force_run_starts_a_queued_execution_even_when_the_flow_is_at_its_limit(
    pool: sqlx::PgPool,
) {
    // Arrange: limit 1, one RUNNING, one QUEUED.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let running = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, queued).await, "QUEUED");

    // Act
    let out = egret_workflow::worker::force_run(&pool, ws, queued)
        .await
        .expect("force-run");

    // Assert: it left QUEUED and runs; the first is still running, so the
    // flow is transiently at 2 against a limit of 1.
    assert_eq!(out, egret_workflow::worker::ForceRun::Started);
    assert_eq!(state(&pool, queued).await, "RUNNING");
    assert_eq!(state(&pool, running).await, "RUNNING");
    assert_eq!(history(&pool, queued).await, json!(["CREATED", "RUNNING"]));
    assert_eq!(
        run_count(&pool, queued).await,
        run_count(&pool, running).await
    );

    // A second force-run of the same row is refused, nothing changes.
    let again = egret_workflow::worker::force_run(&pool, ws, queued)
        .await
        .expect("force-run again");
    assert_eq!(
        again,
        egret_workflow::worker::ForceRun::NotQueued("RUNNING".into())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_queued_restart_resumes_to_restarted_not_created(pool: sqlx::PgPool) {
    // Arrange: limit 1. One live execution holds the slot; one QUEUED row whose
    // history ends on RESTARTED (as a restart-in-place gated to QUEUED would).
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let live = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, queued).await, "QUEUED");
    // Force the QUEUED row to look like a gated restart.
    sqlx::query("UPDATE workflow.executions SET history = $2 WHERE id = $1")
        .bind(queued)
        .bind(json!([
            "CREATED",
            "RUNNING",
            "FAILED",
            "RESTARTED",
            "QUEUED"
        ]))
        .execute(&pool)
        .await
        .unwrap();

    // Act: terminate the live one, free the slot, drain.
    sqlx::query(
        "UPDATE workflow.executions SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb WHERE id = $1",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    egret_workflow::worker::concurrency::concurrency_sweep(&pool)
        .await
        .unwrap();

    // Then: it left the queue; the history pop restored RESTARTED, so the
    // resumed run's history does NOT reset to ["CREATED"].
    assert_ne!(state(&pool, queued).await, "QUEUED");
    let h = history(&pool, queued).await;
    let arr = h.as_array().unwrap();
    assert!(
        arr.iter().any(|s| s == "RESTARTED"),
        "pop must keep the RESTARTED history, got {h}"
    );
    assert_ne!(arr, &vec![json!("CREATED")], "must not reset to CREATED");
}

/// Force an execution terminal `SUCCESS` (so it may be replayed and frees its slot).
async fn finish_success(pool: &sqlx::PgPool, id: Uuid) {
    sqlx::query(
        "UPDATE workflow.executions SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb, ended_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("finish");
}

/// A whole replay of `from`, through the public worker entry.
async fn whole_replay(pool: &sqlx::PgPool, ws: Uuid, from: Uuid, src: &str) -> Uuid {
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("owner");
    egret_workflow::worker::replay_execution(
        pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from,
            source: src,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: owner,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused")
}

/// Limit 1: one execution finished (to replay), one live holding the slot.
/// The whole replay is born `terminal` (or QUEUED), no task run.
async fn whole_replay_over_limit(pool: sqlx::PgPool, behavior: &str, terminal: &str) {
    let src = flow_src(Some((1, behavior)));
    let (ws, flow) = setup(&pool, &src).await;
    let old = start(&pool, ws, Some(flow), &src).await;
    finish_success(&pool, old).await;
    let live = start(&pool, ws, Some(flow), &src).await;

    let replayed = whole_replay(&pool, ws, old, &src).await;

    assert_eq!(state(&pool, replayed).await, terminal);
    assert_eq!(run_count(&pool, replayed).await, 0, "no task run");
    if terminal == "QUEUED" {
        assert_eq!(history(&pool, replayed).await, json!(["CREATED", "QUEUED"]));
        // The drain resumes it: free the slot, sweep, and it leaves QUEUED
        // with its replay identity (attempt, labels, origin) intact.
        let snapshot = |id| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (i32, Option<serde_json::Value>, Option<Uuid>)>(
                    "SELECT attempt_number, labels, replayed_from_execution_id \
                     FROM workflow.executions WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("snapshot")
            }
        };
        let before = snapshot(replayed).await;
        assert_eq!(before.2, Some(old), "born as a replay of the old execution");
        finish_success(&pool, live).await;
        egret_workflow::worker::concurrency::concurrency_sweep(&pool)
            .await
            .expect("sweep");
        assert_eq!(
            state(&pool, replayed).await,
            "RUNNING",
            "the drain resumed it"
        );
        assert!(
            run_count(&pool, replayed).await > 0,
            "its first settle made runs"
        );
        assert_eq!(
            snapshot(replayed).await,
            before,
            "attempt/labels/origin kept"
        );
    } else {
        assert_eq!(history(&pool, replayed).await, json!(["CREATED", terminal]));
        assert!(
            has_ended_at(&pool, replayed).await,
            "a terminal start stamps ended_at"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_whole_replay_over_the_limit_is_queued(pool: sqlx::PgPool) {
    whole_replay_over_limit(pool, "QUEUE", "QUEUED").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_whole_replay_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    whole_replay_over_limit(pool, "CANCEL", "CANCELLED").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_whole_replay_over_a_fail_limit_is_failed(pool: sqlx::PgPool) {
    whole_replay_over_limit(pool, "FAIL", "FAILED").await;
}

// A copy (partial replay from a task) over a full limit WAS ungated through
// Task 3 (soft overshoot) and is now gated by Task 4: see
// `a_partial_replay_over_the_limit_is_queued_then_resumes` and
// `a_partial_replay_over_a_cancel_limit_is_cancelled` below.

// ---- Task 3 (8c): the subflow child is gated on the CHILD flow's limit -----

const PARENT_SRC: &str = "id: parent\nnamespace: t\ntasks:\n  - id: s\n    \
     type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n    wait: true\n";

fn child_src(behavior: &str) -> String {
    format!(
        "id: child\nnamespace: t\nconcurrency:\n  limit: 1\n  behavior: {behavior}\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

/// File a second flow in an existing workspace.
async fn add_flow(pool: &sqlx::PgPool, ws: Uuid, src: &str) -> Uuid {
    let user: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("owner");
    let flow = egret_workflow::spec::parse(src).expect("parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: src.to_string(),
        plan: serde_json::to_value(&flow).expect("serialises"),
        folder_id: None,
    };
    egret_workflow::FlowRepo::new(Db { pool: pool.clone() })
        .create(ws, egret_core::UserId(user), &input)
        .await
        .expect("file the flow")
        .id
}

/// Child flow at its limit (one live execution), then the parent runs.
/// Returns (ws, child_flow, holder, parent, child).
async fn parent_over_full_child(
    pool: &sqlx::PgPool,
    behavior: &str,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let csrc = child_src(behavior);
    let (ws, child_flow) = setup(pool, &csrc).await;
    let hold = start(pool, ws, Some(child_flow), &csrc).await;
    let parent_flow = add_flow(pool, ws, PARENT_SRC).await;
    // The Subflow spawns its child inside the parent's first settle; no tick,
    // which would also finish the holder and free the slot.
    let parent = start(pool, ws, Some(parent_flow), PARENT_SRC).await;
    let child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND trigger_execution_id = $2",
    )
    .bind(child_flow)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("the child exists");
    (ws, child_flow, hold, parent, child)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_over_the_childs_limit_is_queued(pool: sqlx::PgPool) {
    // Act
    let (_ws, _cf, _hold, parent, child) = parent_over_full_child(&pool, "QUEUE").await;

    // Assert
    assert_eq!(state(&pool, child).await, "QUEUED");
    assert_eq!(history(&pool, child).await, json!(["CREATED", "QUEUED"]));
    assert_eq!(
        run_count(&pool, child).await,
        0,
        "a queued child runs no task"
    );
    assert_eq!(
        state(&pool, parent).await,
        "RUNNING",
        "the parent keeps waiting on a non-terminal child"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_queued_subflow_child_runs_when_the_slot_frees_and_the_parent_completes(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (_ws, _cf, hold, parent, child) = parent_over_full_child(&pool, "QUEUE").await;

    // Act: the holder finishes through the worker, which drains the child.
    tick(&pool, 1).await;
    tick(&pool, 5).await;

    // Assert
    assert_eq!(state(&pool, hold).await, "SUCCESS");
    assert_eq!(state(&pool, child).await, "SUCCESS");
    assert_eq!(state(&pool, parent).await, "SUCCESS");
}

async fn subflow_child_terminal(
    pool: sqlx::PgPool,
    behavior: &str,
    terminal: &str,
    expected_parent: &str,
) {
    // Act
    let (_ws, _cf, _hold, parent, child) = parent_over_full_child(&pool, behavior).await;

    // Assert
    assert_eq!(state(&pool, child).await, terminal);
    assert_eq!(
        run_count(&pool, child).await,
        0,
        "a terminal child runs no task"
    );
    // Pins the parent's reaction to the terminal child.
    assert_eq!(
        state(&pool, parent).await,
        expected_parent,
        "parent outcome over a {terminal} child"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    // Pins existing Subflow behaviour: a CANCELLED child does not fail the
    // parent. A later change to that is noticed here.
    subflow_child_terminal(pool, "CANCEL", "CANCELLED", "SUCCESS").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_over_a_fail_limit_is_failed(pool: sqlx::PgPool) {
    // A FAILED child propagates to the parent (transmitFailed).
    subflow_child_terminal(pool, "FAIL", "FAILED", "FAILED").await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_under_the_limit_runs(pool: sqlx::PgPool) {
    // Arrange: nothing holds the child's slot.
    let csrc = child_src("QUEUE");
    let (ws, child_flow) = setup(&pool, &csrc).await;
    let parent_flow = add_flow(&pool, ws, PARENT_SRC).await;
    let parent = start(&pool, ws, Some(parent_flow), PARENT_SRC).await;

    // Act
    tick(&pool, 3).await;

    // Assert
    let child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND trigger_execution_id = $2",
    )
    .bind(child_flow)
    .bind(parent)
    .fetch_one(&pool)
    .await
    .expect("child");
    assert_ne!(state(&pool, child).await, "QUEUED");
    assert_eq!(state(&pool, parent).await, "SUCCESS");
}

// ---- Task 4 (8c): the walk-based starts (restart-in-place, partial replay) --

/// Drive `n` task runs through the REAL worker, FAILING every task, so a
/// one-task execution reaches `FAILED` through the real `settle`.
async fn tick_failing(pool: &sqlx::PgPool, n: usize) {
    egret_workflow::worker::tick(pool, n, &common::Answering(false))
        .await
        .expect("tick");
}

/// How many of `id`'s task runs are `RUNNING` — the "not claimable" invariant
/// (the worker's claim query only ever takes `RUNNING`/`KILLING` runs).
async fn running_run_count(pool: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND state = 'RUNNING'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("running runs")
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_in_place_over_the_limit_is_queued_then_resumes(pool: sqlx::PgPool) {
    // Arrange: limit 1. A FAILED execution to restart; a live one holding the slot.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick_failing(&pool, 3).await;
    assert_eq!(state(&pool, failed).await, "FAILED");
    let live = start(&pool, ws, Some(flow), &src).await; // holds the slot
    assert_eq!(state(&pool, live).await, "RUNNING");

    // Act: restart the failed execution while over the limit.
    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();

    // Then: it is QUEUED (not RESTARTED/RUNNING); the history records the
    // RESTARTED it would have taken, and no run is RUNNING (the reset run is
    // CREATED, which the claim query never takes — the safety invariant).
    assert_eq!(state(&pool, failed).await, "QUEUED");
    let h = history(&pool, failed).await;
    let arr = h.as_array().unwrap();
    assert_eq!(arr.last().unwrap(), "QUEUED");
    assert!(arr.iter().any(|s| s == "RESTARTED"), "history was {h}");
    assert_eq!(
        running_run_count(&pool, failed).await,
        0,
        "a QUEUED restart leaves no RUNNING run"
    );

    // And: freeing the slot drains it back to a real restart.
    finish_success(&pool, live).await;
    egret_workflow::worker::concurrency::concurrency_sweep(&pool)
        .await
        .unwrap();
    assert_ne!(state(&pool, failed).await, "QUEUED");
    let h = history(&pool, failed).await;
    assert!(
        h.as_array().unwrap().iter().any(|s| s == "RESTARTED"),
        "the pop kept RESTARTED, got {h}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_in_place_over_a_fail_limit_is_failed(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "FAIL")));
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick_failing(&pool, 3).await;
    assert_eq!(state(&pool, failed).await, "FAILED");
    let _live = start(&pool, ws, Some(flow), &src).await;
    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();
    // Re-failed by the gate, through settle (no drive).
    assert_eq!(state(&pool, failed).await, "FAILED");
    assert_eq!(
        running_run_count(&pool, failed).await,
        0,
        "a FAILED restart drives nothing"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_in_place_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "CANCEL")));
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick_failing(&pool, 3).await;
    assert_eq!(state(&pool, failed).await, "FAILED");
    let _live = start(&pool, ws, Some(flow), &src).await;
    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();
    assert_eq!(state(&pool, failed).await, "CANCELLED");
    assert_eq!(running_run_count(&pool, failed).await, 0);
}

/// A restart in place of a FAILED execution UNDER the limit still runs
/// (the gate does not queue when a slot is free).
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_in_place_under_the_limit_runs(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick_failing(&pool, 3).await;
    assert_eq!(state(&pool, failed).await, "FAILED");

    // No live holder: the slot is free, so the restart drives.
    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();
    assert_ne!(state(&pool, failed).await, "QUEUED");
    assert!(
        running_run_count(&pool, failed).await > 0 || state(&pool, failed).await == "SUCCESS",
        "the restart drove its reset run"
    );
}

/// A partial replay (from a task run of a finished execution) over the limit is
/// QUEUED with its copied prefix, then drained to a correct resume.
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_replay_over_the_limit_is_queued_then_resumes(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let old = start(&pool, ws, Some(flow), &src).await;
    tick(&pool, 1).await;
    assert_eq!(state(&pool, old).await, "SUCCESS");
    let run_a: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(old)
    .fetch_one(&pool)
    .await
    .expect("run a");
    let live = start(&pool, ws, Some(flow), &src).await; // holds the slot
    assert_eq!(state(&pool, live).await, "RUNNING");
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&pool)
        .await
        .expect("owner");

    // Act: replay from task a while over the limit.
    let new = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: old,
            source: &src,
            kind: egret_workflow::worker::ReplayKind::FromTaskRun(run_a),
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: owner,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");

    // Then: QUEUED with the copied prefix present, RESTARTED recorded, no run
    // RUNNING.
    assert_eq!(state(&pool, new).await, "QUEUED");
    let h = history(&pool, new).await;
    let arr = h.as_array().unwrap();
    assert_eq!(arr.last().unwrap(), "QUEUED");
    assert!(arr.iter().any(|s| s == "RESTARTED"), "history was {h}");
    assert!(
        run_count(&pool, new).await > 0,
        "the copied runs exist for the drain to resume"
    );
    assert_eq!(
        running_run_count(&pool, new).await,
        0,
        "a QUEUED partial replay leaves no RUNNING run"
    );

    // And: freeing the slot drains it to a resume that completes.
    finish_success(&pool, live).await;
    egret_workflow::worker::concurrency::concurrency_sweep(&pool)
        .await
        .unwrap();
    assert_ne!(state(&pool, new).await, "QUEUED");
    tick(&pool, 5).await;
    assert_eq!(state(&pool, new).await, "SUCCESS");
}

/// A partial replay over a CANCEL limit terminalizes through settle.
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_replay_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "CANCEL")));
    let (ws, flow) = setup(&pool, &src).await;
    let old = start(&pool, ws, Some(flow), &src).await;
    tick(&pool, 1).await;
    let run_a: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(old)
    .fetch_one(&pool)
    .await
    .expect("run a");
    let _live = start(&pool, ws, Some(flow), &src).await;
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&pool)
        .await
        .expect("owner");

    let new = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: old,
            source: &src,
            kind: egret_workflow::worker::ReplayKind::FromTaskRun(run_a),
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: owner,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");

    assert_eq!(state(&pool, new).await, "CANCELLED");
    assert_eq!(running_run_count(&pool, new).await, 0);
}

/// A `concurrency: {limit, behavior: QUEUE}` flow whose single `Loop` iteration
/// fails.
fn loop_src(behavior: &str) -> String {
    format!(
        "id: lc\nnamespace: t\nconcurrency:\n  limit: 1\n  behavior: {behavior}\n\
         tasks:\n  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"1\"]'\n    \
         tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: x\n"
    )
}

/// The Task-4 review finding: a restart that reopens a `Loop` (D8 iteration
/// restarts, which live in `restart::open`) must NOT be queued over a QUEUE
/// limit. The drain resumes via `promote_queued`'s plain settle, which never
/// re-enters `open`, so a queued-then-drained loop restart would reopen the Loop
/// run WITHOUT restarting its failing iteration — a silently-wrong result.
/// Instead it SOFT-OVERSHOOTS (runs now, driving the iteration restart), which
/// spec §9 sanctions for this rare intractable sub-case.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_reopening_restart_over_the_limit_soft_overshoots(pool: sqlx::PgPool) {
    let src = loop_src("QUEUE");
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    for _ in 0..30 {
        if state(&pool, failed).await == "FAILED" {
            break;
        }
        tick_failing(&pool, 8).await;
    }
    assert_eq!(state(&pool, failed).await, "FAILED");
    // The failing iteration row (a Loop sub-execution).
    let iter: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 \
           AND loop_index IS NOT NULL ORDER BY loop_index DESC LIMIT 1",
    )
    .bind(failed)
    .fetch_one(&pool)
    .await
    .expect("the failing iteration");
    assert_eq!(state(&pool, iter).await, "FAILED");

    // A live holder occupies the flow's only slot.
    let _live = start(&pool, ws, Some(flow), &src).await;

    // Act: restart the loop execution while over the limit.
    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();

    // Then: soft-overshoot — it ran now (not left QUEUED) and its failing
    // iteration was restarted in place (D8), not silently skipped.
    assert_ne!(
        state(&pool, failed).await,
        "QUEUED",
        "a loop-reopening restart soft-overshoots instead of queueing"
    );
    let ih = history(&pool, iter).await;
    assert!(
        ih.as_array().unwrap().iter().any(|s| s == "RESTARTED"),
        "the failing iteration was restarted in place, got {ih}"
    );
}

/// A loop-reopening restart over a CANCEL limit still terminalizes (a cancelled
/// restart restarts nothing, so the D8 concern does not apply).
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_reopening_restart_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    let src = loop_src("CANCEL");
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    for _ in 0..30 {
        if state(&pool, failed).await == "FAILED" {
            break;
        }
        tick_failing(&pool, 8).await;
    }
    assert_eq!(state(&pool, failed).await, "FAILED");
    let _live = start(&pool, ws, Some(flow), &src).await;

    egret_workflow::worker::restart_execution(&pool, failed)
        .await
        .unwrap();
    assert_eq!(state(&pool, failed).await, "CANCELLED");
}
