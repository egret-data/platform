//! Ranger Admin REST-compatible surface: service-def/service/policy CRUD + versioned policy download.
pub mod api;
pub mod auth;
pub mod dto;
pub mod error;
pub mod routes;
pub use routes::{ranger_router, RangerState};
