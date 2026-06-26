//! Pluggable warehouse-provisioning hook for workspace creation (CE-2 T4).
//!
//! ## Why a trait object instead of a direct call
//!
//! When a workspace is created its backing lakekeeper *warehouse* must be
//! provisioned (and the workspace→warehouse mapping persisted). That logic lives
//! in `egret-catalog-engine`, which embeds lakekeeper. But `egret-workspace`
//! must **not** depend on `egret-catalog-engine` — the engine pulls in the whole
//! lakekeeper graph (and a second `sqlx` major), and `egret-catalog-engine`
//! already depends on `egret-catalog`, which is conceptually downstream of the
//! workspace primitives. A direct dependency would create a layering cycle.
//!
//! So the workspace-create handler depends only on this small **`WarehouseProvisioner`
//! trait**. The composition root (`egret-bin`) — which holds the lakekeeper
//! engine state, the storage config, and the catalog store — supplies a concrete
//! implementation that calls
//! `egret_catalog_engine::provision_workspace_warehouse`. egret-workspace stays
//! engine-free; the engine dependency lives only where the full state is already
//! assembled.

use async_trait::async_trait;
use egret_core::WorkspaceId;
use std::sync::Arc;

/// Provisions (idempotently) the lakekeeper warehouse backing a freshly-created
/// workspace and records the workspace→warehouse mapping.
///
/// Implemented in the composition root over `egret-catalog-engine`. The
/// workspace-create handler invokes this immediately after the workspace row is
/// committed.
#[async_trait]
pub trait WarehouseProvisioner: Send + Sync {
    /// Ensure a warehouse exists for `(workspace_id, slug)` and the mapping is
    /// stored. Idempotent: safe to call again for an already-provisioned
    /// workspace. Returns an error string on failure (a workspace without a
    /// warehouse is unusable, so the handler surfaces this).
    async fn provision(&self, workspace_id: WorkspaceId, slug: &str) -> Result<(), String>;
}

/// Shared, optional provisioner handle carried on `WorkspaceState`.
///
/// `None` means "no warehouse provisioning configured" — e.g. when the embedded
/// catalog engine is disabled, or in tests that exercise plain workspace CRUD.
/// In that case workspace creation succeeds without provisioning a warehouse.
pub type SharedWarehouseProvisioner = Option<Arc<dyn WarehouseProvisioner>>;
