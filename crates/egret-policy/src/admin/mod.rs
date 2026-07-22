//! Admin Policies router (`/policies`, `/principals`), nested under
//! `/api/control-plane` — moved into the Policy/Ranger seam in E2d.
//!
//! Read/parse Ranger policies from the seam's `PolicyStore` into typed summaries
//! for the admin UI. Ranger stores three policy kinds by `policyType`:
//! 0 = access (`policyItems`), 1 = masking (`dataMaskPolicyItems`),
//! 2 = row-filter (`rowFilterPolicyItems`).

mod error;
mod ranger_json;
mod reader;

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::session::SessionService;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use egret_core::http::ErrorBody;

use crate::PolicyStore;
pub use error::PolicyHttpError;
pub use ranger_json::{
    editor_to_ranger_json, is_tag_kind, masking_to_ranger_json, rowfilter_to_ranger_json,
    tag_editor_to_ranger_json, MaskingPolicyInput, RangerPolicy, RowFilterPolicyInput,
};
pub use reader::{EngineBinding, EngineBindingReader, EngineReadError};
use std::sync::Arc;

/// State for the admin Policies router. The `PolicyStore` is in-seam; the engine
/// binding read + the identity repos are cross-seam, wired in the composition root.
#[derive(Clone)]
pub struct PolicyAdminState {
    /// The seam's own policy store (`policy.authz_*`).
    pub store: PolicyStore,
    /// Cross-seam read into the engine seam (which engines bind to which profile).
    pub engines: Arc<dyn EngineBindingReader>,
    /// Core-hub identity lookups for the principal picker.
    pub users: egret_auth::users::UserRepo,
    pub groups: egret_auth::groups::GroupRepo,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// In-cluster URL Trino pods use to reach egret's REST catalog + token
    /// endpoint (e.g. `http://egret:8080`); rendered into the plugin config.
    pub engine_egret_url: String,
    /// Policy decision point: every route gates on `egret_core::authz::require`
    /// (B1b-2). `platform_admin` still bypasses inside the PDP.
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
}

impl FromRef<PolicyAdminState> for SessionService {
    fn from_ref(s: &PolicyAdminState) -> Self {
        s.sessions.clone()
    }
}

/// Build the admin Policies router (to be nested at `/api/control-plane`).
pub fn policy_admin_router() -> Router<PolicyAdminState> {
    router()
}

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
#[derive(Debug, Serialize, utoipa::ToSchema)]
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
    // Tag policies (Access P3.2) key on a single `tag` resource rather than the
    // catalog.schema.table.column path — surface it as "tag: <name>" so the
    // Policies list isn't blank for a tag-based mask/row/access policy.
    if resource_path.is_empty() {
        if let Some(tag) = level("tag") {
            resource_path = format!("tag: {tag}");
        }
    }
    // Other non-table resources (trinouser, function, procedure, queryid, …)
    // don't fit the catalog.schema.table.column path — surface them as
    // "type: value" so the list view isn't blank for e.g. impersonation policies.
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Policy profile (Ranger service) name to list policies for.
    pub service: String,
    /// Optional kind filter: `access` | `masking` | `rowfilter`.
    #[serde(default)]
    pub r#type: Option<String>,
    /// Optional case-insensitive name/resource-path substring filter.
    #[serde(default)]
    pub q: Option<String>,
}

/// A profile (Ranger service) + the engine(s) that link to it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileDto {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub policy_version: i64,
    pub policy_count: i64,
    pub engines: Vec<ProfileEngine>,
    /// The classification-tag policy service this Trino profile carries its
    /// tag-based masking/row-filters in (the configured `tagService` or the
    /// `<name>_tag` convention), if such a service exists. `None` otherwise — the
    /// FE shows a "Tags" tab only when this is set.
    pub tag_service: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileEngine {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
}

