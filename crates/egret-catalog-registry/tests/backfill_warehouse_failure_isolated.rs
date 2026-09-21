//! I2 (review fix round 1, warehouse half): a warehouse whose catalog cannot be
//! created must not abort the boot for every OTHER warehouse and engine.
//!
//! `ensure_warehouse_catalog` is deliberately race-safe (a concurrent name or
//! warehouse_id collision is handled, never surfaced as an error), so the only
//! way to make it fail for a warehouse that genuinely exists is a referentially
//! inconsistent row: here, a warehouse whose `workspace_id` does not exist.
//! `catalog.warehouses.workspace_id` is normally FK-enforced, so this is built
//! with `SET LOCAL session_replication_role = replica` (disables constraint
//! triggers for one transaction) — simulating the kind of data corruption
//! (a botched manual fix, a migration bug) this defence exists for, without any
//! timing dependence on a real concurrent deleter.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use uuid::Uuid;

#[tokio::test]
async fn a_warehouse_with_a_dangling_workspace_blocks_only_itself() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = db.pool.clone();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-whfail', 'BF', 'active') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-whfail-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Two GOOD warehouses, valid in every way.
    let good1: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-good-1', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let good2: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-good-2', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    // An engine on good1, so step 2 (legacy rows) is exercised too.
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-whfail-trino', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(good1)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'bench', 'tpch', '{}'::jsonb, false)",
    )
    .bind(engine)
    .execute(&pool)
    .await
    .unwrap();

    // One BAD warehouse: real row, but its workspace_id references nothing.
    // `SET LOCAL` reverts automatically at commit, so the FK is enforced again
    // for every later statement (including run_backfill's own inserts).
    let bogus_workspace = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    let bad: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-bad', $1, $2) RETURNING id",
    )
    .bind(bogus_workspace)
    .bind(sc)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // `session_replication_role` was only ever `SET LOCAL`, scoped to that one
    // transaction — every statement from here on enforces the FK normally, so
    // `bad`'s dangling `workspace_id` is exactly the corruption this test wants:
    // real, persisted, and only discoverable when something tries to insert
    // against it.

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = RegistryStore::new(db.clone());

    let run = run_backfill(&store, &vault)
        .await
        .expect("one bad warehouse must not fail the whole boot");

    // The good warehouses (and the engine's row on good1) migrated normally.
    let cat1 = store
        .get_catalog_by_name("bf_good_1")
        .await
        .unwrap()
        .expect("good1 migrates despite bad's failure");
    assert_eq!(cat1.warehouse_id, Some(good1));
    let cat2 = store
        .get_catalog_by_name("bf_good_2")
        .await
        .unwrap()
        .expect("good2 migrates despite bad's failure");
    assert_eq!(cat2.warehouse_id, Some(good2));
    store
        .get_catalog_by_name("bench")
        .await
        .unwrap()
        .expect("the legacy row on good1's engine still migrates");
    assert_eq!(
        store.unmigrated_legacy_catalog_count(engine).await.unwrap(),
        0
    );

    // The bad warehouse got no catalog and is reported, not silently dropped.
    assert!(
        store
            .get_catalog_for_warehouse(bad)
            .await
            .unwrap()
            .is_none(),
        "a referentially broken warehouse must not end up with a catalog"
    );
    assert!(run.failed_rows >= 1);
    let failed: Vec<(String, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT code, warehouse_id, message FROM registry.migration_report \
          WHERE run_id = $1 AND code = 'backfill_failed'",
    )
    .bind(run.run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        failed.iter().any(|(_, wh, _)| *wh == Some(bad)),
        "the bad warehouse's own id is on its failure report: {failed:?}"
    );

    // A second boot: the two good warehouses are not re-migrated (idempotent),
    // and the bad one is retried and still fails the same way, not silently
    // abandoned.
    let second = run_backfill(&store, &vault).await.expect("second boot");
    assert_eq!(
        second.report_rows, 1,
        "only the still-broken warehouse's failure is reported again; the two \
         good ones and bench are not re-migrated or re-reported"
    );
    let second_failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.migration_report WHERE run_id = $1 AND code = 'backfill_failed'",
    )
    .bind(second.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        second_failed, 1,
        "the still-broken warehouse is retried and reported again on the next boot"
    );
    let total_catalogs: i64 = sqlx::query_scalar("SELECT count(*) FROM registry.catalogs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total_catalogs, 3, "still exactly good1, good2, bench");
}
