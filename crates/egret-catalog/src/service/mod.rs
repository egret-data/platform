pub mod embedded;
pub mod store;
pub use embedded::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
pub use store::CatalogStore;
