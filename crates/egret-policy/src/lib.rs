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
pub use service_def::{
    seed_tag, seed_trino, tag_service_def, trino_service_def, TAG_DEF_NAME, TRINO_DEF_NAME,
};
pub use store::{PolicyRow, PolicyStore, ServiceRow, TagAssignmentRow};

/// The Policy/Ranger seam's OpenAPI document (E0·P4b-5): the admin Policies
/// console router (`/api/control-plane`) and the Ranger Admin REST serving
/// surface (`/service`) merged into one `OpenApi`. `egret-server` merges this
/// into the top-level API doc. This is the per-crate `openapi()` convention
/// every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = admin::PolicyAdminApi::openapi();
    doc.merge(ranger::routes::RangerApi::openapi());
    doc
}
