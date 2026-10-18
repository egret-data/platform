//! Migration 0195: `workflow.executions.variables` holds NULL or a JSON object.
//!
//! DB-gated through `#[sqlx::test]`, which resolves `DATABASE_URL` itself.

use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn variables_takes_null_or_an_object_and_refuses_anything_else(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange: one execution row.
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("seed user");
    let ws = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 't', $3)")
        .bind(ws)
        .bind(ws.to_string())
        .bind(user)
        .execute(&pool)
        .await
        .expect("seed workspace");
    let execution: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, inputs) \
         VALUES ($1, 'w', 'm0195', 'id: m0195', 'CREATED', '[\"CREATED\"]'::jsonb, '{}'::jsonb) \
         RETURNING id",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("seed execution");

    // Act + Assert
    for ok in ["NULL", "'{}'::jsonb", "'{\"a\":{\"b\":[1]}}'::jsonb"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE workflow.executions SET variables = {ok} WHERE id = $1"
        )))
        .bind(execution)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "'[]'::jsonb",
        "'1'::jsonb",
        "'\"s\"'::jsonb",
        "'null'::jsonb",
    ] {
        let err = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE workflow.executions SET variables = {bad} WHERE id = $1"
        )))
        .bind(execution)
        .execute(&pool)
        .await
        .expect_err(bad);
        assert!(
            err.to_string()
                .contains("workflow_executions_variables_is_object"),
            "{bad}: {err}"
        );
    }
}
