//! Policy/Ranger seam (E2): Ranger policy storage + versioning (service-defs,
//! services, policies as native RangerPolicy jsonb) AND the Ranger Admin
//! REST-compatible serving surface engines poll. Implements the `PolicyPort`
//! contract (egret-policy-api) that cross-seam callers depend on.
pub mod admin;
pub mod ambiguous_arbitration;
mod auth_failures;
pub mod default_policies;
pub mod error;
// View governance R5 (Task 7 fix round 1): the `egret-lineage` download transform.
pub mod lineage_transform;
mod plugin_downloads;
mod port_impl;
pub mod ranger;
pub mod service_def;
pub mod store;
pub mod unit_overlap;
pub mod unreadable_table_mask;
pub use admin::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyHttpError, ProfileRecompiler,
};
pub use ambiguous_arbitration::{
    ambiguous_arbitration_in, ambiguous_arbitrations_in, resources_may_intersect,
    AmbiguousArbitration, Arbitrated, PolicyUnderWrite,
};
// Declared in the leaf contract crate so `egret-engine-mgmt` can reach it in
// production; re-exported so the canonical path stays `egret_policy::infra_names`.
pub use auth_failures::{auth_failures_via, AuthFailure, DOWNLOAD_WINDOW, FAILURE_WINDOW};
pub use egret_policy_api::infra_names;
pub use error::PolicyStoreError;
pub use plugin_downloads::{
    plugins_hold_policy_version, plugins_hold_tag_version, wait_for_tag_version, wait_for_versions,
    DownloadKind, HoldState, WaitError, NO_PLUGIN_POLLED,
};
pub use ranger::{ranger_router, ranger_surface, RangerState};
pub use service_def::{
    seed_starrocks, seed_tag, seed_trino, starrocks_service_def, tag_service_def,
    trino_service_def, STARROCKS_DEF_NAME, TAG_DEF_NAME, TRINO_DEF_NAME,
};
pub use unit_overlap::{
    compiled_rule_names_via, created_overlaps, load_unit_via, unit_assignments_via, unit_overlaps,
    units_for_service_via, units_for_services_via, AssignedTag, OverlapIdentity, OverlapShape,
    StoredUnit, UnitIds, UnitOverlap, UnitPolicy,
};
pub use unreadable_table_mask::{
    unreadable_table_mask_for, MaskOrigin, TableRef, TagRef, TagWrite, UnreadableTableMask,
    LISTED_OTHER_TABLES,
};

pub use store::{
    catalog_is_assigned_to_workspace_via, catalog_names_for_ids_via, catalogs_for_service_via,
    clearance_gates_for_tags_via, compiler_targeted_services_via, delete_policy_via,
    enabled_policies_for_service_via, engine_binding_profile_via, get_service_by_id_via,
    get_service_def_via, insert_policy_owned_by_via, is_compiler_owned_policy_name,
    lock_service_rows_via, resolved_tags_for_resource_via, service_id_by_name_via,
    services_and_tag_services_via, services_for_catalog_via, set_policy_enabled_via,
    tag_assignment_coordinates_via, tag_assignments_for_catalogs_via,
    tag_assignments_for_service_via, tag_keys_by_ids_via, tag_keys_for_resource_via,
    tag_name_for_key_via, tag_names_for_keys_via, update_policy_via, usable_tag_services_via,
    CatalogRow, PolicyRow, PolicyStore, ResolvedTag, ServiceRow, ServiceTagAssignmentRow,
    TagAssignmentRow, COMPILER_CLEARANCE_PREFIX, COMPILER_ENGINE_GRANT_PREFIX,
    COMPILER_GRANT_PREFIX, COMPILER_HIDDEN_PREFIX, COMPILER_RULE_PREFIX,
};

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
