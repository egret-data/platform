//! Control-plane Policies router (`/policies`), nested under `/api/control-plane`.
//!
//! Read/parse Ranger policies from the shared `AuthzStore` into typed summaries
//! for the admin UI. Ranger stores three policy kinds by `policyType`:
//! 0 = access (`policyItems`), 1 = masking (`dataMaskPolicyItems`),
//! 2 = row-filter (`rowFilterPolicyItems`). P1 is read-only.

mod ranger_json;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::require_permission;
use crate::error::CpError;
use crate::CpState;
pub use ranger_json::{
    editor_to_ranger_json, masking_to_ranger_json, rowfilter_to_ranger_json, MaskingPolicyInput,
    RangerPolicy, RowFilterPolicyInput,
};

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
    pub description: String,
    pub labels: Vec<String>,
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
    let mut resource_path = ["catalog", "schema", "table", "column"]
        .iter()
        .filter_map(|k| level(k))
        .collect::<Vec<_>>()
        .join(".");
    // Non-table resources (trinouser, function, procedure, queryid, …) don't fit
    // the catalog.schema.table.column path — surface them as "type: value" so the
    // list view isn't blank for e.g. impersonation policies.
    if resource_path.is_empty() {
        if let Some(obj) = res {
            for (k, v) in obj {
                if let Some(val) = v
                    .get("values")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
                {
                    resource_path = format!("{k}: {val}");
                    break;
                }
            }
        }
    }

    // principals + permissions, unioned across every item bucket this kind can
    // populate. Access policies (0) can carry entries in any of allow/deny
    // items and allow/deny exceptions — a deny-only or exceptions-only policy
    // must still surface its principals/permissions, not just policyItems.
    // Masking/row-filter keep their single item array.
    let ik = item_key(policy_type);
    let buckets: &[&str] = if policy_type == 0 {
        &[
            "policyItems",
            "denyPolicyItems",
            "allowExceptions",
            "denyExceptions",
        ]
    } else {
        std::slice::from_ref(&ik)
    };
    let mut principals: Vec<String> = Vec::new();
    let mut permissions: Vec<String> = Vec::new();
    for bk in buckets {
        if let Some(items) = body.get(*bk).and_then(Value::as_array) {
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
        description: body
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels: body
            .get("policyLabels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
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
    pub policy_count: i64,
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
        .route("/policies", get(list_policies).post(create_policy))
        .route(
            "/policies/{id}",
            get(get_policy)
                .put(update_policy)
                .delete(delete_policy_handler),
        )
        .route(
            "/policies/{id}/enabled",
            axum::routing::put(set_policy_enabled),
        )
        .route("/policies/services", get(list_profiles))
        .route(
            "/policies/services/{service}/trino-config",
            get(trino_config),
        )
        .route(
            "/policies/services/{service}/seed-defaults",
            axum::routing::post(seed_defaults),
        )
        .route("/policies/service-def/{name}", get(get_service_def))
        .route("/policies/resource-lookup", get(resource_lookup))
        .route("/principals", get(list_principals))
}

#[derive(Debug, Deserialize)]
pub struct ResourceLookupQuery {
    pub service: String,
    pub level: String, // catalog | schema | table | column
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub table: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
}

/// The `values` array of one resource-level entry (e.g. `resources.schema.values`).
fn values_at(res: Option<&serde_json::Map<String, Value>>, k: &str) -> Vec<String> {
    res.and_then(|r| r.get(k))
        .and_then(|v| v.get("values"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// GET /policies/resource-lookup — suggestions for one resource level.
/// catalog: engine-catalog names of engines bound to the profile + values used
/// in the service's existing policies. schema/table/column: distinct values used
/// in the service's policies, filtered so parent levels match.
async fn resource_lookup(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<ResourceLookupQuery>,
) -> Result<Json<Vec<String>>, CpError> {
    require_permission(&p, "policy:read")?;
    let svc = st.authz.get_service(&qp.service).await?;
    let mut out: Vec<String> = Vec::new();

    if qp.level == "catalog" {
        if let Ok(engines) = st.storage.list_engines().await {
            for e in engines
                .iter()
                .filter(|e| e.authz_service_id == Some(svc.id))
            {
                if let Ok(cats) = st.storage.list_engine_catalogs(e.id).await {
                    for c in cats {
                        if !out.contains(&c.name) {
                            out.push(c.name);
                        }
                    }
                }
            }
        }
    }

    for row in st.authz.list_policies(svc.id).await? {
        let res = row.policy_jsonb.get("resources").and_then(Value::as_object);
        let parent_ok = |k: &str, want: &Option<String>| -> bool {
            match want {
                None => true,
                Some(w) => values_at(res, k).iter().any(|v| v == w || v == "*"),
            }
        };
        if qp.level != "catalog" && !parent_ok("catalog", &qp.catalog) {
            continue;
        }
        if (qp.level == "table" || qp.level == "column") && !parent_ok("schema", &qp.schema) {
            continue;
        }
        if qp.level == "column" && !parent_ok("table", &qp.table) {
            continue;
        }
        for v in values_at(res, &qp.level) {
            if v != "*" && !out.contains(&v) {
                out.push(v);
            }
        }
    }

    if let Some(q) =
        qp.q.as_deref()
            .map(str::to_lowercase)
            .filter(|s| !s.is_empty())
    {
        out.retain(|v| v.to_lowercase().contains(&q));
    }
    out.sort();
    out.truncate(50);
    Ok(Json(out))
}

/// GET /policies?service=&type=&q= — policy summaries for one profile, filtered.
async fn list_policies(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<ListQuery>,
) -> Result<Json<Vec<PolicySummary>>, CpError> {
    require_permission(&p, "policy:read")?;
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
    require_permission(&p, "policy:read")?;
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
    require_permission(&p, "policy:read")?;
    let engines = st.storage.list_engines().await?;
    let counts = st.authz.policy_counts().await?;
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
            policy_count: counts.get(&s.id).copied().unwrap_or(0),
            id: s.id,
            name: s.name,
            r#type: s.r#type,
            policy_version: s.policy_version,
        })
        .collect();
    Ok(Json(profiles))
}

#[derive(Debug, Serialize)]
pub struct ConfigFile {
    pub name: String,
    pub content: String,
}

/// GET /policies/services/{service}/trino-config — the two files a Trino
/// coordinator needs to enforce this profile via the native ranger plugin.
async fn trino_config(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<Json<Vec<ConfigFile>>, CpError> {
    require_permission(&p, "policy:read")?;
    let _ = st.authz.get_service(&service).await?; // 404 if the profile doesn't exist
    let base = st.engine_egret_url.trim_end_matches('/');

    let props = format!(
        "access-control.name=ranger\n\
         ranger.service.name={service}\n\
         ranger.plugin.config.resource=ranger-trino-security.xml\n"
    );
    let xml = format!(
        "<?xml version=\"1.0\"?>\n\
         <?xml-stylesheet type=\"text/xsl\" href=\"configuration.xsl\"?>\n\
         <configuration>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.policy.rest.url</name>\n\
         \x20   <value>{base}</value>\n\
         \x20 </property>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.service.name</name>\n\
         \x20   <value>{service}</value>\n\
         \x20 </property>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.policy.pollIntervalMs</name>\n\
         \x20   <value>30000</value>\n\
         \x20 </property>\n\
         </configuration>\n"
    );
    Ok(Json(vec![
        ConfigFile {
            name: "access-control.properties".into(),
            content: props,
        },
        ConfigFile {
            name: "ranger-trino-security.xml".into(),
            content: xml,
        },
    ]))
}

/// POST /policies/services/{service}/seed-defaults — seed default "admin has
/// all access" policies (one per Trino resource hierarchy) for a profile,
/// mirroring Ranger's default-policy-on-service-create behavior. Idempotent:
/// a no-op (created: 0) if the service already has any policy.
async fn seed_defaults(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<Json<Value>, CpError> {
    require_permission(&p, "policy:seed")?;
    let svc = st.authz.get_service(&service).await?;
    let created = st
        .authz
        .seed_default_trino_policies(svc.id, &service, "admin")
        .await?;
    Ok(Json(json!({ "created": created })))
}

/// GET /policies/service-def/{name} — the def slices the editor renders from.
/// The frontend passes a SERVICE name; resolve it to its service-def (by the
/// service's `type`). Falls back to treating `name` as a def name directly
/// (back-compat) when no such service exists.
async fn get_service_def(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, CpError> {
    require_permission(&p, "policy:read")?;
    let def_name = match st.authz.get_service(&name).await {
        Ok(svc) => svc.r#type,
        Err(_) => name,
    };
    Ok(Json(st.authz.get_service_def(&def_name).await?.def_jsonb))
}

#[derive(Debug, Deserialize)]
pub struct PrincipalQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>, // "user" | "group"
}

/// One egret identity offered by the policy picker.
#[derive(Debug, Serialize)]
pub struct Principal {
    pub kind: &'static str,
    /// The value written into the policy (user email / group name).
    pub name: String,
    /// Display label.
    pub label: String,
}

/// GET /principals?q=&kind= — egret users + groups for the policy picker.
///
/// Correctness note: the user principal `name` is the user's `email`; if a
/// deployment's Trino/Ranger group-mapping keys identities on something else
/// (e.g. a username), the admin must free-type the correct value instead.
async fn list_principals(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<PrincipalQuery>,
) -> Result<Json<Vec<Principal>>, CpError> {
    require_permission(&p, "policy:read")?;
    let q = qp.q.unwrap_or_default().to_lowercase();
    let want_users = qp.kind.as_deref() != Some("group");
    let want_groups = qp.kind.as_deref() != Some("user");
    let mut out: Vec<Principal> = Vec::new();
    if want_users {
        for u in st.users.list(200, 0).await.map_err(|_| CpError::Internal)? {
            let name = u.email.clone();
            let label = if u.name.is_empty() {
                u.email.clone()
            } else {
                u.name.clone()
            };
            if q.is_empty() || name.to_lowercase().contains(&q) || label.to_lowercase().contains(&q)
            {
                out.push(Principal {
                    kind: "user",
                    name,
                    label,
                });
            }
        }
    }
    if want_groups {
        for g in st.groups.list(200).await.map_err(|_| CpError::Internal)? {
            if q.is_empty() || g.name.to_lowercase().contains(&q) {
                out.push(Principal {
                    kind: "group",
                    name: g.name.clone(),
                    label: g.name,
                });
            }
        }
    }
    out.truncate(50);
    Ok(Json(out))
}

/// POST /policies — create an Access/Masking/Row-filter policy (by `kind`).
///
/// Missing `kind` defaults to "access" (see `editor_to_ranger_json`), so the
/// P2 editor form — which never sent `kind` — stays wire-compatible.
async fn create_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(v): Json<Value>,
) -> Result<Json<Value>, CpError> {
    require_permission(&p, "policy:create")?;
    let pol = editor_to_ranger_json(&v).map_err(CpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    let svc = st.authz.get_service(&pol.service).await?;
    let id = st.authz.create_policy(svc.id, &pol.name, &pol.body).await?;
    if !pol.is_enabled {
        // create_policy inserts with enabled=true (DB default); reflect the form's toggle.
        st.authz
            .update_policy(id, &pol.name, &pol.body, false)
            .await?;
    }
    Ok(Json(json!({ "id": id })))
}

/// PUT /policies/{id} — replace a policy's body (any kind).
async fn update_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>, CpError> {
    require_permission(&p, "policy:update")?;
    let pol = editor_to_ranger_json(&v).map_err(CpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    st.authz
        .update_policy(id, &pol.name, &pol.body, pol.is_enabled)
        .await?;
    Ok(Json(json!({ "id": id })))
}

/// DELETE /policies/{id}
async fn delete_policy_handler(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "policy:delete")?;
    st.authz.delete_policy(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct EnabledBody {
    pub enabled: bool,
}

/// PUT /policies/{id}/enabled — toggle a policy's enabled flag.
async fn set_policy_enabled(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(b): Json<EnabledBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "policy:update")?;
    st.authz.set_policy_enabled(id, b.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
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
