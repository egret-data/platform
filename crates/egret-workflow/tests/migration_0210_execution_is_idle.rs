//! Migration 0210: `workflow.execution_is_idle(id)` (package #2b, spec D13
//! and §6), and the orphan sweep's superseded rule that reads it.
//!
//! An execution is idle when it is terminal AND no task run of it is live.
//! A terminal execution can still have live work: an `Exit` sibling, and
//! `afterExecution:` since package #2b. The sweeps ask "idle", not
//! "terminal".
//!
//! DB-gated through `#[sqlx::test]`, which resolves `DATABASE_URL` itself.

use std::time::Duration;

use uuid::Uuid;

/// A workspace to hang executions on.
async fn seed_workspace(pool: &sqlx::PgPool) -> Uuid {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .expect("seed user");
    let ws = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 't', $3)")
        .bind(ws)
        .bind(ws.to_string())
        .bind(user)
        .execute(pool)
        .await
        .expect("seed workspace");
    ws
}

/// An execution in `state`; a terminal one ended `ended_hours_ago` hours ago.
async fn seed_execution(pool: &sqlx::PgPool, ws: Uuid, state: &str, ended_hours_ago: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, inputs, ended_at, \
            updated_at) \
         VALUES ($1, 'w', 'm0210', 'id: m0210', $2, jsonb_build_array($2::text), '{}'::jsonb, \
                 CASE WHEN workflow.state_is_terminal($2) \
                      THEN now() - make_interval(hours => $3) END, \
                 now() - make_interval(hours => $3)) \
         RETURNING id",
    )
    .bind(ws)
    .bind(state)
    .bind(ended_hours_ago)
    .fetch_one(pool)
    .await
    .expect("seed execution")
}

/// A run of `execution` in `state`; a terminal one ended `ended_hours_ago`
/// hours ago.
async fn seed_run(
    pool: &sqlx::PgPool,
    execution: Uuid,
    run_ix: i32,
    state: &str,
    ended_hours_ago: i32,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history, ended_at) \
         VALUES ($1, $2, $2, $3, 'io.kestra.plugin.core.debug.Return', $4, \
                 jsonb_build_array($4::text), \
                 CASE WHEN workflow.state_is_terminal($4) \
                      THEN now() - make_interval(hours => $5) END) \
         RETURNING id",
    )
    .bind(execution)
    .bind(run_ix)
    .bind(format!("t{run_ix}"))
    .bind(state)
    .bind(ended_hours_ago)
    .fetch_one(pool)
    .await
    .expect("seed task run")
}

async fn idle(pool: &sqlx::PgPool, execution: Uuid) -> Option<bool> {
    sqlx::query_scalar("SELECT workflow.execution_is_idle($1)")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("execution_is_idle")
}

#[sqlx::test(migrations = "../../migrations")]
async fn execution_is_idle_is_false_while_a_run_is_live_after_the_end(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a terminal execution whose after-run is still RUNNING.
    let ws = seed_workspace(&pool).await;
    let ex = seed_execution(&pool, ws, "FAILED", 1).await;
    seed_run(&pool, ex, 0, "FAILED", 1).await;
    seed_run(&pool, ex, 1, "RUNNING", 0).await;
    // Act
    let answer = idle(&pool, ex).await;
    // Assert
    assert_eq!(answer, Some(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn execution_is_idle_is_true_when_terminal_and_every_run_terminal(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a terminal execution, every run terminal (KILLED included).
    let ws = seed_workspace(&pool).await;
    let ex = seed_execution(&pool, ws, "SUCCESS", 1).await;
    seed_run(&pool, ex, 0, "SUCCESS", 1).await;
    seed_run(&pool, ex, 1, "KILLED", 0).await;
    // And one with no run at all.
    let bare = seed_execution(&pool, ws, "CANCELLED", 1).await;
    // Act
    let answers = (idle(&pool, ex).await, idle(&pool, bare).await);
    // Assert
    assert_eq!(answers, (Some(true), Some(true)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn execution_is_idle_is_false_while_running(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a running execution whose runs are all terminal so far, and a
    // KILLING one (live: the kill is not over).
    let ws = seed_workspace(&pool).await;
    let ex = seed_execution(&pool, ws, "RUNNING", 0).await;
    seed_run(&pool, ex, 0, "SUCCESS", 0).await;
    let killing = seed_execution(&pool, ws, "KILLING", 0).await;
    // Act
    let answers = (idle(&pool, ex).await, idle(&pool, killing).await);
    // Assert
    assert_eq!(answers, (Some(false), Some(false)));
}

/// Spec §6: a container whose run has no row (a `LoopUntil` iteration D12
/// renamed) is superseded only when its execution is IDLE. While an
/// `afterExecution:` run is live the execution is terminal but not idle, so
/// the container is not judged at all: a superseded iteration inside
/// `afterExecution:` keeps its container while the phase runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_superseded_kept_container_is_not_judged_while_after_execution_runs(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: terminal 10 h ago, an after-run live; a container of a run id
    // no row holds, labelled with that execution.
    let ws = seed_workspace(&pool).await;
    let ex = seed_execution(&pool, ws, "SUCCESS", 10).await;
    seed_run(&pool, ex, 0, "SUCCESS", 10).await;
    seed_run(&pool, ex, 1, "RUNNING", 0).await;
    let gone = Uuid::new_v4();
    // Act
    let superseded = egret_workflow::worker::commands::superseded_for_tests(
        &pool,
        &[(gone, Some(ex))],
        Some(Duration::from_secs(3600)),
    )
    .await
    .expect("run_liveness");
    // Assert
    assert!(
        superseded.is_empty(),
        "a terminal execution with a live run is not idle: {superseded:?}"
    );
}

/// Spec §6: a superseded kept container's retention runs from the LAST end
/// of the execution's work, `GREATEST(e.ended_at, max(task_runs.ended_at))`,
/// not from the execution's `ended_at`, which an after-run outlives.
#[sqlx::test(migrations = "../../migrations")]
async fn the_superseded_retention_runs_from_the_last_runs_end(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: the execution ended 10 h ago; its after-run ended just now.
    let ws = seed_workspace(&pool).await;
    let ex = seed_execution(&pool, ws, "SUCCESS", 10).await;
    seed_run(&pool, ex, 0, "SUCCESS", 10).await;
    let after = seed_run(&pool, ex, 1, "SUCCESS", 0).await;
    let last_end: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT ended_at FROM workflow.task_runs WHERE id = $1")
            .bind(after)
            .fetch_one(&pool)
            .await
            .unwrap();
    let gone = Uuid::new_v4();
    // Act: a one-hour retention.
    let superseded = egret_workflow::worker::commands::superseded_for_tests(
        &pool,
        &[(gone, Some(ex))],
        Some(Duration::from_secs(3600)),
    )
    .await
    .expect("run_liveness");
    // Assert
    assert_eq!(
        superseded,
        vec![(gone, last_end, false)],
        "judged from the after-run's end (inside the hour), not the execution's (10 h ago)"
    );
}
