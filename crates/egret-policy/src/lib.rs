//! Policy/Ranger seam (E2): Ranger policy storage + versioning (service-defs,
//! services, policies as native RangerPolicy jsonb) AND the Ranger Admin
//! REST-compatible serving surface engines poll. Implements the `PolicyPort`
//! contract (egret-policy-api) that cross-seam callers depend on.
pub mod admin;
pub mod default_policies;
pub mod error;
mod port_impl;
pub mod ranger;
pub mod service_def;
pub mod store;
pub use admin::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyHttpError,
};
pub use error::PolicyStoreError;
pub use ranger::{ranger_router, RangerState};
pub use service_def::{seed_trino, trino_service_def, TRINO_DEF_NAME};
pub use store::{PolicyRow, PolicyStore};
