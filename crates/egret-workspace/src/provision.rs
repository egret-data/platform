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

/// Per-workspace storage override supplied at workspace-creation time (CE-5).
///
/// When `Some`, the provisioner will (in a later task) configure the lakekeeper
/// warehouse to use this storage backend instead of the system-wide default.
/// All fields map 1-to-1 to the `workspace_storage` row written by the
/// CE-5 Task 1 store.
#[derive(Clone)]
pub struct StorageSpec {
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    /// Reserved: persisted to `workspace_storage` but NOT yet applied. The
    /// provisioner currently lays the warehouse out at the fixed
    /// `workspaces/<slug>/warehouse` prefix regardless of this value, so a stored
    /// `key_prefix` is recorded metadata, not an active override (wiring it
    /// through is future work).
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}

impl std::fmt::Debug for StorageSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact credentials (mirrors `egret_storage::StorageConfig`'s Debug):
        // this hits HTTP/tracing paths during workspace provisioning.
        f.debug_struct("StorageSpec")
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .finish()
    }
}

/// Read-only view of a workspace's effective storage configuration (CE-5),
/// returned by the `GET /workspaces/{workspace}/storage` endpoint.
///
/// **Never carries the secret value.** `access_key` is the (non-secret) S3 key
/// id; the secret key is represented only by the `has_secret` boolean (whether a
/// sealed `secret_ref` exists in the vault). This type is `Serialize`-only so the
/// secret can never leak into a response body.
#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkspaceStorageInfo {
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    /// Reserved: the stored value, surfaced for visibility. NOT currently applied
    /// to the warehouse layout (see [`StorageSpec::key_prefix`]) — the warehouse
    /// uses the fixed `workspaces/<slug>/warehouse` prefix.
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub has_secret: bool,
}

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
    ///
    /// `storage` is `None` until CE-5 Task 3 wires per-workspace storage; the
    /// engine impl ignores it for now and falls back to the system-wide config.
    async fn provision(
        &self,
        workspace_id: WorkspaceId,
        slug: &str,
        storage: Option<StorageSpec>,
    ) -> Result<(), String>;

    /// Return the workspace's per-workspace storage config (CE-5), WITHOUT the
    /// secret value. `Ok(Some(info))` when the workspace has a stored
    /// `workspace_storage` row; `Ok(None)` when it has none (it provisions
    /// against the global default and exposes no per-workspace storage). Powers
    /// the read-only `GET /workspaces/{workspace}/storage` endpoint.
    async fn storage_info(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Option<WorkspaceStorageInfo>, String>;
}

/// Shared, optional provisioner handle carried on `WorkspaceState`.
///
/// `None` means "no warehouse provisioning configured" — e.g. when the embedded
/// catalog engine is disabled, or in tests that exercise plain workspace CRUD.
/// In that case workspace creation succeeds without provisioning a warehouse.
pub type SharedWarehouseProvisioner = Option<Arc<dyn WarehouseProvisioner>>;
