//! Concrete [`WarehouseProvisioner`] implementation backed by the embedded
//! lakekeeper engine (CE-2 T5).
//!
//! `egret-workspace` defines the [`WarehouseProvisioner`] trait but must stay
//! free of the lakekeeper engine graph, so the workspace-create handler depends
//! only on the trait. This module — in `egret-catalog-engine`, where the engine
//! `ApiContext`, the egret-side `CatalogStore`, and the storage config already
//! live — supplies the impl. The composition root (`egret-bin`) constructs one
//! and sets it on `WorkspaceState.warehouse_provisioner`.
//!
//! `provision` simply delegates to [`crate::provision_workspace_warehouse`]
//! (idempotent, live MinIO `validate_access`), discarding the returned warehouse
//! UUID — the mapping it persists is the durable record the rest of egret reads.

use async_trait::async_trait;
use egret_catalog::service::CatalogStore;
use egret_core::WorkspaceId;
use egret_storage::StorageConfig;
use egret_workspace::provision::WarehouseProvisioner;
use lakekeeper::api::ApiContext;

use crate::mount::EngineState;
use crate::provision_workspace_warehouse;

/// Engine-backed warehouse provisioner. Holds everything
/// [`provision_workspace_warehouse`] needs so the workspace-create handler can
/// invoke it through the trait without knowing about lakekeeper.
#[derive(Clone)]
pub struct EngineWarehouseProvisioner {
    engine_state: ApiContext<EngineState>,
    store: CatalogStore,
    storage_cfg: StorageConfig,
    system_user: String,
}

impl EngineWarehouseProvisioner {
    /// Assemble the provisioner from the live engine state + egret-side store +
    /// runtime storage config + the configured `catalog.system_user`.
    pub fn new(
        engine_state: ApiContext<EngineState>,
        store: CatalogStore,
        storage_cfg: StorageConfig,
        system_user: String,
    ) -> Self {
        Self {
            engine_state,
            store,
            storage_cfg,
            system_user,
        }
    }
}

#[async_trait]
impl WarehouseProvisioner for EngineWarehouseProvisioner {
    async fn provision(&self, workspace_id: WorkspaceId, slug: &str) -> Result<(), String> {
        provision_workspace_warehouse(
            self.engine_state.clone(),
            &self.store,
            workspace_id.0,
            slug,
            &self.storage_cfg,
            &self.system_user,
        )
        .await
        .map(|_warehouse_id| ())
        .map_err(|e| e.to_string())
    }
}
