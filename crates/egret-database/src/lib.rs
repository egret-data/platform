//! Database registry: engines, encrypted credentials, per-workspace ownership, connection test.

mod error;
pub mod model;
mod repo;
mod routes;
pub mod tester;

pub use error::{DatabaseError, DbApiError};
pub use model::Engine;
pub use repo::DatabaseRepo;
pub use routes::{workspace_database_router, DatabaseState};
