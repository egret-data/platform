//! DB + MinIO gated test for the PROD per-workspace warehouse provisioner
//! (`provision_workspace_warehouse`, CE-2 T4).
//!
//! Proves the provisioning path end-to-end at the function level (the plan's
//! "test `provision_workspace_warehouse` directly" option — the full HTTP
//! handler path can't run here because building the lakekeeper engine state into
//! `WorkspaceState` is CE-2 T5):
//!
//!   1. seed a workspace row in egret's `public` schema,
//!   2. call `provision_workspace_warehouse` → it creates a real lakekeeper
//!      warehouse (live MinIO `validate_access`) and records the
//!      workspace→warehouse mapping,
//!   3. assert `warehouse_for_workspace` / `warehouse_for_slug` both resolve to
//!      that warehouse UUID,
//!   4. call it AGAIN → same warehouse UUID, no error (idempotent).
//!
//! Needs MinIO reachable at `localhost:9000` with a writable `warehouse` bucket
//! (the egret dev stack ships it). Self-skips when `DATABASE_URL` is unset.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test provision_ws

use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    ensure_bootstrapped, provision_workspace_warehouse, run_lakekeeper_migrations,
};
use egret_metastore::Db;
use egret_storage::StorageConfig;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

/// Dev MinIO config matching the egret dev stack (bucket `warehouse`,
/// egret/egretsecret, S3-compat at localhost:9000).
fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

#[tokio::test]
async fn provision_workspace_warehouse_maps_and_is_idempotent() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping provision_workspace_warehouse test");
        return;
    };

    // egret's own migrations (workspaces + workspace_warehouse tables in `public`).
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    // lakekeeper's catalog schema + default project (provisioning authorizes
    // against the bootstrapped system principal).
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");
    let engine_state = egret_catalog_engine::build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    // Seed an owner + a workspace through the egret-side (sqlx-0.8) repos, so the
    // test never mixes sqlx majors against `db.pool`. Unique slug/email so the
    // test is re-runnable against a persistent catalog.
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = UserRepo::new(db.clone())
        .insert_local(&format!("t4-{suffix}@egret.test"), "T4 Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("t4ws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "T4 Provision Test", owner)
        .await
        .expect("seed workspace row");
    let workspace_id: Uuid = ws.id.0;

    let store = CatalogStore::new(db.clone());
    let storage_cfg = dev_storage_config();

    // No mapping yet.
    assert_eq!(
        store
            .warehouse_for_workspace(workspace_id)
            .await
            .expect("query mapping"),
        None,
        "freshly-seeded workspace has no warehouse mapping"
    );

    // 1st provision: creates the warehouse (live MinIO validate_access) + mapping.
    let wh = provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id,
        &slug,
        &storage_cfg,
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    // Mapping resolves both ways to the returned warehouse UUID.
    assert_eq!(
        store.warehouse_for_workspace(workspace_id).await.unwrap(),
        Some(wh),
        "warehouse_for_workspace resolves to the provisioned warehouse"
    );
    assert_eq!(
        store.warehouse_for_slug(&slug).await.unwrap(),
        Some(wh),
        "warehouse_for_slug resolves to the provisioned warehouse"
    );

    // 2nd provision: idempotent — same UUID, no error, no second warehouse.
    let wh_again = provision_workspace_warehouse(
        engine_state,
        &store,
        workspace_id,
        &slug,
        &storage_cfg,
        SYSTEM_USER,
    )
    .await
    .expect("re-provision is idempotent");
    assert_eq!(
        wh_again, wh,
        "re-provisioning returns the same warehouse UUID"
    );
}
