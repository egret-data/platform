//! I1 (review fix round 1): a run that migrates some rows and fails on others
//! (here: no secrets master key, so any row needing to EXTRACT a plaintext
//! secret or a warehouse credential fails) must not lose the report for the
//! rows that DID succeed — each row's mutation and its report commit together,
//! so nothing written before the "crash" (the unconfigured run) is ever lost,
//! and nothing is written twice when the SAME rows are retried on a later boot.
//!
//! I2 (review fix round 1, scan half): a saved-SQL scan that fails (here: the
//! table one of its six queries reads is gone) must not abort the run — the
//! rename stays reported and its `scanPending` marker stays set for a later
//! boot to retry, exactly once, when the table is back.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

fn vault(db: &Db, configured: bool) -> egret_secrets::WorkspaceSecretStore {
    let crypto = configured
        .then(|| egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap());
    egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), crypto),
    )
}

fn store_of(db: &Db) -> RegistryStore {
    RegistryStore::new(db.clone())
}

async fn codes(pool: &sqlx::PgPool, run: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT code FROM registry.migration_report WHERE run_id = $1 ORDER BY id")
        .bind(run)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Every report row across every run, `(code, subject)` — used to prove nothing
/// is duplicated once the retried rows finally succeed.
async fn all_report_rows(pool: &sqlx::PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT code, subject FROM registry.migration_report ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn pending_scan_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM registry.migration_report \
          WHERE code = 'engine_catalog_renamed' AND COALESCE((detail->>'scanPending')::boolean, true)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_run_interrupted_by_a_missing_master_key_loses_nothing_and_duplicates_nothing() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = db.pool.clone();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-partial', 'BF', 'active') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-partial-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-partial-wh', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-partial-trino', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(wh)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Needs NO secret: succeeds even with no master key configured.
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'bench', 'tpch', '{}'::jsonb, false)",
    )
    .bind(engine)
    .execute(&pool)
    .await
    .unwrap();
    // The primary: needs a credential secret, so it FAILS with no master key.
    let client = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine)
    .bind(
        json!({ "warehouse": "bf-partial-wh", "credential": "cid:csecret", "clientRowId": client }),
    )
    .execute(&pool)
    .await
    .unwrap();
    // A plaintext-password row: also needs a secret, so it FAILS too, and (once
    // it succeeds on retry) is renamed `Old-Pg` -> `old_pg`, feeding the scan
    // half of this test below.
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'Old-Pg', 'postgresql', $2, false)",
    )
    .bind(engine)
    .bind(json!({ "properties": {
        "connection-url": "jdbc:postgresql://db:5432/x", "connection-user": "u", "connection-password": "hunter2" } }))
    .execute(&pool)
    .await
    .unwrap();

    // "Boot 1": no secrets master key configured — simulates a real deployment
    // gap (plan decision #31). `bench` migrates; `egret` and `Old-Pg` fail.
    let unconfigured = vault(&db, false);
    let first = run_backfill(&store_of(&db), &unconfigured)
        .await
        .expect("first boot");
    assert_eq!(first.failed_rows, 2, "egret and Old-Pg both need a secret");

    let bench = store_of(&db)
        .get_catalog_by_name("bench")
        .await
        .unwrap()
        .expect("bench migrated on boot 1 despite the other two failing");
    let first_codes = codes(&pool, first.run_id).await;
    assert!(
        first_codes.iter().any(|c| c == "engine_catalog_typed"),
        "{first_codes:?}"
    );
    assert_eq!(
        first_codes
            .iter()
            .filter(|c| c.as_str() == "backfill_failed")
            .count(),
        2,
        "{first_codes:?}"
    );
    // Crucially: bench's report ALREADY exists after boot 1 — not deferred to
    // some later "flush" that a crash between boot 1 and boot 2 could lose.
    let after_boot1 = all_report_rows(&pool).await;
    assert!(
        after_boot1
            .iter()
            .any(|(c, s)| c == "engine_catalog_typed" && s.contains("bench")),
        "{after_boot1:?}"
    );

    // Before "boot 2" (which will migrate `Old-Pg`, renaming it in the SAME run
    // that also runs step 3): seed a saved query naming the OLD (pre-migration)
    // catalog name — `Old-Pg` is not a bare SQL identifier, so a real user would
    // have quoted it — and break one of the six tables `scan_saved_sql` reads,
    // so THIS run's own scan attempt fails — proving a failed scan does not
    // abort the boot (I2).
    sqlx::query(r#"INSERT INTO analytics.saved_queries (workspace_id, name, sql) VALUES ($1, 'q', 'SELECT * FROM "Old-Pg".s.t')"#)
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TABLE analytics.sql_snippets")
        .execute(&pool)
        .await
        .unwrap();

    // "Boot 2": master key now configured. The two failed rows succeed; `bench`
    // (already mapped) is untouched — no re-migration, no duplicate report row.
    // `Old-Pg` renames and its OWN scan attempt (step 3, same run) fails.
    let configured = vault(&db, true);
    let second = run_backfill(&store_of(&db), &configured)
        .await
        .expect("a failed scan must not abort the boot");
    assert_eq!(
        second.failed_rows, 0,
        "the scan failure is not a row failure"
    );

    let store = store_of(&db);
    assert_eq!(
        store.unmigrated_legacy_catalog_count(engine).await.unwrap(),
        0,
        "every legacy row is migrated after boot 2 (the scan failure blocked nothing)"
    );
    store
        .get_catalog_by_name("old_pg")
        .await
        .unwrap()
        .expect("Old-Pg migrated on boot 2 despite its own scan failing");
    assert!(store
        .get_attachment(engine, bench.id)
        .await
        .unwrap()
        .is_some());

    let second_codes = codes(&pool, second.run_id).await;
    assert!(
        second_codes.iter().any(|c| c == "secret_extracted"),
        "{second_codes:?}"
    );
    assert!(
        !second_codes.iter().any(|c| c == "engine_catalog_typed"),
        "bench is not re-reported on boot 2: {second_codes:?}"
    );
    assert!(
        second_codes.iter().any(|c| c == "saved_sql_scan_failed"),
        "{second_codes:?}"
    );
    assert!(
        !second_codes
            .iter()
            .any(|c| c == "saved_sql_references_catalog"),
        "the scan did not complete, so it reports no hits yet: {second_codes:?}"
    );
    assert_eq!(
        pending_scan_count(&pool).await,
        1,
        "the rename stays pending for a later boot to retry"
    );

    // Nothing duplicated across BOTH boots: every (code, subject) pair is
    // unique — bench's entries appear exactly once (from boot 1); egret's and
    // Old-Pg's appear exactly once (from boot 2, not boot 1, since boot 1's
    // failed attempt wrote only `backfill_failed` for them).
    let all_rows = all_report_rows(&pool).await;
    let mut seen = std::collections::HashSet::new();
    for pair in &all_rows {
        assert!(
            seen.insert(pair.clone()),
            "duplicate report row: {pair:?} in {all_rows:?}"
        );
    }

    // Restore the table (as a later boot would find it fixed) and retry.
    sqlx::query(
        "CREATE TABLE analytics.sql_snippets (\
           id uuid PRIMARY KEY DEFAULT gen_random_uuid(), \
           workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, \
           name text NOT NULL, body text NOT NULL, description text NOT NULL DEFAULT '', \
           created_by uuid, created_at timestamptz NOT NULL DEFAULT now(), \
           updated_at timestamptz NOT NULL DEFAULT now())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let third = run_backfill(&store_of(&db), &configured)
        .await
        .expect("third boot");
    let third_codes = codes(&pool, third.run_id).await;
    assert_eq!(
        third_codes
            .iter()
            .filter(|c| c.as_str() == "saved_sql_references_catalog")
            .count(),
        1,
        "{third_codes:?}"
    );
    assert_eq!(
        pending_scan_count(&pool).await,
        0,
        "cleared once the scan actually completes"
    );

    // A fourth, fully no-op boot: the completed scan is never repeated.
    let fourth = run_backfill(&store_of(&db), &configured)
        .await
        .expect("fourth boot");
    assert_eq!((fourth.report_rows, fourth.failed_rows), (0, 0));
    let all_rows_final = all_report_rows(&pool).await;
    assert_eq!(
        all_rows_final
            .iter()
            .filter(|(c, _)| c == "saved_sql_references_catalog")
            .count(),
        1,
        "the scan hit is written exactly once, ever: {all_rows_final:?}"
    );
}
