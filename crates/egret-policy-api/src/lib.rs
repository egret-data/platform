//! Policy/Ranger seam contract (E2 contract-crate seed): the engine-facing wire
//! DTOs + the `PolicyPort` trait that cross-seam callers depend on instead of the
//! concrete `PolicyStore`. Leaf crate — serde-only, no sqlx/axum/egret-* deps.
pub mod dto;
pub mod port;

pub use dto::{PolicyService, ServicePolicies, ServiceTags, TagAssignment};
pub use port::{PolicyError, PolicyPort};
