//! T1 (review fix round 1): the exact kind-cluster shape — 24 warehouses named
//! like `e2e-wh-<13 digits>` in ONE workspace, exactly ONE engine primary — with
//! EXACT resulting names and EXACT report-row counts per code, not `.any()`.
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

#[tokio::test]
async fn twenty_four_warehouses_one_primary_exact_names_and_report() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = &db.pool;
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-kind', 'BF', 'active') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-kind-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();

    // 24 warehouses, one workspace, names like `e2e-wh-<13 digits>` — none a
    // valid SQL identifier as-is.
    const BASE: u64 = 1_784_500_000_000;
    let mut names = Vec::with_capacity(24);
    let mut ids = Vec::with_capacity(24);
    for i in 0..24u64 {
        let name = format!("e2e-wh-{}", BASE + i);
        assert_eq!(
            name.len(),
            "e2e-wh-".len() + 13,
            "name {name} must carry 13 digits"
        );
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(&name)
        .bind(ws)
        .bind(sc)
        .fetch_one(pool)
        .await
        .unwrap();
        names.push(name);
        ids.push(id);
    }

    // Exactly ONE engine primary, on warehouse index 0.
    let primary_wh = ids[0];
    let primary_wh_name = names[0].clone();
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('demo-trino', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(primary_wh)
    .fetch_one(pool)
    .await
    .unwrap();
    let client = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine)
    .bind(
        json!({ "warehouse": primary_wh_name, "credential": "cid:csecret", "clientRowId": client }),
    )
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
    assert_eq!(run.failed_rows, 0);

    // The primary's catalog is `egret`; the other 23 have their EXACT normalised
    // names, one-to-one with their warehouse.
    let egret = store.get_catalog_by_name("egret").await.unwrap().unwrap();
    assert_eq!(egret.warehouse_id, Some(primary_wh));
    for (name, id) in names.iter().zip(ids.iter()).skip(1) {
        let expected = name.replace('-', "_");
        let cat = store
            .get_catalog_by_name(&expected)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no catalog named {expected} for warehouse {name}"));
        assert_eq!(
            cat.warehouse_id,
            Some(*id),
            "catalog {expected} must belong to warehouse {name}"
        );
    }
    // Exactly 24 catalogs total: no extras, no misses.
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM registry.catalogs")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(total, 24);

    // EXACT report-row counts per code — not `.any()`.
    let rows: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT code, warehouse_id FROM registry.migration_report WHERE run_id = $1 ORDER BY id",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await
    .unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (code, _) in &rows {
        *counts.entry(code.clone()).or_default() += 1;
    }
    let expected: BTreeMap<String, usize> = [
        ("warehouse_catalog_created".to_string(), 24),
        ("primary_named_egret".to_string(), 1),
        ("secret_extracted".to_string(), 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(counts, expected, "full report: {rows:?}");
    assert_eq!(rows.len(), 26, "no row appears twice, none is missing");

    // Which warehouse got `primary_named_egret`: exactly the primary, never one
    // of the 23 others.
    let primary_named_egret_warehouses: Vec<Uuid> = rows
        .iter()
        .filter(|(c, _)| c == "primary_named_egret")
        .map(|(_, wh)| wh.expect("primary_named_egret always names its warehouse"))
        .collect();
    assert_eq!(primary_named_egret_warehouses, vec![primary_wh]);
}
