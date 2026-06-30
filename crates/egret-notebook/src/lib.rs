//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod error;
pub mod model;
pub mod provisioning;
pub mod routes;
pub mod runtime;
pub mod store;

pub use provisioning::{provision_notebook_access, NamespaceProvisioner, Provisioned};
pub use routes::{notebook_router, NotebookState};
pub use store::NotebookStore;
