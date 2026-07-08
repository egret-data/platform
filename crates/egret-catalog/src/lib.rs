//! Egret catalog storage layer: the Postgres-backed `CatalogStore` providing
//! embedded-catalog warehouses, roles/grants, and Catalog Clients.
//!
//! The catalog-token and warehouse-mapping modules were removed as part of the
//! REST-catalog redesign; the global storage-connection registry was relocated to
//! `egret-storage` (E3a).
pub mod error;
pub mod service;

pub use error::CatalogError;
pub use service::CatalogStore;
