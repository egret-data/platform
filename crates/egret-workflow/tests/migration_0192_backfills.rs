//! Migration `0192_workflow_trigger_backfills.sql` on a database that already
//! has executions (W7f §3.2-§3.3): the rows it finds get `backfill_id` and
//! `labels` NULL, and the per-tick index it rebuilds still refuses a second
//! regular execution for a tick that ran before the migration.
//!
//! The usual fixture (`common::db()`) is migrated to HEAD, 0192 included, so
//! it cannot hold a row written before 0192. This builds a private database
//! with every migration strictly BEFORE 0192 applied, writes rows, then runs
//! 0192's body (as `egret-catalog-registry`'s `migration_0151_refuses_unmapped_rows`).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::str::FromStr;

use egret_metastore::MIGRATOR;
use sqlx::{ConnectOptions, Connection};
use uuid::Uuid;

const VERSION: i64 = 192;
const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";

/// DROP+CREATE the private database `name`, then apply every migration with
/// `version < 192`.
async fn db_before_0192(name: &str) -> Option<sqlx::PgPool> {
    let dsn = egret_test_db::url().await?;
    let base = sqlx::postgres::PgConnectOptions::from_str(&dsn).expect("parse DATABASE_URL");
    let mut admin = base.clone().connect().await.expect("connect");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = '{name}' AND pid <> pg_backend_pid()"
    )))
    .execute(&mut admin)
    .await
    .ok();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {name}"
    )))
    .execute(&mut admin)
    .await
    .expect("drop the previous run's database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut admin)
        .await
        .expect("create the private database");
    admin.close().await.ok();

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(base.database(name))
        .await
        .expect("connect the private database");
    for m in MIGRATOR.migrations.iter().filter(|m| m.version < VERSION) {
        sqlx::raw_sql(m.sql.clone())
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("migration {} failed to apply: {e}", m.version));
    }
    Some(pool)
}

/// An execution written with only the columns that exist before 0192. A
/// schedule fire when `tick` is `Some`, an API-shaped row otherwise.
fn insert_execution(
    ws: Uuid,
    flow: Uuid,
    tick: Option<&str>,
) -> sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments> {
    let record = tick.map(|_| serde_json::json!({"id": "s", "type": SCHEDULE, "variables": {}}));
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger, scheduled_date) \
         VALUES ($1, $2, 'w7f', 'm0192', 'id: m0192', 'CREATED', '[\"CREATED\"]'::jsonb, \
                 '{}'::jsonb, $3, $4::timestamptz)",
    )
    .bind(ws)
    .bind(flow)
    .bind(record)
    .bind(tick.map(str::to_owned))
}

#[tokio::test]
async fn migration_0192_keeps_existing_executions_and_their_per_tick_uniqueness() {
    let Some(pool) = db_before_0192("egret_test_migration_0192").await else {
        eprintln!("SKIPPED, asserted nothing -- no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };

    // Arrange: a flow with a fired tick and an API-shaped execution, written
    // before 0192 exists.
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
    let flow: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan, created_by) \
         VALUES ($1, 'w7f', 'm0192', 'id: m0192', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(ws)
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("seed flow");
    let tick = "2026-10-01T00:00:00Z";
    insert_execution(ws, flow, Some(tick))
        .execute(&pool)
        .await
        .expect("a fired tick before 0192");
    insert_execution(ws, flow, None)
        .execute(&pool)
        .await
        .expect("an API-shaped execution before 0192");

    // Act: run 0192's body.
    let m = MIGRATOR
        .migrations
        .iter()
        .find(|m| m.version == VERSION)
        .expect("0192 is embedded");
    sqlx::raw_sql(m.sql.clone())
        .execute(&pool)
        .await
        .expect("0192 applies over existing executions");

    // Assert: the rows it found carry neither column.
    let rows: Vec<(Option<Uuid>, Option<serde_json::Value>)> =
        sqlx::query_as("SELECT backfill_id, labels FROM workflow.executions WHERE flow_id = $1")
            .bind(flow)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(None, None), (None, None)]);

    // The rebuilt index still refuses a second regular execution for the
    // tick that ran before the migration...
    let err = insert_execution(ws, flow, Some(tick))
        .execute(&pool)
        .await
        .expect_err("a second regular execution for a pre-0192 tick");
    let d = err.as_database_error().expect("a database error");
    assert_eq!(d.code().as_deref(), Some("23505"));
    assert_eq!(
        d.constraint(),
        Some("workflow_executions_one_per_schedule_tick")
    );
    // ...and a backfill execution for it is accepted (D2, the user's
    // decision: a backfill re-runs dates that already ran).
    sqlx::query(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger, scheduled_date, backfill_id) \
         SELECT workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
                execution_trigger, scheduled_date, gen_random_uuid() \
           FROM workflow.executions WHERE flow_id = $1 AND scheduled_date IS NOT NULL",
    )
    .bind(flow)
    .execute(&pool)
    .await
    .expect("a backfill execution for a pre-0192 tick");

    pool.close().await;
}
