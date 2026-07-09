//! `egret-query` — the query seam: the database registry (engines + encrypted
//! credentials, per-workspace ownership, connection test) and the async query
//! engine (Trino/StarRocks connectors, job manager, result store).
//!
//! Merged from the former `egret-database` (registry) and `egret-engine`
//! (execution) crates (E4b). Folding them into one crate lets the registry's
//! connection `tester` reuse the execution `Connector` for its `SELECT 1`
//! probe instead of re-implementing the Trino/StarRocks clients — which would
//! otherwise require a `database` → `engine` → `database` dependency cycle.

pub mod execution;
pub mod registry;

// Flat re-exports so external call sites reach the public API via `egret_query::…`
// (the same surface the two former crates exposed). Sub-module paths
// (`egret_query::execution::…`, `egret_query::registry::…`) remain available too.
pub use execution::{
    engine_router, rows_to_csv, EngineApiError, EngineError, EngineState, JobManager, Query,
    QueryStore,
};
pub use registry::{
    workspace_database_router, DatabaseError, DatabaseRepo, DatabaseState, DbApiError, Engine,
};
