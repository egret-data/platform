//! Egret catalog storage layer: the Postgres-backed `CatalogStore` providing
//! embedded-catalog warehouses, roles/grants, and Catalog Clients.
//!
//! This module was the standalone `egret-catalog` crate (the store over the
//! shared `egret_metastore::Db` pool); E3d-1 merged it into
//! `egret-catalog-engine` so the Catalog seam lives in one crate. The store's
//! SQL runs on `egret_metastore::Db`'s pool — the same sqlx as lakekeeper's,
//! since the workspace moved to 0.9; this module used to reach it through a
//! `sqlx08` alias because the two majors could not share a `Pool<Postgres>`.
pub mod embedded;
pub mod error;
/// `CatalogPort` (egret-catalog-api) impl over `CatalogStore` — the E3b seam
/// boundary. Only trait/`From` impls; nothing to re-export.
pub mod handle;
mod port_impl;
pub use embedded::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
pub use error::CatalogError;
pub use handle::CatalogStore;
