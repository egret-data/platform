//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod docs_port;
pub mod engine_token;
pub mod error;
pub mod model;
pub mod output;
pub mod routes;
pub mod runtime;
pub mod store;

pub use routes::{notebook_router, NotebookState};
pub use runtime::{build_notebook_runtime, NotebookRuntime};
pub use store::NotebookStore;

/// The Notebook seam's OpenAPI document (E0·P4b-8): the workspace-scoped notebook
/// router (CRUD, cells, folders, trash, bookmarks, settings, runtime manifest).
/// `egret-server` calls this and merges it into the top-level API doc — the
/// per-crate `openapi()` convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    routes::NotebookApi::openapi()
}
