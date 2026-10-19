//! Migration 0197: `working_dir_swept_at` only on a WorkingDirectory block's run.
//!
//! DB-gated through `#[sqlx::test]`, which resolves `DATABASE_URL` itself.

use uuid::Uuid;

/// One execution and one run that is not a block (`working_dir` NULL).
async fn seed_one_run(pool: &sqlx::PgPool) -> Uuid {
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
    let execution: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, inputs) \
         VALUES ($1, 'w', 'm0197', 'id: m0197', 'CREATED', '[\"CREATED\"]'::jsonb, '{}'::jsonb) \
         RETURNING id",
    )
    .bind(ws)
    .fetch_one(pool)
    .await
    .expect("seed execution");
    sqlx::query_scalar(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, 0, 0, 'a', 'io.kestra.plugin.core.debug.Return', 'CREATED', \
                 '[\"CREATED\"]'::jsonb) RETURNING id",
    )
    .bind(execution)
    .fetch_one(pool)
    .await
    .expect("seed task run")
}

#[sqlx::test(migrations = "../../migrations")]
async fn swept_at_is_only_for_a_block_run(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: a run that is not a block.
    let run = seed_one_run(&pool).await;
    // Act
    let err =
        sqlx::query("UPDATE workflow.task_runs SET working_dir_swept_at = now() WHERE id = $1")
            .bind(run)
            .execute(&pool)
            .await
            .expect_err("not a block");
    // Assert
    assert!(
        err.to_string()
            .contains("workflow_task_runs_working_dir_swept_on_block"),
        "{err}"
    );
    // A block's run takes it.
    sqlx::query(
        "UPDATE workflow.task_runs SET working_dir = 'FINISHED', working_dir_swept_at = now() \
          WHERE id = $1",
    )
    .bind(run)
    .execute(&pool)
    .await
    .expect("a block's run is stamped");
}
