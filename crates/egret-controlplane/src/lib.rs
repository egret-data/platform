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

pub use auth::require_platform_admin;
pub use error::CpError;

use axum::extract::FromRef;
use axum::Router;
use egret_auth::session::SessionService;
use egret_catalog_engine::{CatalogStore, WarehouseBrowser};
use egret_secrets::WorkspaceSecretStore;
use egret_storage::StorageRegistry;
use std::sync::Arc;

/// Shared state for the control-plane router.
#[derive(Clone)]
pub struct CpState {
    /// Embedded-catalog store; used here only to list warehouses for the storage
    /// credential-rotation push-down (the warehouse/role/grant/client admin moved
    /// to egret-catalog-engine in E3d-3).
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
    /// Reads catalog structure from the embedded lakekeeper (wired in egret-bin);
    /// `None` in tests. Used by the Storage router's credential-rotation push-down
    /// (the warehouse admin router itself moved to egret-catalog-engine in E3d-3).
    pub browser: Option<Arc<dyn WarehouseBrowser>>,
    /// Cedar policy decision point (B1b). Built in `egret-bin` from the SAME
    /// `EventBus` as `roles` so `RolesChanged` hot-reloads it; stored here as a
    /// trait object so this crate stays independent of Cedar/egret-auth.
    pub pdp: Arc<dyn egret_core::authz::Pdp>,
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
        .merge(access::router())
}

/// The control-plane seam's OpenAPI document (E0·P4b-3): the access, storage,
/// and secrets routers' `#[utoipa::path]` handlers merged into one `OpenApi`.
/// `egret-server` merges this into the top-level API doc. This is the per-crate
/// `openapi()` convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = access::ControlPlaneAccessApi::openapi();
    doc.merge(storage::ControlPlaneStorageApi::openapi());
    doc.merge(secrets::ControlPlaneSecretsApi::openapi());
    doc
}
