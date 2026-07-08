//! Policy download + tags download endpoints (Ranger plugin polling protocol).

use crate::ranger::dto::{ServicePolicies, ServiceTags};
use crate::ranger::error::RangerError;
use crate::ranger::routes::RangerState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct DownloadParams {
    #[serde(rename = "lastKnownVersion")]
    pub last_known_version: Option<i64>,
}

/// `GET /plugins/policies/download/{service}?lastKnownVersion=N`
///
/// Returns 304 when the plugin's cached version matches; 200 + `ServicePolicies` otherwise.
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
    let sp = ServicePolicies::new(svc.name, svc.id, svc.policy_version, def, policies, None);
    Ok(Json(sp).into_response())
}

/// `GET /tags/download/{service}?lastKnownVersion=N`
///
/// Returns an empty well-formed tag payload so the plugin doesn't error (tag policies deferred).
pub async fn download_tags(
    State(s): State<RangerState>,
    Path(service): Path<String>,
    Query(_q): Query<DownloadParams>,
) -> Result<Response, RangerError> {
    let _ = s.store.get_service(&service).await?; // 404 if unknown
    Ok(Json(ServiceTags::empty(service)).into_response())
}
