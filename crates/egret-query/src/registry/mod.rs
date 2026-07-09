//! Database registry: engines, encrypted credentials, per-workspace ownership,
//! connection test. Folded in from the former `egret-database` crate (E4b).

mod error;
pub mod model;
mod repo;
mod routes;
pub mod tester;

pub use error::{DatabaseError, DbApiError};
pub use model::Engine;
pub use repo::DatabaseRepo;
pub use routes::{workspace_database_router, DatabaseState};
