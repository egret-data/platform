//! Policy/Ranger seam (E2): Ranger policy storage + versioning (service-defs,
//! services, policies as native RangerPolicy jsonb) AND the Ranger Admin
//! REST-compatible serving surface engines poll. Implements the `PolicyPort`
//! contract (egret-policy-api) that cross-seam callers depend on.
pub mod default_policies;
pub mod error;
mod port_impl;
pub mod ranger;
pub mod service_def;
pub mod store;
pub use error::AuthzError;
pub use ranger::{ranger_router, RangerState};
pub use service_def::{seed_trino, trino_service_def, TRINO_DEF_NAME};
pub use store::{AuthzStore, PolicyRow};
