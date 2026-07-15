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
use serde_json::{json, Value};

/// Synthetic policy ids for the injected baseline policies. Kept far above any DB
/// serial id so they never collide with a stored policy in the download.
const SELF_IMPERSONATION_POLICY_ID: i64 = 900_000_001;
const QUERY_TIMEOUT_PROPERTY_POLICY_ID: i64 = 900_000_002;

/// The system session property egret's Trino connector sets on every statement
/// (`X-Trino-Session: query_max_run_time=<n>s`). A delegated (non-super) user must
/// be allowed to set it or the query is denied before it runs.
const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";

/// Build a policyType-0 allow policy: `{USER}` (every requesting user, on their
/// own request) gets `accesses` on `resources`. Used only for the synthetic
/// baseline policies below.
fn baseline_allow(
    id: i64,
    name: &str,
    service: &str,
    resources: Value,
    accesses: &[&str],
) -> Value {
    let accesses: Vec<Value> = accesses
        .iter()
        .map(|a| json!({ "type": a, "isAllowed": true }))
        .collect();
    json!({
        "id": id,
        "name": name,
        "service": service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": true,
        "version": 1,
        "resources": resources,
        "policyItems": [{
            "users": ["{USER}"],
            "groups": [],
            "roles": [],
            "accesses": accesses,
            "delegateAdmin": false
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isDenyAllElse": false,
        "policyPriority": 0
    })
}

/// The synthetic, always-served baseline allow policies every delegated query
/// needs on a Trino service. Delegated queries run AS the calling user (not a
/// Ranger super-user), so every infrastructure action egret's connector performs
/// must be permitted or the query is denied before any data policy applies:
///
/// 1. **Self-impersonation** — the caller authenticates via their engine JWT and
///    then presents `X-Trino-User = <their own username>`; Trino's apache-ranger
///    plugin treats principal→session-user as impersonation and denies it unless a
///    policy allows it (`Access Denied: Principal X cannot become user X`).
/// 2. **Query-timeout session property** — the connector sets `query_max_run_time`
///    on every statement; setting a system property needs `alter` on
///    `systemproperty` (`Cannot set system session property query_max_run_time`).
///
/// Both scope the resource to the `{USER}` macro (self only) and grant no data
/// access — actual table/column reads still require the user's own grants.
/// Injected at download time (never stored) so they apply to every Trino service.
fn baseline_delegation_policies(service: &str) -> Vec<Value> {
    vec![
        baseline_allow(
            SELF_IMPERSONATION_POLICY_ID,
            "egret-self-impersonation",
            service,
            json!({ "trinouser": { "values": ["{USER}"], "isExcludes": false, "isRecursive": false } }),
            &["impersonate"],
        ),
        baseline_allow(
            QUERY_TIMEOUT_PROPERTY_POLICY_ID,
            "egret-query-timeout-property",
            service,
            json!({ "systemproperty": { "values": [QUERY_TIMEOUT_PROPERTY], "isExcludes": false, "isRecursive": false } }),
            &["alter"],
        ),
    ]
}

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
    let mut policies = s.store.enabled_policies_for_service(svc.id).await?;
    // Trino resource services get the baseline delegated-query allow policies
    // (self-impersonation + query-timeout property; see
    // `baseline_delegation_policies`), or delegated queries are denied before any
    // data policy applies. Tag services (policyType-1/2 masking) don't gate these,
    // so this only applies to the `trino` resource service.
    if svc.r#type == "trino" {
        policies.extend(baseline_delegation_policies(&svc.name));
    }
    // The tag service this resource service is linked to (trino profiles carry
    // `tagService = <name>_tag`), captured before `svc.name` is moved below.
    let tag_service = svc
        .config_jsonb
        .get("tagService")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_policies_allow_self_impersonation_and_query_timeout() {
        let ps = baseline_delegation_policies("rgr2");
        assert_eq!(ps.len(), 2);
        for p in &ps {
            assert_eq!(p["service"], "rgr2");
            assert_eq!(p["policyType"], 0);
            assert_eq!(p["isEnabled"], true);
            assert_eq!(p["policyItems"][0]["users"][0], "{USER}");
            // Non-null id is mandatory — the Ranger policy engine NPEs otherwise.
            assert!(p["id"].as_i64().is_some());
        }
        // 1) self-impersonation: {USER} may impersonate {USER}.
        let imp = &ps[0];
        assert_eq!(imp["id"], SELF_IMPERSONATION_POLICY_ID);
        assert_eq!(imp["resources"]["trinouser"]["values"][0], "{USER}");
        assert_eq!(imp["policyItems"][0]["accesses"][0]["type"], "impersonate");
        assert_eq!(imp["policyItems"][0]["accesses"][0]["isAllowed"], true);
        // 2) query-timeout: {USER} may alter the query_max_run_time system property.
        let prop = &ps[1];
        assert_eq!(prop["id"], QUERY_TIMEOUT_PROPERTY_POLICY_ID);
        assert_eq!(
            prop["resources"]["systemproperty"]["values"][0],
            QUERY_TIMEOUT_PROPERTY
        );
        assert_eq!(prop["policyItems"][0]["accesses"][0]["type"], "alter");
        assert_eq!(prop["policyItems"][0]["accesses"][0]["isAllowed"], true);
        // distinct ids so the plugin doesn't dedup them.
        assert_ne!(ps[0]["id"], ps[1]["id"]);
    }
}
