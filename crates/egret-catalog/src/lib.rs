//! Egret catalog identity layer: the Postgres-backed `CatalogStore` providing
//! per-workspace catalog tokens and the workspace → warehouse mapping.
//!
//! The Iceberg REST catalog itself is served by the embedded lakekeeper engine
//! (`egret-catalog-engine`); this crate now only owns the reusable A-3 identity
//! bits that the engine front-auth, the browse API, warehouse provisioning, and
//! the `egret catalog mint-token` CLI depend on.
pub mod error;
pub mod service;

pub use error::CatalogError;
