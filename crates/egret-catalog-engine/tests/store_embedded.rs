//! Embedded-catalog RBAC store: warehouse → role → Catalog Client create + verify.
mod common;

use egret_catalog_engine::NewClient;
use uuid::Uuid;

#[tokio::test]
async fn create_client_then_verify() {
    let Some(store) = common::store().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    // The storage-connection registry moved to egret-storage (E3a); seed the row
    // the warehouse FK-references directly, so this crate needn't depend on it.
    let sc_id: Uuid = sqlx::query_scalar(
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

/// `current` reads the same row `verify` does, without the secret: the same
/// token shape, the stored hash as-is, and nothing once the client is gone.
#[tokio::test]
async fn current_mirrors_verify_without_the_secret() {
    use egret_machine_idp::ClientVerifier;
    let Some(store) = common::store().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let ws = common::seed_workspace(&store).await;
    let sc_id: Uuid = sqlx::query_scalar(
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

    let verified = ClientVerifier::verify(&store, &created.client_id, &created.client_secret)
        .await
        .unwrap()
        .expect("must verify");
    let state = store
        .current(&created.client_id)
        .await
        .unwrap()
        .expect("a live client has a current state");
    assert_eq!(state.verified, verified.verified);

    let stored_hash: String =
        sqlx::query_scalar("SELECT client_secret_hash FROM catalog.catalog_clients WHERE id = $1")
            .bind(created.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(state.secret_hash, stored_hash);
    assert_eq!(
        verified.secret_hash, stored_hash,
        "verify must hand back the hash its Argon2 check used"
    );

    assert!(store.current("egret_unknown").await.unwrap().is_none());

    store.revoke_client(created.id).await.unwrap();
    assert!(
        store.current(&created.client_id).await.unwrap().is_none(),
        "a deleted client has no current state"
    );
}

// The engine/engine_catalog CRUD tests moved to
// `egret-engine-mgmt/tests/store.rs` alongside the `EngineMgmtStore` methods
// (E1b) — this crate no longer owns that data.
