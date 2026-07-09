//! Engine-provisioning seam storage: owns the `engine_mgmt` schema.

pub mod engines;
pub mod error;
pub mod http_error;
pub mod monitor;
pub mod state;
pub mod store;

pub use engines::router as engine_mgmt_router;
pub use error::EngineMgmtError;
pub use http_error::EngineHttpError;
pub use state::EngineMgmtState;
pub use store::{Engine, EngineCatalog, EngineMgmtStore};

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
    doc
}
