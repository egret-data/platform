//! Iceberg REST Catalog: HTTP layer + Postgres CatalogStore + commit engine.
pub mod api;
pub mod auth;
pub mod error;
pub mod http_error;
pub mod routes;
pub mod service;

pub use error::CatalogError;
pub use routes::{iceberg_router, CatalogState};
