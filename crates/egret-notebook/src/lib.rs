//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod error;
pub mod model;
pub mod routes;
pub mod store;

pub use routes::{notebook_router, NotebookState};
pub use store::NotebookStore;
