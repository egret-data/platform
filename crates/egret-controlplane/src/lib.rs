//! Control-plane HTTP surface: platform-admin-only, GLOBAL Storage + Secrets.
//!
//! After migration 0050 storage connections are global
//! (`storage.storage_connections`, relocated to its own schema in E3a) and named
//! secrets are global-capable (nullable `workspace_id`). This crate rebuilds
//! their HTTP routers at the control plane; every handler is gated on
//! `require_platform_admin` and every store call uses the global scope
//! (`secrets.*(None, ..)`, the global `StorageConn` CRUD on `StorageRegistry`
//! from egret-storage).
//!
//! The router is nested at `/api/control-plane` in `egret-server`, behind the
//! same CSRF layer as the other mutating APIs.

mod access;
mod auth;
mod error;
mod secrets;
mod storage;
mod warehouses;

pub use auth::require_platform_admin;
pub use error::CpError;
pub use warehouses::{
    BrowseColumn, BrowseDeletedTabular, BrowseNamespace, BrowsePartition, BrowseSearchResult,
    BrowseSnapshot, BrowseSort, BrowseTableDetail, BrowseTabular, BrowseTask,
    BrowseWarehouseConfig, ProvisionS3, WarehouseBrowser, WarehouseProvisioner,
};

use axum::extract::FromRef;
use axum::Router;
use egret_auth::session::SessionService;
use egret_catalog_engine::CatalogStore;
use egret_secrets::WorkspaceSecretStore;
use egret_storage::StorageRegistry;
use std::sync::Arc;

/// Shared state for the control-plane router.
#[derive(Clone)]
pub struct CpState {
    /// Embedded-catalog store: warehouses + catalog RBAC (roles/grants/clients).
    pub storage: CatalogStore,
    /// Global storage-connection registry (`storage.storage_connections`, E3a).
    pub storage_registry: StorageRegistry,
    /// Identity lookups for the access-control console (roles/permissions).
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
        .merge(access::router())
}