pub fn router() -> Router<PolicyAdminState> {
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
        .route(
            "/policies/services",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/policies/services/{service}",
            axum::routing::delete(delete_profile),
        )
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ResourceLookupQuery {
    /// Policy profile (Ranger service) name to draw suggestions from.
    pub service: String,
    /// Resource level to suggest: `catalog` | `schema` | `table` | `column`.
    pub level: String,
    /// Parent catalog filter (for schema/table/column levels).
    #[serde(default)]
    pub catalog: Option<String>,
    /// Parent schema filter (for table/column levels).
    #[serde(default)]
    pub schema: Option<String>,
    /// Parent table filter (for the column level).
    #[serde(default)]
    pub table: Option<String>,
    /// Optional case-insensitive substring filter on the suggestions.
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
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/resource-lookup",
    tag = "policy-admin",
    params(ResourceLookupQuery),
    responses(
        (status = 200, description = "Suggested resource values for the given level (max 50)", body = Vec<String>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn resource_lookup(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Query(qp): Query<ResourceLookupQuery>,
) -> Result<Json<Vec<String>>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let svc = st.store.get_service(&qp.service).await?;
    let mut out: Vec<String> = Vec::new();
    let push_unique = |out: &mut Vec<String>, v: String| {
        if v != "*" && !out.contains(&v) {
            out.push(v);
        }
    };

    // Suggestions from the profile's bound engines: catalog names at the catalog
    // level, and LIVE catalog metadata (real schemas/tables/columns from the
    // engine's warehouse) at the deeper levels — so a fresh policy can pick from
    // what actually exists, not only values already used in other policies.
    let bindings = st.engines.list_engine_bindings().await.unwrap_or_default();
    let bound = || {
        bindings
            .iter()
            .filter(|e| e.authz_service_id == Some(svc.id))
    };
    if qp.level == "catalog" {
        for e in bound() {
            if let Ok(names) = st.engines.engine_catalog_names(e.id).await {
                for name in names {
                    push_unique(&mut out, name);
                }
            }
        }
    } else {
        let mut warehouses: Vec<uuid::Uuid> = Vec::new();
        for e in bound() {
            if let Some(w) = e.warehouse_id {
                if !warehouses.contains(&w) {
                    warehouses.push(w);
                }
            }
        }
        for wh in warehouses {
            let live = match qp.level.as_str() {
                "schema" => st.engines.warehouse_schemas(wh).await,
                "table" => match &qp.schema {
                    Some(s) => st.engines.warehouse_tables(wh, s).await,
                    None => Ok(Vec::new()),
                },
                "column" => match (&qp.schema, &qp.table) {
                    (Some(s), Some(t)) => st.engines.warehouse_columns(wh, s, t).await,
                    _ => Ok(Vec::new()),
                },
                _ => Ok(Vec::new()),
            };
            if let Ok(vals) = live {
                for v in vals {
                    push_unique(&mut out, v);
                }
            }
        }
    }

    for row in st.store.list_policies(svc.id).await? {
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
#[utoipa::path(
    get,
    path = "/api/control-plane/policies",
    tag = "policy-admin",
    params(ListQuery),
    responses(
        (status = 200, description = "Policy summaries for the profile, filtered", body = Vec<PolicySummary>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_policies(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Query(qp): Query<ListQuery>,
) -> Result<Json<Vec<PolicySummary>>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let svc = st.store.get_service(&qp.service).await?;
    let mut out: Vec<PolicySummary> = st
        .store
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
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/{id}",
    tag = "policy-admin",
    params(("id" = i64, Path, description = "Policy id")),
    responses(
        (status = 200, description = "The raw stored Ranger policy body (with id + isEnabled)", body = Object),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_policy(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let row = st.store.get_policy(id).await?;
    let mut body = row.policy_jsonb;
    if let Value::Object(m) = &mut body {
        m.insert("id".into(), Value::from(row.id));
        m.insert("isEnabled".into(), Value::from(row.enabled));
    }
    Ok(Json(body))
}

/// GET /policies/services — profiles + the engine(s) that link to each.
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/services",
    tag = "policy-admin",
    responses(
        (status = 200, description = "Policy profiles with their linked engines and policy counts", body = Vec<ProfileDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_profiles(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
) -> Result<Json<Vec<ProfileDto>>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let engines = st.engines.list_engine_bindings().await?;
    let counts = st.store.policy_counts().await?;
    let services = st.store.list_services().await?;
    // Owned name set so `tag_service` resolution can borrow it while we consume
    // `services` below (can't hold a borrow of `services` across `into_iter`).
    let names: std::collections::HashSet<String> =
        services.iter().map(|s| s.name.clone()).collect();
    let profiles = services
        .into_iter()
        .map(|s| {
            // Only Trino profiles carry tag policies, and only when their tag
            // service (configured `tagService`, else `<name>_tag`) actually
            // exists — otherwise the FE would show an empty, dead Tags tab.
            let tag_service = if s.r#type == "trino" {
                let candidate = s
                    .config_jsonb
                    .get("tagService")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}_tag", s.name));
                names.contains(&candidate).then_some(candidate)
            } else {
                None
            };
            ProfileDto {
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
                tag_service,
            }
        })
        .collect();
    Ok(Json(profiles))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateProfileBody {
    /// Ranger service (profile) name.
    pub name: String,
    /// Ranger service type: `"trino"` or `"starrocks"`.
    pub r#type: String,
    /// Optional service config (e.g. StarRocks JDBC coordinates). Stored verbatim.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Doc-only mirror of the body `create_profile` returns.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedProfile {
    pub id: i64,
    pub name: String,
    pub r#type: String,
}

/// POST /policies/services — create a policy profile (a Ranger service
/// instance) of the given type. The manual analogue of Trino provisioning's
/// auto-bind, so a StarRocks profile can exist without an egret-provisioned
/// engine.
#[utoipa::path(
    post,
    path = "/api/control-plane/policies/services",
    tag = "policy-admin",
    request_body = CreateProfileBody,
    responses(
        (status = 200, description = "Profile created", body = CreatedProfile),
        (status = 400, description = "Invalid type or missing name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:create permission", body = ErrorBody),
        (status = 409, description = "A profile with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_profile(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Json(b): Json<CreateProfileBody>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:create").await?;
    if b.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "profile name is required".into(),
        ));
    }
    if b.r#type != "trino" && b.r#type != "starrocks" {
        return Err(PolicyHttpError::BadRequest(
            "type must be \"trino\" or \"starrocks\"".into(),
        ));
    }
    let cfg = if b.config.is_null() {
        json!({})
    } else {
        b.config.clone()
    };
    let svc = st.store.create_service(&b.name, &b.r#type, &cfg).await?;
    Ok(Json(
        json!({ "id": svc.id, "name": svc.name, "type": svc.r#type }),
    ))
}

/// DELETE /policies/services/{service} — delete a policy profile (Ranger
/// service) and, via `ON DELETE CASCADE`, every policy under it.
///
/// GUARD: refused (409) while an engine still enforces the profile — deleting
/// it out from under a bound engine would silently strip that engine's access
/// control. The admin must unbind (or delete) the engine first. A paired tag
/// service (`<name>_tag`), if any, is a separate profile and is deleted on its
/// own.
#[utoipa::path(
    delete,
    path = "/api/control-plane/policies/services/{service}",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Policy profile (Ranger service) name")),
    responses(
        (status = 204, description = "Profile deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:delete permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "Profile is still enforced by an engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_profile(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<StatusCode, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:delete").await?;
    let svc = st.store.get_service(&service).await?;
    let bound: Vec<String> = st
        .engines
        .list_engine_bindings()
        .await?
        .into_iter()
        .filter(|e| e.authz_service_id == Some(svc.id))
        .map(|e| e.name)
        .collect();
    if !bound.is_empty() {
        return Err(PolicyHttpError::Conflict(format!(
            "profile is enforced by {} engine(s): {} — unbind or delete the engine first",
            bound.len(),
            bound.join(", ")
        )));
    }
    st.store.delete_service(svc.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConfigFile {
    pub name: String,
    pub content: String,
}

/// GET /policies/services/{service}/trino-config — the two files a Trino
/// coordinator needs to enforce this profile via the native ranger plugin.
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/services/{service}/trino-config",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Policy profile (Ranger service) name")),
    responses(
        (status = 200, description = "The Trino coordinator config files for this profile's native ranger plugin", body = Vec<ConfigFile>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn trino_config(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<Json<Vec<ConfigFile>>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let _ = st.store.get_service(&service).await?; // 404 if the profile doesn't exist
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
#[utoipa::path(
    post,
    path = "/api/control-plane/policies/services/{service}/seed-defaults",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Policy profile (Ranger service) name")),
    responses(
        (status = 200, description = "Number of default policies created (0 if the profile already had policies)", body = SeedResult),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:seed permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn seed_defaults(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:seed").await?;
    let svc = st.store.get_service(&service).await?;
    let created = st
        .store
        .seed_default_trino_policies(svc.id, &service, "admin")
        .await?;
    Ok(Json(json!({ "created": created })))
}

/// GET /policies/service-def/{name} — the def slices the editor renders from.
/// The frontend passes a SERVICE name; resolve it to its service-def (by the
/// service's `type`). Falls back to treating `name` as a def name directly
/// (back-compat) when no such service exists.
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/service-def/{name}",
    tag = "policy-admin",
    params(("name" = String, Path, description = "Profile name (resolved to its service-def) or a def name directly")),
    responses(
        (status = 200, description = "The raw Ranger service-definition the editor renders from", body = Object),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such service-def", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_service_def(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let def_name = match st.store.get_service(&name).await {
        Ok(svc) => svc.r#type,
        Err(_) => name,
    };
    Ok(Json(st.store.get_service_def(&def_name).await?.def_jsonb))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PrincipalQuery {
    /// Optional case-insensitive substring filter on name/label.
    #[serde(default)]
    pub q: Option<String>,
    /// Optional kind filter: `user` | `group`.
    #[serde(default)]
    pub kind: Option<String>,
}

/// One egret identity offered by the policy picker.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Principal {
    #[schema(value_type = String)]
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
#[utoipa::path(
    get,
    path = "/api/control-plane/principals",
    tag = "policy-admin",
    params(PrincipalQuery),
    responses(
        (status = 200, description = "egret users + groups for the policy principal picker (max 50)", body = Vec<Principal>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_principals(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Query(qp): Query<PrincipalQuery>,
) -> Result<Json<Vec<Principal>>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let q = qp.q.unwrap_or_default().to_lowercase();
    let want_users = qp.kind.as_deref() != Some("group");
    let want_groups = qp.kind.as_deref() != Some("user");
    let mut out: Vec<Principal> = Vec::new();
    if want_users {
        for u in st
            .users
            .list(200, 0)
            .await
            .map_err(|_| PolicyHttpError::Internal)?
        {
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
        for g in st
            .groups
            .list(200)
            .await
            .map_err(|_| PolicyHttpError::Internal)?
        {
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

/// Doc-only mirror of the `{ "id": <n> }` body the create/update handlers return.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedPolicyId {
    /// The created/updated policy's id.
    pub id: i64,
}

/// Doc-only mirror of the `{ "created": <n> }` body `seed_defaults` returns.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeedResult {
    /// How many default policies were created (0 if the profile already had any).
    pub created: i64,
}

/// Resolve the tag service a tag policy is stored under and build its body.
/// The editor form's `service` is the trino PROFILE; its tag service is that
/// profile's `config.tagService` link, falling back to the `<profile>_tag`
/// naming convention. Returns the tag `ServiceRow` + the serialized policy
/// (body bound to the tag service, `serviceType:"tag"`).
async fn resolve_tag_service_and_body(
    st: &PolicyAdminState,
    v: &Value,
) -> Result<(crate::store::ServiceRow, RangerPolicy), PolicyHttpError> {
    let profile = v
        .get("service")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            PolicyHttpError::BadRequest("tag policy requires a `service` (trino profile)".into())
        })?;
    let trino = st.store.get_service(profile).await?;
    let tag_service_name = trino
        .config_jsonb
        .get("tagService")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{profile}_tag"));
    let tag_svc = st.store.get_service(&tag_service_name).await?;
    let pol = tag_editor_to_ranger_json(v, &tag_svc.name).map_err(PolicyHttpError::BadRequest)?;
    Ok((tag_svc, pol))
}

/// Read the required `service` name from an editor policy body, or 400.
fn require_service(v: &Value) -> Result<&str, PolicyHttpError> {
    v.get("service")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| PolicyHttpError::BadRequest("policy requires a `service`".into()))
}

/// POST /policies — create an Access/Masking/Row-filter policy (by `kind`).
///
/// Missing `kind` defaults to "access" (see `editor_to_ranger_json`), so the
/// P2 editor form — which never sent `kind` — stays wire-compatible.
///
/// The request body is one of the editor input unions — `AccessPolicyInput`
/// (the default when `kind` is absent), `MaskingPolicyInput`, or
/// `RowFilterPolicyInput` — dispatched on `kind`; documented as a free-form
/// object since the handler accepts any of the three shapes.
#[utoipa::path(
    post,
    path = "/api/control-plane/policies",
    tag = "policy-admin",
    request_body = Object,
    responses(
        (status = 200, description = "Policy created", body = CreatedPolicyId),
        (status = 400, description = "Invalid editor input or missing policy name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:create permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "A policy with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_policy(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Json(v): Json<Value>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:create").await?;

    // Tag policies (Access P3.2) are authored against a trino profile but stored
    // under that profile's tag service. Resolve the tag service, build the
    // `serviceType:"tag"` body, and store it there. The generic access/masking/
    // rowfilter path below is unchanged.
    if is_tag_kind(&v) {
        let (svc, pol) = resolve_tag_service_and_body(&st, &v).await?;
        if pol.name.trim().is_empty() {
            return Err(PolicyHttpError::BadRequest(
                "policy name is required".into(),
            ));
        }
        let id = st.store.create_policy(svc.id, &pol.name, &pol.body).await?;
        if !pol.is_enabled {
            st.store
                .update_policy(id, &pol.name, &pol.body, false)
                .await?;
        }
        return Ok(Json(json!({ "id": id })));
    }

    let service_name = require_service(&v)?;
    let svc = st.store.get_service(service_name).await?;
    let pol = editor_to_ranger_json(&v, &svc.r#type).map_err(PolicyHttpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
    let id = st.store.create_policy(svc.id, &pol.name, &pol.body).await?;
    if !pol.is_enabled {
        // create_policy inserts with enabled=true (DB default); reflect the form's toggle.
        st.store
            .update_policy(id, &pol.name, &pol.body, false)
            .await?;
    }
    Ok(Json(json!({ "id": id })))
}

/// PUT /policies/{id} — replace a policy's body (any kind).
#[utoipa::path(
    put,
    path = "/api/control-plane/policies/{id}",
    tag = "policy-admin",
    params(("id" = i64, Path, description = "Policy id")),
    request_body = Object,
    responses(
        (status = 200, description = "Policy replaced", body = CreatedPolicyId),
        (status = 400, description = "Invalid editor input or missing policy name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_policy(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let service_name = require_service(&v)?;
    // Resolve the target service's Ranger type (and validate it exists) so the
    // serialized body carries the correct `serviceType` (trino|starrocks).
    let svc = st.store.get_service(service_name).await?;
    let pol = editor_to_ranger_json(&v, &svc.r#type).map_err(PolicyHttpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
    st.store
        .update_policy(id, &pol.name, &pol.body, pol.is_enabled)
        .await?;
    Ok(Json(json!({ "id": id })))
}

/// DELETE /policies/{id}
#[utoipa::path(
    delete,
    path = "/api/control-plane/policies/{id}",
    tag = "policy-admin",
    params(("id" = i64, Path, description = "Policy id")),
    responses(
        (status = 204, description = "Policy deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:delete permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_policy_handler(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:delete").await?;
    st.store.delete_policy(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EnabledBody {
    pub enabled: bool,
}

/// PUT /policies/{id}/enabled — toggle a policy's enabled flag.
#[utoipa::path(
    put,
    path = "/api/control-plane/policies/{id}/enabled",
    tag = "policy-admin",
    params(("id" = i64, Path, description = "Policy id")),
    request_body = EnabledBody,
    responses(
        (status = 204, description = "Policy enabled flag updated"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_policy_enabled(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(b): Json<EnabledBody>,
) -> Result<StatusCode, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    st.store.set_policy_enabled(id, b.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// OpenAPI fragment for the admin Policies console router (mounted at
/// `/api/control-plane`). Merged into `egret_policy::openapi()`. The
/// `AccessPolicyInput` / `MaskingPolicyInput` / `RowFilterPolicyInput` editor
/// unions are published as exported vocabulary (like the reference seam's
/// `Database`) so the FE gets typed inputs even though the create/update
/// handlers accept any object and dispatch on `kind`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_policies,
        create_policy,
        get_policy,
        update_policy,
        delete_policy_handler,
        set_policy_enabled,
        list_profiles,
        create_profile,
        delete_profile,
        trino_config,
        seed_defaults,
        get_service_def,
        resource_lookup,
        list_principals,
    ),
    components(schemas(
        PolicySummary,
        ProfileDto,
        ProfileEngine,
        CreateProfileBody,
        CreatedProfile,
        ConfigFile,
        Principal,
        EnabledBody,
        CreatedPolicyId,
        SeedResult,
        ranger_json::AccessPolicyInput,
        ranger_json::AccessItemInput,
        ranger_json::ResourceLevel,
        ranger_json::MaskingPolicyInput,
        ranger_json::MaskItemInput,
        ranger_json::RowFilterPolicyInput,
        ranger_json::RowFilterItemInput,
        ErrorBody,
    )),
    tags((name = "policy-admin", description = "Data-governance policy console: access/masking/row-filter policy CRUD, profiles, and the principal picker")),
)]
pub struct PolicyAdminApi;

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
