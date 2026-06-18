//! Database registry: engines, encrypted credentials, workspace grants, connection test.

mod error;
mod grants;
pub mod model;
mod repo;
mod routes;
pub mod tester;

pub use error::{DatabaseError, DbApiError};
pub use model::Engine;
pub use repo::DatabaseRepo;
pub use routes::{admin_database_router, me_database_router, DatabaseState};
