//! I3 (review fix round 1): a secret name collision with a DIFFERENT value must
//! never silently reuse the wrong secret. Two legacy rows whose warehouse
//! credential secret names collide (both reference the same `clientRowId`, as a
//! stale/duplicated Catalog Client reference could) but carry DIFFERENT
//! credentials: each engine's attachment must resolve to its OWN credential,
//! never the other's.
//!
//! DB-gated; FAILS without DATABASE_URL.

use egret_catalog_registry::{run_backfill, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn a_secret_name_collision_with_a_different_value_never_binds_the_wrong_credential() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.unwrap();
    db.migrate().await.unwrap();
    let pool = db.pool.clone();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ('bf-secret', 'BF', 'active') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ('bf-secret-sc', 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let wh_a: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-secret-wh-a', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let wh_b: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ('bf-secret-wh-b', $1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine_a: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-secret-trino-a', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(wh_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    let engine_b: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ('bf-secret-trino-b', 'trino', 'egret-engines', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(wh_b)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Two DIFFERENT engine primaries whose legacy rows happen to name the SAME
    // `clientRowId` (a stale/duplicated reference — the secret name the backfill
    // computes for a warehouse credential is `catalog-client-<clientRowId>`,
    // independent of the warehouse or engine) but carry DIFFERENT credentials.
    let shared_client = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine_a)
    .bind(json!({ "warehouse": "bf-secret-wh-a", "credential": "cid:alpha-secret", "clientRowId": shared_client }))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, is_primary) \
         VALUES ($1, 'egret', 'egret-warehouse', $2, true)",
    )
    .bind(engine_b)
    .bind(json!({ "warehouse": "bf-secret-wh-b", "credential": "cid:beta-secret", "clientRowId": shared_client }))
    .execute(&pool)
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

    // Both primaries migrated (with TWO primaries the "egret" exception does
    // not apply — each warehouse keeps its own normalised name, same as
    // `backfill_several_primaries.rs`); what matters here is their secrets.
    let cat_a = store
        .get_catalog_for_warehouse(wh_a)
        .await
        .unwrap()
        .expect("wh_a migrated");
    let cat_b = store
        .get_catalog_for_warehouse(wh_b)
        .await
        .unwrap()
        .expect("wh_b migrated");
    assert_ne!(cat_a.id, cat_b.id);

    let att_a = store
        .get_attachment(engine_a, cat_a.id)
        .await
        .unwrap()
        .expect("engine_a attached");
    let att_b = store
        .get_attachment(engine_b, cat_b.id)
        .await
        .unwrap()
        .expect("engine_b attached");

    // The heart of I3: two DIFFERENT secrets, each holding its OWN engine's
    // credential — never the other's, and never silently aliased.
    assert_ne!(
        att_a.credential_secret_id, att_b.credential_secret_id,
        "colliding secret names must never share one secret across different values"
    );
    assert_eq!(
        vault
            .resolve_plaintext(None, att_a.credential_secret_id.unwrap())
            .await
            .unwrap(),
        "cid:alpha-secret"
    );
    assert_eq!(
        vault
            .resolve_plaintext(None, att_b.credential_secret_id.unwrap())
            .await
            .unwrap(),
        "cid:beta-secret"
    );

    // The collision was reported, not silently resolved.
    let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT code, message, detail FROM registry.migration_report WHERE run_id = $1 ORDER BY id",
    )
    .bind(run.run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let codes: Vec<&str> = rows.iter().map(|(c, _, _)| c.as_str()).collect();
    assert!(codes.contains(&"secret_name_collision"), "{codes:?}");

    // Exactly two named secrets exist for this shared client id: the original
    // and its suffixed sibling — no third is created on a second boot.
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE name LIKE $1 ORDER BY name")
            .bind(format!("catalog-client-{shared_client}%"))
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(names.len(), 2, "{names:?}");
    let suffixed_name = names
        .iter()
        .find(|n| n.as_str() != format!("catalog-client-{shared_client}"))
        .expect("one of the two names must be the suffixed sibling");

    // M1: the `secret_extracted` message names the secret ACTUALLY used (the
    // suffixed one), not the base name the wanted-but-taken name — otherwise an
    // operator reading the report would go looking under the wrong name.
    assert!(
        rows.iter()
            .any(|(c, msg, _)| c == "secret_extracted" && msg.contains(suffixed_name.as_str())),
        "no secret_extracted message names the suffixed secret {suffixed_name}: {rows:?}"
    );
    // The collision entry's own detail names both the wanted and used names.
    let collision_detail = rows
        .iter()
        .find(|(c, _, _)| c == "secret_name_collision")
        .map(|(_, _, d)| d.clone())
        .unwrap();
    assert_eq!(
        collision_detail["usedName"].as_str(),
        Some(suffixed_name.as_str())
    );

    let second = run_backfill(&store, &vault).await.expect("second boot");
    assert_eq!((second.report_rows, second.failed_rows), (0, 0));
    let names_after: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE name LIKE $1 ORDER BY name")
            .bind(format!("catalog-client-{shared_client}%"))
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        names_after, names,
        "idempotent: no new secret, no re-suffixing, on a no-op boot"
    );
}
