//! Async query engine: connectors (Trino/StarRocks), job manager, result store.

pub mod connector;
mod error;
mod job;
pub mod model;
mod routes;
pub mod store;

pub use error::{EngineApiError, EngineError};
pub use store::{Query, QueryStore};
