//! SP3c-1 review F3: the boot backfill and migration 0150 must agree about
//! which attachment is primary.
//!
//! Two populations end up in `registry.engine_catalog_attachments`: rows the
//! SQL migration marked (`catalogs.warehouse_id = engines.warehouse_id`), and
//! rows the Rust backfill writes at a later boot for legacy
//! `engine_mgmt.engine_catalogs` the migration never saw. `list_engine_
//! attachments` orders both by the same column, and Trino's catalog env-var
//! numbering rides on that order — so if the two populations are marked by
//! different rules, an engine migrated on Tuesday renders its catalogs in a
//! different order from one migrated on Wednesday.
//!
//! The obvious rule for the backfill is the legacy `engine_catalogs.is_primary`
//! flag it is reading anyway. It is WRONG, and this is the fixture where the two
//! rules diverge: a legacy row whose flag says NOT primary, but whose warehouse
//! resolves to the engine's own. `row_warehouse` resolves a non-primary row's
//! warehouse independently of the flag (through `clientRowId`, else the
//! `warehouse` name), so the resulting catalog's `warehouse_id` can equal the
//! engine's while the flag says false. Migration 0150 would call that
//! attachment primary; the flag would not.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn the_backfill_marks_primacy_the_way_migration_0150_does() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = &db.pool;

    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'F3', 'active') RETURNING id",
    )
    .bind(format!("f3-{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .bind(format!("f3-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();

    // The engine's OWN warehouse. The legacy row below names it while carrying
    // `is_primary = false` — the whole point of the fixture.
    let wh_name = format!("f3lake{}", &Uuid::new_v4().simple().to_string()[..10]);
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&wh_name)
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .unwrap();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id",
    )
    .bind(format!("f3-trino-{}", Uuid::new_v4().simple()))
    .bind(wh)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, false)",
    )
    .bind(engine)
    .bind(json!({ "warehouse": wh_name, "credential": "cid:csecret" }))
    .execute(pool)
    .await
    .unwrap();

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = RegistryStore::new(db.clone());
    let run = run_backfill(&store, &vault).await.expect("backfill");
    assert_eq!(run.failed_rows, 0, "the row itself migrates fine");

    let listed = store.list_engine_attachments(engine).await.unwrap();
    assert_eq!(listed.len(), 1, "one attachment");
    assert_eq!(
        listed[0].catalog.warehouse_id,
        Some(wh),
        "the catalog really does resolve to the engine's own warehouse — without \
         this the fixture proves nothing"
    );

    // The CONTROL for the whole test: the legacy flag says false. If the
    // backfill copied the flag, the assertion below would read `false` and the
    // two populations would be ordered by different rules.
    let legacy_flag: bool = sqlx::query_scalar(
        "SELECT is_primary FROM engine_mgmt.engine_catalogs WHERE engine_id = $1",
    )
    .bind(engine)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!legacy_flag, "the legacy row is NOT flagged primary");

    assert!(
        listed[0].is_primary(),
        "the backfill must use migration 0150's rule (catalogs.warehouse_id = \
         engines.warehouse_id), not the legacy `engine_catalogs.is_primary` flag"
    );

    // And the two rules really are the same rule, run twice: what 0150's SQL
    // would have written for this row, asked of the database directly.
    let by_migration_rule: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM registry.engine_catalog_attachments a \
             JOIN registry.catalogs c ON c.id = a.catalog_id \
             JOIN engine_mgmt.engines e ON e.id = a.engine_id \
            WHERE a.engine_id = $1 AND c.warehouse_id = e.warehouse_id)",
    )
    .bind(engine)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        by_migration_rule,
        listed[0].is_primary(),
        "the backfill's answer and migration 0150's predicate agree row-for-row"
    );
}
