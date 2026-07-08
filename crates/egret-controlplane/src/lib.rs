//! Control-plane HTTP surface: platform-admin-only, GLOBAL Storage + Secrets.
//!
//! After migration 0050 storage connections are global (`storage_connections`)
//! and named secrets are global-capable (nullable `workspace_id`). This crate
//! rebuilds their HTTP routers at the control plane; every handler is gated on
//! `require_platform_admin` and every store call uses the global scope
//! (`secrets.*(None, ..)`, the global `StorageConn` CRUD on `CatalogStore`).
//!
//! The router is nested at `/api/control-plane` in `egret-server`, behind the
//! same CSRF layer as the other mutating APIs.

mod access;
mod auth;
mod error;
mod policies;
mod secrets;
mod storage;
mod warehouses;

pub use auth::require_platform_admin;
pub use error::CpError;
pub use policies::{
    editor_to_ranger_json, masking_to_ranger_json, rowfilter_to_ranger_json, summarize_policy,
    MaskingPolicyInput, PolicySummary, RangerPolicy, RowFilterPolicyInput,
};
pub use warehouses::{
    BrowseColumn, BrowseDeletedTabular, BrowseNamespace, BrowsePartition, BrowseSearchResult,
    BrowseSnapshot, BrowseSort, BrowseTableDetail, BrowseTabular, BrowseTask,
    BrowseWarehouseConfig, ProvisionS3, WarehouseBrowser, WarehouseProvisioner,
};

use axum::extract::FromRef;
use axum::Router;
use egret_auth::session::SessionService;
use egret_catalog::CatalogStore;
use egret_secrets::WorkspaceSecretStore;
use std::sync::Arc;

/// Shared state for the control-plane router.
#[derive(Clone)]
pub struct CpState {
    /// Global storage-connection registry (`storage_connections`).
    pub storage: CatalogStore,
    /// Shared Ranger policy store (`authz_*`), the same store the machine-facing
    /// `/service` router writes. Used by the read-only Policies endpoints.
    pub authz: egret_authz::AuthzStore,
    /// Identity lookups for the policy principal picker.
    pub users: egret_auth::users::UserRepo,
    pub groups: egret_auth::groups::GroupRepo,
    /// Platform RBAC roles, permissions, and principal assignments.
    pub roles: egret_auth::roles::RoleRepo,
    /// Named secret layer, used in GLOBAL scope (`None` workspace) here.
    pub secrets: WorkspaceSecretStore,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// Provisions the embedded lakekeeper warehouse on create (wired in egret-bin);
    /// `None` in tests (the egret row is recorded without a lakekeeper counterpart).
    pub provisioner: Option<Arc<dyn WarehouseProvisioner>>,
    /// Reads catalog structure from the embedded lakekeeper (wired in egret-bin);
    /// `None` in tests.
    pub browser: Option<Arc<dyn WarehouseBrowser>>,
    /// Cross-seam, read-only: the `policies` module's resource-lookup/profile
    /// listing surfaces which engines/catalogs are bound to a Ranger service,
    /// and `trino_config` needs egret's public URL to render the plugin config.
    /// Owned by the `egret-engine-mgmt` seam (E1c); read directly here, same
    /// as `EngineMgmtState` reads `CatalogStore`/`AuthzStore` cross-seam.
    pub engine_mgmt: egret_engine_mgmt::EngineMgmtStore,
    /// In-cluster URL Trino pods use to reach egret's REST catalog + token
    /// endpoint (e.g. `http://egret:8080`); populated from config in egret-bin.
    pub engine_egret_url: String,
}

// The `Auth(principal)` extractor pulls `SessionService` from the router state.
impl FromRef<CpState> for SessionService {
    fn from_ref(s: &CpState) -> Self {
        s.sessions.clone()
    }
}

/// Build the control-plane router (Storage + Secrets), to be nested at
/// `/api/control-plane`.
pub fn control_plane_router() -> Router<CpState> {
    Router::new()
        .merge(storage::router())
        .merge(secrets::router())
        .merge(warehouses::router())
        .merge(policies::router())
        .merge(access::router())
}
