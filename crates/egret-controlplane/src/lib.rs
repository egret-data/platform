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

mod auth;
mod error;
mod polaris_conn;
mod secrets;
mod storage;

pub use auth::require_platform_admin;
pub use error::CpError;
pub use polaris_conn::{polaris_client, PolarisConn, PolarisConnInput, PolarisConnStore};

use axum::extract::FromRef;
use axum::Router;
use egret_auth::session::SessionService;
use egret_catalog::CatalogStore;
use egret_secrets::WorkspaceSecretStore;

/// Shared state for the control-plane router.
#[derive(Clone)]
pub struct CpState {
    /// Global storage-connection registry (`storage_connections`).
    pub storage: CatalogStore,
    /// Named secret layer, used in GLOBAL scope (`None` workspace) here.
    pub secrets: WorkspaceSecretStore,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// Singleton Polaris connection registry (`polaris_connection`).
    pub polaris_conn: PolarisConnStore,
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
        .merge(polaris_conn::router())
}
