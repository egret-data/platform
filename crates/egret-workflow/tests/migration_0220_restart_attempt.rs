//! Migration 0220 (workflow package 7a, spec D7 and D12): a restart in place
//! raises `attempt_number` on any execution, so 0139's
//! `workflow_executions_metadata_belongs_to_a_replay` no longer ties
//! `attempt_number > 1` to the replay shape. `origin_offset_ms` stays the
//! replay's alone.
//!
//! DB-gated through `#[sqlx::test]`, which resolves `DATABASE_URL` itself.

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

/// Insert a top-level execution (no parent) with the given metadata.
async fn insert_top_level(
    pool: &sqlx::PgPool,
    ws: Uuid,
    attempt_number: i32,
    origin_offset_ms: i64,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, inputs, \
            attempt_number, origin_offset_ms) \
         VALUES ($1, 'w', 'm0220', 'id: m0220', 'FAILED', '[\"FAILED\"]'::jsonb, '{}'::jsonb, \
                 $2, $3) \
         RETURNING id",
    )
    .bind(ws)
    .bind(attempt_number)
    .bind(origin_offset_ms)
    .fetch_one(pool)
    .await
}

fn check_violation(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .filter(|d| d.code().as_deref() == Some("23514"))
        .and_then(|d| d.constraint().map(str::to_string))
}

/// A restarted top-level execution holds `attempt_number = 3` with no replay
/// shape and `origin_offset_ms = 0`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_top_level_row_may_hold_a_raised_attempt_number(pool: sqlx::PgPool) {
    let ws = seed_workspace(&pool).await;

    let id = insert_top_level(&pool, ws, 3, 0).await;

    let id = id.expect("a top-level execution restarted twice is a legal row");
    let (n, off): (i32, i64) = sqlx::query_as(
        "SELECT attempt_number, origin_offset_ms FROM workflow.executions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((n, off), (3, 0));
}

/// The UPDATE a restart issues is accepted on a plain execution.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_raises_attempt_number_on_a_plain_execution(pool: sqlx::PgPool) {
    let ws = seed_workspace(&pool).await;
    let id = insert_top_level(&pool, ws, 1, 0).await.unwrap();

    let raised = sqlx::query(
        "UPDATE workflow.executions SET attempt_number = attempt_number + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await;

    raised.expect("a restart's attempt_number + 1 is accepted");
}

/// `origin_offset_ms > 0` is still the replay's alone: a top-level row with
/// it is refused, whatever its `attempt_number`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_origin_offset_without_the_replay_shape_is_still_refused(pool: sqlx::PgPool) {
    let ws = seed_workspace(&pool).await;

    for attempt in [1, 2] {
        let err = insert_top_level(&pool, ws, attempt, 5_000)
            .await
            .expect_err("an origin offset outside a replay is refused");
        assert_eq!(
            check_violation(&err).as_deref(),
            Some("workflow_executions_metadata_belongs_to_a_replay"),
            "attempt {attempt}: refused by the relaxed CHECK, got {err}"
        );
    }
}

/// `attempt_number = 0` stays refused (the positive CHECK and the relaxed one).
#[sqlx::test(migrations = "../../migrations")]
async fn attempt_number_zero_is_still_refused(pool: sqlx::PgPool) {
    let ws = seed_workspace(&pool).await;

    let err = insert_top_level(&pool, ws, 0, 0)
        .await
        .expect_err("attempt 0 is refused");

    assert!(check_violation(&err).is_some(), "a CHECK refused it: {err}");
}

/// Insert a run of `execution` in `state` with `history`.
async fn insert_run(pool: &sqlx::PgPool, execution: Uuid, run_ix: i32, state: &str, history: &str) {
    sqlx::query(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, $2, $2, $3, 'io.kestra.plugin.core.log.Log', $4, $5::jsonb)",
    )
    .bind(execution)
    .bind(run_ix)
    .bind(format!("t{run_ix}"))
    .bind(state)
    .bind(history)
    .execute(pool)
    .await
    .expect("seed run");
}

async fn idle(pool: &sqlx::PgPool, execution: Uuid) -> Option<bool> {
    sqlx::query_scalar("SELECT workflow.execution_is_idle($1)")
        .bind(execution)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// An emptied run (`history = []`, state `CREATED`: what a restart leaves
/// for an `errors:` run the walk never reaches again) is not live work, so
/// the terminal execution is idle; a `CREATED` run with a history still is.
#[sqlx::test(migrations = "../../migrations")]
async fn an_emptied_run_does_not_keep_a_terminal_execution_live(pool: sqlx::PgPool) {
    let ws = seed_workspace(&pool).await;
    let emptied = insert_top_level(&pool, ws, 2, 0).await.unwrap();
    insert_run(
        &pool,
        emptied,
        0,
        "SUCCESS",
        r#"["CREATED","RUNNING","SUCCESS"]"#,
    )
    .await;
    insert_run(&pool, emptied, 1, "CREATED", "[]").await;
    let live = insert_top_level(&pool, ws, 1, 0).await.unwrap();
    insert_run(&pool, live, 0, "CREATED", r#"["CREATED"]"#).await;

    assert_eq!(idle(&pool, emptied).await, Some(true));
    assert_eq!(idle(&pool, live).await, Some(false));
}
