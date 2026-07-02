//! Egret catalog storage layer: the Postgres-backed `CatalogStore` providing
//! the per-workspace storage-connection registry (`workspace_storages`).
//!
//! The catalog-token and warehouse-mapping modules were removed as part of the
//! REST-catalog redesign; a later plan moves the storage-connection code to the
//! control plane and reuses it.
pub mod error;
pub mod service;

pub use error::CatalogError;
pub use service::StorageConn;
