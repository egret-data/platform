//! HTTP state + the Ranger Admin `ranger_router()` (axum 0.8).

use super::api;
use crate::PolicyStore;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

#[derive(Clone)]
pub struct RangerState {
    pub store: PolicyStore,
    pub auth_token: Arc<str>,
}

pub fn ranger_router(state: RangerState) -> Router {
    Router::new()
        .route(
            "/plugins/policies/download/{service}",
            get(api::download::download_policies),
        )
        .route(
            "/plugins/secure/policies/download/{service}",
            get(api::download::download_policies),
        )
        .route(
            "/tags/download/{service}",
            get(api::download::download_tags),
        )
        .merge(api::crud_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::auth::require_bearer,
        ))
        .with_state(state)
}

/// The whole Ranger surface as the server mounts it: `/ranger/service/...`.
pub fn ranger_surface(state: RangerState) -> Router {
    Router::new().nest(
        egret_engine_mgmt_api::ranger_config::RANGER_SURFACE_PREFIX,
        Router::new().nest("/service", ranger_router(state)),
    )
}

// ---------- OpenAPI doc-only mirrors (E0·P4b-5) ----------
//
// The Ranger surface serves arbitrary, verbatim Ranger JSON (service-defs and
// policies stored as jsonb). These `#[derive(ToSchema)]` structs mirror the
// wire envelopes for documentation only — the handlers emit `serde_json::Value`,
// so the shapes here are never constructed. Fields that carry opaque Ranger JSON
// are documented as free-form objects via `#[schema(value_type = ...)]` rather
// than forcing a fake shape onto them.

/// Doc-only mirror of the `ServicePolicies` plugin download envelope
/// (`egret-policy-api`) returned by `download_policies`.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangerServicePolicies {
    pub service_name: String,
    pub service_id: i64,
    pub policy_version: i64,
    /// epoch millis; omitted when unknown
    pub policy_update_time: Option<i64>,
    /// The raw Ranger service-definition (stored verbatim).
    #[schema(value_type = Object)]
    pub service_def: serde_json::Value,
    /// The service's enabled policies, each a raw Ranger policy object.
    #[schema(value_type = Vec<Object>)]
    pub policies: Vec<serde_json::Value>,
    pub audit_mode: String,
}

/// Doc-only mirror of the empty `ServiceTags` payload returned by `download_tags`.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangerServiceTags {
    pub op: String,
    pub service_name: String,
    pub tag_version: i64,
    /// Tag map keyed by numeric tag-id → tag-def instance.
    #[schema(value_type = Object)]
    pub tags: serde_json::Map<String, serde_json::Value>,
    #[schema(value_type = Vec<Object>)]
    pub service_resources: Vec<serde_json::Value>,
    /// `{ "<resourceId>": [<tagId>, ...] }` linking each resource to its tags.
    #[schema(value_type = Object)]
    pub resource_to_tag_ids: serde_json::Map<String, serde_json::Value>,
}

/// Doc-only mirror of the `{ "serviceDefs": [...] }` envelope `list_defs` returns.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangerServiceDefs {
    /// Each element is a raw Ranger service-definition object (stored verbatim).
    #[schema(value_type = Vec<Object>)]
    pub service_defs: Vec<serde_json::Value>,
}

/// Doc-only mirror of one `list_services` entry.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangerServiceRef {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    /// The service's config map (stored verbatim).
    #[schema(value_type = Object)]
    pub configs: serde_json::Value,
    pub policy_version: i64,
    pub is_enabled: bool,
}

/// Doc-only mirror of the Ranger error envelope (`{ msg, statusCode }`), which
/// differs from the control-plane `ErrorBody` shape.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangerErrorBody {
    pub msg: String,
    pub status_code: u16,
}

/// OpenAPI fragment for the Ranger Admin REST serving surface (mounted at
/// `/service`) — the read-only download + service/def/policy GET endpoints the
/// Trino Ranger plugin polls (E2e). Merged into `egret_policy::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        api::download::download_policies,
        api::download::download_tags,
        api::crud::list_defs,
        api::crud::get_def,
        api::crud::list_services,
        api::crud::get_policy,
    ),
    components(schemas(
        RangerServicePolicies,
        RangerServiceTags,
        RangerServiceDefs,
        RangerServiceRef,
        RangerErrorBody,
    )),
    tags((name = "ranger", description = "Ranger Admin REST serving surface polled by engine plugins (read-only policy/service-def download)")),
)]
pub struct RangerApi;
