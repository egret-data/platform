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
pub mod audit;
mod error;
mod governance_browse;
mod lineage_sweep;
mod secrets;
mod storage;
mod tags;
mod view_exposure;

pub use error::CpError;
pub use governance_browse::{
    BrowseColumn, BrowseEngine, BrowseItem, BrowseKind, BrowseResponse, BrowseSource,
    CatalogLister, Listing, ListingEngine, ListingEngineType, NotListed, NotListedReason,
    UnwiredCatalogLister,
};
pub use view_exposure::{
    EngineNotChecked, ExposureEngine, LineageState, MatchedBy, MaterializedViewLister, MvRow,
    SecurityMode, TrinoViewItem, TrinoViewReason, ViewExposureDto, ViewExposureItem,
};

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
    /// Effective-access resolver (AX-3): enumerates a principal's roles,
    /// permissions, workspace memberships, and object grants for the
    /// `GET /access/effective` console endpoint.
    pub access_resolver: egret_auth::effective::AccessResolver,
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
    /// Append-only audit sink (B4). Mutation events are emitted at the
    /// grant-change handlers; best-effort, never fails the request.
    pub audit: egret_audit::AuditSink,
    /// Read side of the audit trail (B4), backing `GET /control-plane/audit`.
    pub audit_store: egret_audit::AuditStore,
    /// Platform classification-tag catalog (Access P3.1, metadata only — see
    /// `egret_bi::tags`).
    pub tags: egret_bi::TagRepo,
    /// Classification access policies (Access P3.3): which clearance roles a tag
    /// requires. Mutations publish `RolesChanged` on the SAME bus as `roles` so
    /// the Cedar PDP reloads. Model + CRUD only in this task; PDP enforcement is
    /// a later task.
    pub classification_policies: egret_bi::ClassificationPolicyRepo,
    /// The database, for the cross-schema reads the classification page needs
    /// (`GET /classifications/{id}/view-exposure`).
    pub db: egret_metastore::Db,
    /// Lists a StarRocks database's materialized views (view governance A4).
    /// The real one dials the engine's FE and is wired in `egret-bin`; tests
    /// pass a fake.
    pub mv_lister: std::sync::Arc<dyn MaterializedViewLister>,
    /// Deadline for ALL of one report's listings together; tests shorten it.
    pub mv_list_timeout: std::time::Duration,
    /// The view-governance lineage sweep (B3), run on demand by
    /// `POST /classifications/lineage-sweep`. The real one is
    /// `egret_catalog_engine::LineageSweep`, wired in `egret-bin`; tests pass a
    /// fake.
    pub lineage_sweeper: Arc<dyn egret_catalog_engine::LineageSweeper>,
    /// The views of egret's catalogs and their stored security mode, for the
    /// DEFINER audit (view governance B5). The real one is
    /// `egret_catalog_engine::EmbeddedViewCatalog`, wired in `egret-bin`;
    /// tests pass a fake.
    pub view_catalog: Arc<dyn egret_catalog_engine::ViewCatalog>,
    /// Deadline for the whole Trino half of one view-exposure report (the
    /// listing, every lineage read and every metadata load); tests shorten it.
    pub view_audit_timeout: std::time::Duration,
    /// Lists one level of a non-egret catalog on an engine, for the Governance
    /// Data tree (`GET /governance/browse/{catalog_id}`). The real Trino and
    /// StarRocks listers are wired in `egret-bin`; tests pass a fake.
    pub catalog_lister: Arc<dyn CatalogLister>,
    /// Deadline for one browse listing, on an engine or on egret's
    /// warehouse; tests shorten it.
    pub browse_timeout: std::time::Duration,
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
        .merge(audit::audit_router())
        .merge(tags::router())
        .merge(view_exposure::router())
        .merge(lineage_sweep::router())
        .merge(governance_browse::router())
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
    doc.merge(audit::ControlPlaneAuditApi::openapi());
    doc.merge(tags::ControlPlaneTagsApi::openapi());
    doc.merge(view_exposure::ControlPlaneViewExposureApi::openapi());
    doc.merge(lineage_sweep::ControlPlaneLineageSweepApi::openapi());
    doc.merge(governance_browse::ControlPlaneGovernanceBrowseApi::openapi());
    doc
}
