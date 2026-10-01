//! Policy download + tags download endpoints (Ranger plugin polling protocol).

use crate::ranger::dto::{ServicePolicies, ServiceTags, TagAssignment};
use crate::ranger::error::RangerError;
use crate::ranger::routes::{
    RangerErrorBody, RangerServicePolicies, RangerServiceTags, RangerState,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DownloadParams {
    /// The plugin's cached policy version; a match yields `304 Not Modified`.
    #[serde(rename = "lastKnownVersion")]
    pub last_known_version: Option<i64>,
}

/// `GET /plugins/policies/download/{service}?lastKnownVersion=N`
///
/// Returns 304 when the plugin's cached version matches; 200 + `ServicePolicies` otherwise.
#[utoipa::path(
    get,
    path = "/service/plugins/policies/download/{service}",
    tag = "ranger",
    params(
        ("service" = String, Path, description = "Policy profile (Ranger service) name"),
        DownloadParams,
    ),
    responses(
        (status = 200, description = "The service's enabled policies + service-def (plugin download envelope)", body = RangerServicePolicies),
        (status = 304, description = "Not modified — the plugin's lastKnownVersion matches the current policy version"),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 404, description = "No such policy profile", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn download_policies(
    State(s): State<RangerState>,
    Path(service): Path<String>,
    Query(q): Query<DownloadParams>,
) -> Result<Response, RangerError> {
    let svc = s.store.get_service(&service).await?; // 404 if unknown
    if q.last_known_version == Some(svc.policy_version) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let def = s.store.get_service_def(&svc.r#type).await?.def_jsonb;
    let policies = s.store.enabled_policies_for_service(svc.id).await?;
    // The tag service this resource service is linked to (trino profiles carry
    // `tagService = <name>_tag`), captured before `svc.name` is moved below.
    let tag_service = svc
        .config_jsonb
        .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let mut sp = ServicePolicies::new(svc.name, svc.id, svc.policy_version, def, policies, None);
    // Embed the tag service's def + policies as `tagPolicies` so the Trino
    // apache-ranger plugin turns on tag-based masking/row-filter evaluation and
    // its tag enricher. Without this, tag policies download fine on their own
    // service but the resource plugin never consults them → masking silently
    // no-ops. Best-effort: a missing/unreadable tag service just omits the block.
    if let Some(name) = tag_service {
        if let Ok(tsvc) = s.store.get_service(&name).await {
            let tdef = s.store.get_service_def(&tsvc.r#type).await?.def_jsonb;
            let tpolicies = s.store.enabled_policies_for_service(tsvc.id).await?;
            sp.tag_policies = Some(Box::new(ServicePolicies::new(
                tsvc.name,
                tsvc.id,
                tsvc.policy_version,
                tdef,
                tpolicies,
                None,
            )));
        }
    }
    Ok(Json(sp).into_response())
}

/// `GET /tags/download/{service}?lastKnownVersion=N`
///
/// Serves the real resource→tag mapping (`catalog_tag_assignments`) for the
/// service's warehouse(s) as a Ranger `ServiceTags`. Returns 304 when the
/// plugin's cached `tagVersion` matches the current one; 200 otherwise.
#[utoipa::path(
    get,
    path = "/service/tags/download/{service}",
    tag = "ranger",
    params(
        ("service" = String, Path, description = "Policy profile (Ranger service) name"),
        DownloadParams,
    ),
    responses(
        (status = 200, description = "The service's catalog tag assignments (plugin tag-download envelope)", body = RangerServiceTags),
        (status = 304, description = "Not modified — the plugin's lastKnownVersion matches the current tag version"),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 404, description = "No such policy profile", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn download_tags(
    State(s): State<RangerState>,
    Path(service): Path<String>,
    Query(q): Query<DownloadParams>,
) -> Result<Response, RangerError> {
    let svc = s.store.get_service(&service).await?; // 404 if unknown
    if q.last_known_version == Some(svc.tag_version) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let assignments = s
        .store
        .tag_assignments_for_service(svc.id)
        .await?
        .into_iter()
        .map(|r| {
            TagAssignment::new(
                r.catalog,
                r.schema_name,
                r.table_name,
                r.column_name,
                r.tag_id,
                r.tag_name,
            )
        })
        .collect();
    let tags = ServiceTags::from_assignments(svc.name, svc.tag_version, assignments);
    Ok(Json(tags).into_response())
}
