//! Backfill: two engines with primaries on different warehouses keep normalised
//! names, and every saved object that names `egret.` is reported. A row that
//! cannot be migrated is reported and blocks only its engine.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn several_primaries_keep_normalised_names_and_report_saved_sql() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = &db.pool;
    let ws: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug, name, status) VALUES ('bf-many', 'BF', 'active') RETURNING id").fetch_one(pool).await.unwrap();
    let sc: Uuid = sqlx::query_scalar("INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) VALUES ('bf-sc2', 's3', 'us-east-1', 'b', 'k', true) RETURNING id").fetch_one(pool).await.unwrap();
    let mut engines = Vec::new();
    for (wh_name, eng) in [
        ("lake-a", "trino-a"),
        ("lake-b", "trino-b"),
        ("lake-c", "trino-c"),
    ] {
        let wh: Uuid = sqlx::query_scalar("INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id")
            .bind(wh_name).bind(ws).bind(sc).fetch_one(pool).await.unwrap();
        let e: Uuid = sqlx::query_scalar("INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id")
            .bind(eng).bind(wh).fetch_one(pool).await.unwrap();
        engines.push((e, wh_name));
    }
    for (e, wh_name) in &engines[..2] {
        sqlx::query("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, 'egret', 'egret-warehouse', $2, true)")
            .bind(e).bind(json!({ "warehouse": wh_name, "credential": "id:secret", "clientRowId": Uuid::new_v4() }))
            .execute(pool).await.unwrap();
    }
    // trino-c's primary has no credential: it cannot be migrated.
    sqlx::query("INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) VALUES ($1, 'egret', 'egret-warehouse', '{\"warehouse\":\"lake-c\"}'::jsonb, true)")
        .bind(engines[2].0).execute(pool).await.unwrap();

    // Saved SQL: four objects name egret., one does not.
    let db_id: Uuid = sqlx::query_scalar("INSERT INTO query.databases (workspace_id, name, engine, host, port) VALUES ($1, 'bf-conn', 'trino', 'h', 8080) RETURNING id")
        .bind(ws).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.saved_queries (workspace_id, name, sql) VALUES ($1, 'orders', 'SELECT * FROM egret.sales.orders'), ($1, 'quoted', 'select 1 from \"EGRET\" . s.t'), ($1, 'unrelated', 'SELECT * FROM my_egret.s.t')")
        .bind(ws).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.datasets (workspace_id, database_id, name, catalog, schema_name, table_name) VALUES ($1, $2, 'orders-ds', 'egret', 'sales', 'orders')")
        .bind(ws).bind(db_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO analytics.sql_snippets (workspace_id, name, body) VALUES ($1, 'snip', 'JOIN egret.dim.customers c')")
        .bind(ws).execute(pool).await.unwrap();

    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let vault = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = RegistryStore::new(db.clone());
    let run = run_backfill(&store, &vault).await.expect("backfill");

    assert!(
        store.get_catalog_by_name("egret").await.unwrap().is_none(),
        "no exception with several primaries"
    );
    for name in ["lake_a", "lake_b", "lake_c"] {
        assert!(
            store.get_catalog_by_name(name).await.unwrap().is_some(),
            "{name}"
        );
    }
    assert_eq!(run.failed_rows, 1);
    assert_eq!(
        store
            .unmigrated_legacy_catalog_count(engines[2].0)
            .await
            .unwrap(),
        1,
        "trino-c blocked"
    );
    assert_eq!(
        store
            .unmigrated_legacy_catalog_count(engines[0].0)
            .await
            .unwrap(),
        0
    );

    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT code, detail FROM registry.migration_report WHERE run_id = $1 ORDER BY id",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await
    .unwrap();
    let renamed = rows
        .iter()
        .filter(|(c, _)| c == "engine_catalog_renamed")
        .count();
    assert_eq!(renamed, 2, "{rows:?}");
    assert_eq!(
        rows.iter().filter(|(c, _)| c == "backfill_failed").count(),
        1
    );
    let mut hits: Vec<String> = rows
        .iter()
        .filter(|(c, _)| c == "saved_sql_references_catalog")
        .map(|(_, d)| d["objectType"].as_str().unwrap().to_string())
        .collect();
    hits.sort();
    assert_eq!(
        hits,
        vec!["dataset", "saved_query", "saved_query", "sql_snippet"],
        "{rows:?}"
    );
}
