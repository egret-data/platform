//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
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
