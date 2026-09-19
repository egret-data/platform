//! Engine-provisioning seam storage: owns the `engine_mgmt` schema.

pub mod assignment;
pub(crate) mod catalog_apply;
pub mod catalogs;
pub mod delegation_default;
pub mod delegator_policy;
pub mod dependents;
pub mod engines;
pub mod error;
pub mod http_error;
pub mod monitor;
pub mod state;
pub mod store;
pub mod tls_cert;

pub use assignment::EngineAssignmentService;
pub use delegation_default::{
    has_platform_jwks, resolve_delegation, starrocks_image_is_pre_3_5, ResolvedDelegation,
};
pub use engines::router as engine_mgmt_router;
pub use engines::workspace_engines_router;
pub use error::EngineMgmtError;
pub use http_error::EngineHttpError;
pub use state::EngineMgmtState;
pub use store::{engine_status, Engine, EngineCatalog, EngineMgmtStore};

/// Aggregated OpenAPI fragment for the engine-provisioning seam (E0·P4b-9):
/// the engines CRUD/lifecycle/catalog/pod-logs-exec router
/// ([`engines::EngineMgmtApi`]) unioned with the native-Trino monitor routes
/// merged onto the same router ([`monitor::MonitorApi`]). `egret-server` merges
/// this into the top-level API doc — the per-crate `openapi()` convention every
/// annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = engines::EngineMgmtApi::openapi();
    doc.merge(monitor::MonitorApi::openapi());
    doc.merge(catalogs::CatalogRegistryApi::openapi());
    doc
}
