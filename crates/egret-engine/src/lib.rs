//! Async query engine: connectors (Trino/StarRocks), job manager, result store.

pub mod connector;
mod error;
mod job;
pub mod model;
pub mod routes;
pub mod store;

pub use error::{EngineApiError, EngineError};
pub use job::JobManager;
pub use routes::{engine_router, rows_to_csv, EngineState};
pub use store::{Query, QueryStore};
