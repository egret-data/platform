//! Engine-provisioning seam storage: owns the `engine_mgmt` schema.

pub mod assignment;
pub(crate) mod catalog_apply;
pub mod catalogs;
pub mod delegation_default;
pub mod delegator_policy;
pub mod dependents;
pub mod engines;
pub mod error;
pub mod external;
pub mod http_error;
pub mod monitor;
pub mod probe;
pub mod profile_compiler;
pub mod starrocks_ddl;
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
// `fe_target` stays `pub(crate)` (used by Task 4/5 apply/bootstrap code within
// this crate via `crate::starrocks_ddl::fe_target`) and is not re-exported here.
pub use probe::{tcp_probe, ProbeOutcome, PROBE_TIMEOUT};
pub use profile_compiler::{OverlapSnapshot, ProfileCompilerPort};
pub use starrocks_ddl::{
    compare_to_fe, parse_show_create_properties, show_catalogs, show_create_catalog, Drift,
    FakeStarRocksDdl, FeError, FeTarget, MysqlDdlRunner, StarRocksDdl, FE_ERROR_EXCERPT_MAX,
    FE_MASKED_KEYS, FE_UNCOMPARABLE_KEYS,
};
pub use state::EngineMgmtState;
pub use store::{engine_status, Engine, EngineMgmtStore};

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
    doc.merge(external::ExternalEngineApi::openapi());
    doc
}
