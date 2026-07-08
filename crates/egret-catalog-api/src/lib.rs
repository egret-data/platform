//! Catalog seam contract (E3b contract-crate): the engine-facing wire DTOs + the
//! `CatalogPort` trait that cross-seam callers depend on instead of the concrete
//! `CatalogStore`. Leaf crate — serde-only, no sqlx/axum/egret-* deps.
pub mod dto;
pub mod port;

pub use dto::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
pub use port::{CatalogApiError, CatalogPort};
