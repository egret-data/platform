//! Migration `0151_drop_legacy_engine_catalogs.sql` refuses (`RAISE
//! EXCEPTION`) rather than drops when a legacy `engine_mgmt.engine_catalogs`
//! row has no mapping in `registry.legacy_engine_catalogs` — i.e. a row the
//! (now-deleted) boot backfill never migrated.
//!
//! This cannot use the normal `egret_test_db::url()` fixture: that database is
//! migrated all the way to HEAD, including 0151 itself, which drops the very
//! tables this test needs to seed. So it builds its own private database with
//! every migration strictly BEFORE 0151 applied, then runs 0151's body
//! directly.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::str::FromStr;

use egret_metastore::MIGRATOR;
use sqlx::{ConnectOptions, Connection};
use uuid::Uuid;

/// DROP+CREATE a private database (isolation by database, not `search_path`:
/// the seam migrations `ALTER TABLE … SET SCHEMA` into shared schema names a
/// `search_path` pin cannot rename — see `setup_isolated_schema` in
/// `egret-controlplane/tests/routes_api.rs`), then apply every migration with
/// `version < 151`. Returns `None` only when `DATABASE_URL` is unset AND
/// `EGRET_ALLOW_DB_SKIP` allows skipping (`egret_test_db::url()` panics
/// otherwise, per this workspace's DB-gated-test convention).
async fn db_before_0151(name: &str) -> Option<sqlx::PgPool> {
    let dsn = egret_test_db::url().await?;
    let base_opts = sqlx::postgres::PgConnectOptions::from_str(&dsn).expect("parse DATABASE_URL");

    let mut admin = base_opts
        .clone()
        .connect()
        .await
        .expect("connect for isolated-db setup");
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
    .expect("drop pre-existing isolated db");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut admin)
        .await
        .expect("create isolated db");
    admin.close().await.ok();

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect_with(base_opts.database(name))
        .await
        .expect("connect isolated db pool");

    // NOT `Db::migrate()` — that runs the full `MIGRATOR`, 0151 included, which
    // is exactly the migration this fixture must stay behind.
    for m in MIGRATOR.migrations.iter().filter(|m| m.version < 151) {
        sqlx::raw_sql(m.sql.clone())
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("migration {} failed to apply: {e}", m.version));
    }
    Some(pool)
}

#[tokio::test]
async fn migration_0151_refuses_unmapped_rows() {
    let Some(pool) = db_before_0151("egret_test_migration_0151").await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };

    // Arrange: a warehouse, an engine, and a legacy `engine_catalogs` row with
    // NO row in `registry.legacy_engine_catalogs` mapping it — the boot
    // backfill never ran for it, or ran and errored before writing the map.
    let ws_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $1, 'active') RETURNING id",
    )
    .bind(format!("m0151-{}", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("seed workspace");

    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .bind(format!("m0151-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("seed storage connection");

    let wh_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("m0151-wh-{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("seed warehouse");

    let eng_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id",
    )
    .bind(format!("m0151-eng-{}", Uuid::new_v4().simple()))
    .bind(wh_id)
    .fetch_one(&pool)
    .await
    .expect("seed engine");

    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', '{}'::jsonb, true)",
    )
    .bind(eng_id)
    .execute(&pool)
    .await
    .expect("seed unmapped legacy catalog row");

    // Positive control: `registry.legacy_engine_catalogs` really is empty here
    // (nothing populates it before 0147's boot backfill runs, and this
    // database never boots egret) — the row above is unmapped because there
    // is genuinely no map, not because the query that would find it is wrong.
    let mapped: i64 = sqlx::query_scalar("SELECT count(*) FROM registry.legacy_engine_catalogs")
        .fetch_one(&pool)
        .await
        .expect("read legacy_engine_catalogs");
    assert_eq!(mapped, 0, "positive control: no row is mapped yet");

    // Act: run 0151's body against this pre-0151 database.
    let m0151 = MIGRATOR
        .migrations
        .iter()
        .find(|m| m.version == 151)
        .expect("0151 is embedded — every migration on disk is embedded (build.rs)");
    let result = sqlx::raw_sql(m0151.sql.clone()).execute(&pool).await;

    // Assert: it raises, and the message names the SP3c-1 hop. `is_err()`
    // alone cannot distinguish this from a syntax error in the `DO` block —
    // both produce an `Err` — so the assertion is on the message text.
    let err = result.expect_err("an unmapped legacy row must raise, not silently drop");
    let msg = err.to_string();
    assert!(
        msg.contains("SP3c-1"),
        "expected the refusal to name the SP3c-1 hop, got: {msg}"
    );
    assert!(
        msg.contains("engine_mgmt.engine_catalogs"),
        "expected the refusal to name the table it found unmigrated rows in, got: {msg}"
    );

    // `raw_sql` sends the whole file (the `DO` block and the three `DROP
    // TABLE`s after it) as one implicit transaction; the `RAISE EXCEPTION`
    // aborts it, so nothing after it committed either. The row — and the
    // table — must still be there.
    let still_present: i64 = sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engine_catalogs")
        .fetch_one(&pool)
        .await
        .expect("table must survive a refused migration");
    assert_eq!(
        still_present, 1,
        "the refusal must not drop the row (or the table) it refused over"
    );
}
