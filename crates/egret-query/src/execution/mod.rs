//! Async query engine: connectors (Trino/StarRocks), job manager, result store.
//! Folded in from the former `egret-engine` crate (E4b).

pub mod connector;
mod error;
mod job;
pub mod model;
pub mod routes;
pub mod store;
/// Internal SQL-Lab parameter rendering. Public only so the integration test
/// `tests/connector_starrocks.rs` can call `render` against a live engine — it
/// is not API surface, hence `#[doc(hidden)]`.
#[doc(hidden)]
pub mod template;

pub use error::{EngineApiError, EngineError};
pub use job::{run_query, JobManager};
pub use routes::{engine_router, rows_to_csv, EngineState, QueryExecutionApi};
pub use store::{Query, QueryStore};
