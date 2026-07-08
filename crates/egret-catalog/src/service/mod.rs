pub mod embedded;
/// `CatalogPort` (egret-catalog-api) impl over `CatalogStore` — the E3b seam
/// boundary. Only trait/`From` impls; nothing to re-export.
mod port_impl;
pub mod store;
pub use embedded::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
pub use store::CatalogStore;
