//! Ranger Admin REST-compatible surface: service-def/service/policy CRUD +
//! versioned policy download. Folded into `egret-policy` (E2b) from the former
//! standalone `egret-ranger` crate; it drives the same `PolicyStore` this crate
//! owns, so they now live together as the Policy/Ranger seam.
pub mod api;
pub mod auth;
pub mod dto;
pub mod error;
pub mod routes;
pub use routes::{ranger_router, ranger_surface, RangerState};
