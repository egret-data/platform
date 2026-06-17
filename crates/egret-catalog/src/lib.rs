//! Iceberg REST Catalog: HTTP layer + Postgres CatalogStore + commit engine.
pub mod error;
pub mod service;

pub use error::CatalogError;
