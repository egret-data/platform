//! Control-plane Policies router (`/policies`), nested under `/api/control-plane`.
//!
//! Read/parse Ranger policies from the shared `AuthzStore` into typed summaries
//! for the admin UI. Ranger stores three policy kinds by `policyType`:
//! 0 = access (`policyItems`), 1 = masking (`dataMaskPolicyItems`),
//! 2 = row-filter (`rowFilterPolicyItems`). P1 is read-only.

mod ranger_json;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;
// Unused until P2 Task 2 wires the create/update endpoints that call these.
#[allow(unused_imports)]
pub use ranger_json::{access_to_ranger_json, AccessPolicyInput};

/// Human-readable name for a Ranger `policyType` integer.
fn policy_type_name(t: i64) -> &'static str {
    match t {
        1 => "masking",
        2 => "rowfilter",
        _ => "access",
    }
}

/// The item array key for each policy kind.
fn item_key(policy_type: i64) -> &'static str {
    match policy_type {
        1 => "dataMaskPolicyItems",
        2 => "rowFilterPolicyItems",
        _ => "policyItems",
    }
}

/// Flattened, UI-facing view of one stored Ranger policy.
#[derive(Debug, Serialize)]
pub struct PolicySummary {
    pub id: i64,
    pub name: String,
    pub service: String,
    pub policy_type: String,
    pub resource_path: String,
    pub principals: Vec<String>,
    pub permissions: Vec<String>,
    pub is_enabled: bool,
}

/// Parse a stored Ranger policy body into a `PolicySummary`.
pub fn summarize_policy(id: i64, service: &str, enabled: bool, body: &Value) -> PolicySummary {
    let policy_type = body.get("policyType").and_then(Value::as_i64).unwrap_or(0);

    // resource_path = catalog.schema.table.column, joining the first value of each level present.
    let res = body.get("resources").and_then(Value::as_object);
    let level = |k: &str| -> Option<String> {
        res.and_then(|r| r.get(k))
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let resource_path = ["catalog", "schema", "table", "column"]
        .iter()
        .filter_map(|k| level(k))
        .collect::<Vec<_>>()
        .join(".");

    // principals + permissions collected across this kind's items.
    let mut principals: Vec<String> = Vec::new();
    let mut permissions: Vec<String> = Vec::new();
    if let Some(items) = body.get(item_key(policy_type)).and_then(Value::as_array) {
        for it in items {
            for key in ["users", "groups", "roles"] {
                if let Some(arr) = it.get(key).and_then(Value::as_array) {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            if !principals.contains(&s.to_string()) {
                                principals.push(s.to_string());
                            }
                        }
                    }
                }
            }
            if let Some(acc) = it.get("accesses").and_then(Value::as_array) {
                for a in acc {
                    if let Some(t) = a.get("type").and_then(Value::as_str) {
                        if !permissions.contains(&t.to_string()) {
                            permissions.push(t.to_string());
                        }
                    }
                }
            }
        }
    }

    PolicySummary {
        id,
        name: body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        service: service.to_string(),
        policy_type: policy_type_name(policy_type).to_string(),
        resource_path,
        principals,
        permissions,
        is_enabled: enabled,
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub service: String,
    #[serde(default)]
    pub r#type: Option<String>, // access | masking | rowfilter
    #[serde(default)]
    pub q: Option<String>,
}

/// A profile (Ranger service) + the engine(s) that link to it.
#[derive(Debug, Serialize)]
pub struct ProfileDto {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub policy_version: i64,
    pub engines: Vec<ProfileEngine>,
}

#[derive(Debug, Serialize)]
pub struct ProfileEngine {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/policies", get(list_policies))
        .route("/policies/{id}", get(get_policy))
        .route("/policies/services", get(list_profiles))
        .route("/policies/service-def/{name}", get(get_service_def))
}

/// GET /policies?service=&type=&q= — policy summaries for one profile, filtered.
async fn list_policies(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<ListQuery>,
) -> Result<Json<Vec<PolicySummary>>, CpError> {
    require_platform_admin(&p)?;
    let svc = st.authz.get_service(&qp.service).await?;
    let mut out: Vec<PolicySummary> = st
        .authz
        .list_policies(svc.id)
        .await?
        .into_iter()
        .map(|r| summarize_policy(r.id, &svc.name, r.enabled, &r.policy_jsonb))
        .collect();
    if let Some(t) = qp.r#type.as_deref() {
        out.retain(|s| s.policy_type == t);
    }
    if let Some(q) =
        qp.q.as_deref()
            .map(str::to_lowercase)
            .filter(|s| !s.is_empty())
    {
        out.retain(|s| {
            s.name.to_lowercase().contains(&q) || s.resource_path.to_lowercase().contains(&q)
        });
    }
    Ok(Json(out))
}

/// GET /policies/{id} — the raw stored Ranger body (with id + isEnabled), for detail.
async fn get_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    let row = st.authz.get_policy(id).await?;
    let mut body = row.policy_jsonb;
    if let Value::Object(m) = &mut body {
        m.insert("id".into(), Value::from(row.id));
        m.insert("isEnabled".into(), Value::from(row.enabled));
    }
    Ok(Json(body))
}

/// GET /policies/services — profiles + the engine(s) that link to each.
async fn list_profiles(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<ProfileDto>>, CpError> {
    require_platform_admin(&p)?;
    let engines = st.storage.list_engines().await?;
    let profiles = st
        .authz
        .list_services()
        .await?
        .into_iter()
        .map(|s| ProfileDto {
            engines: engines
                .iter()
                .filter(|e| e.authz_service_id == Some(s.id))
                .map(|e| ProfileEngine {
                    id: e.id,
                    name: e.name.clone(),
                    status: e.status.clone(),
                })
                .collect(),
            id: s.id,
            name: s.name,
            r#type: s.r#type,
            policy_version: s.policy_version,
        })
        .collect();
    Ok(Json(profiles))
}

/// GET /policies/service-def/{name} — the def slices the editor renders from.
async fn get_service_def(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    Ok(Json(st.authz.get_service_def(&name).await?.def_jsonb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarize_access_policy() {
        let body = json!({
            "name": "allow-analysts-select",
            "policyType": 0,
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]}
            },
            "policyItems": [
                {"users": ["alice"], "groups": ["analysts"], "accesses": [{"type": "select"}]}
            ]
        });
        let s = summarize_policy(42, "trino", true, &body);
        assert_eq!(s.id, 42);
        assert_eq!(s.name, "allow-analysts-select");
        assert_eq!(s.policy_type, "access");
        assert_eq!(s.resource_path, "iceberg.sales.orders.*");
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.principals.contains(&"analysts".to_string()));
        assert_eq!(s.permissions, vec!["select".to_string()]);
        assert!(s.is_enabled);
    }

    #[test]
    fn summarize_masking_and_rowfilter_types() {
        let mask = json!({"name":"m","policyType":1,"dataMaskPolicyItems":[
            {"groups":["pii"],"accesses":[{"type":"select"}]}]});
        assert_eq!(
            summarize_policy(1, "trino", true, &mask).policy_type,
            "masking"
        );
        let rf = json!({"name":"r","policyType":2,"rowFilterPolicyItems":[
            {"users":["bob"],"accesses":[{"type":"select"}]}]});
        assert_eq!(
            summarize_policy(2, "trino", true, &rf).policy_type,
            "rowfilter"
        );
    }
}
