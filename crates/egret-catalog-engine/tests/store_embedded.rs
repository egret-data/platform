//! Embedded-catalog RBAC store: warehouse → role → Catalog Client create + verify.
mod common;

use egret_catalog_engine::NewClient;
use uuid::Uuid;

#[tokio::test]
async fn create_client_then_verify() {
    let Some(store) = common::store().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    // The storage-connection registry moved to egret-storage (E3a); seed the row
    // the warehouse FK-references directly, so this crate needn't depend on it.
    let sc_id: Uuid = sqlx08::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("eb-store-{}", Uuid::new_v4().simple()))
    .fetch_one(store.pool())
    .await
    .unwrap();

    let wh_name = format!("eb-wh-{}", Uuid::new_v4().simple());
    let wh = store.create_warehouse(&wh_name, ws, sc_id).await.unwrap();
    let role = store
        .create_role(wh.id, "engine", "engine", None)
        .await
        .unwrap();

    let created = store
        .create_client(&NewClient {
            name: "trino-eng".into(),
            consumer_type: "engine".into(),
            workspace_id: ws,
            warehouse_id: wh.id,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    assert!(created.client_id.starts_with("egret_"));
    assert!(!created.client_secret.is_empty());

    // Correct secret verifies + returns (warehouse name, role name).
    let vc = store
        .verify_client(&created.client_id, &created.client_secret)
        .await
        .unwrap();
    assert_eq!(vc, Some((wh_name.clone(), "engine".to_string())));

    // Wrong secret → None.
    let bad = store
        .verify_client(&created.client_id, "WRONG")
        .await
        .unwrap();
    assert_eq!(bad, None);
}

// The engine/engine_catalog CRUD tests moved to
// `egret-engine-mgmt/tests/store.rs` alongside the `EngineMgmtStore` methods
// (E1b) — this crate no longer owns that data.
