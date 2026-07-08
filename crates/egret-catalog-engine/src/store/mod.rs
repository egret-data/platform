//! Egret catalog storage layer: the Postgres-backed `CatalogStore` providing
//! embedded-catalog warehouses, roles/grants, and Catalog Clients.
//!
//! This module was the standalone `egret-catalog` crate (the store over the
//! shared `egret_metastore::Db` pool); E3d-1 merged it into
//! `egret-catalog-engine` so the Catalog seam lives in one crate. The store's
//! SQL runs on the egret-side **sqlx 0.8** pool, aliased here as `sqlx08` to
//! coexist with the crate's sqlx 0.9 lakekeeper surface.
pub mod embedded;
pub mod error;
/// `CatalogPort` (egret-catalog-api) impl over `CatalogStore` — the E3b seam
/// boundary. Only trait/`From` impls; nothing to re-export.
mod port_impl;
pub mod store;
pub use embedded::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
pub use error::CatalogError;
pub use store::CatalogStore;
