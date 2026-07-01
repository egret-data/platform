//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod engine_token;
pub mod error;
pub mod model;
pub mod output;
pub mod provisioning;
pub mod routes;
pub mod runtime;
pub mod store;

pub use provisioning::{provision_notebook_access, NamespaceProvisioner, Provisioned};
pub use routes::{notebook_router, NotebookState};
pub use runtime::{build_notebook_runtime, NotebookRuntime, RuntimeCatalog};
pub use store::NotebookStore;
