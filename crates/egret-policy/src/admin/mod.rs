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
    tag_editor_to_ranger_json, MaskItemInput, MaskingPolicyInput, RangerPolicy, RowFilterItemInput,
    RowFilterPolicyInput,
};
// SP4a Task 4 (egret-perm's compiler) builds these three directly rather
// than hand-writing Ranger JSON, then leaves rendering to
// `access_to_ranger_json`.
pub use ranger_json::{AccessItemInput, AccessPolicyInput, ResourceLevel};
// SP4a Task 7: `egret-perm-compile`'s recompile driver (`trigger::recompile_service`)
// is the first real caller of `compile_grant` outside this crate's own
// tests, and `compile_grant` returns a typed `AccessPolicyInput`, never
// rendered JSON (the compiler stays free of `serde_json` rendering choices —
// see `egret-perm-compile/src/grants.rs`'s module doc). Something outside
// `egret-policy` has to render it before it can be written to
// `policy.authz_policies.policy_jsonb`, so `access_to_ranger_json` (the plain,
// resource-addressed renderer) is re-exported here too.
pub use ranger_json::access_to_ranger_json;
// **SP4c Task 2 AMENDS SP4a's D2, and this is the site.** Until SP4c,
// `tag_access_to_ranger_json` stayed crate-private and unreachable on the
// strength of D2 ("tags drive rules only, never access"): a tag ACCESS policy
// reintroduces Ranger's tag-first short-circuit, which was the one hazard this
// boundary held the line on.
//
// SP4c's whole subject is a classification's CLEARANCE, and a clearance cannot
// be expressed as either shape D2 permits — a mask and a row filter both still
// ANSWER the query, and a classification exists to WITHHOLD the data. The only
// Ranger shape that withholds "everyone except these people" is a deny with
// `denyExceptions`, keyed on the tag so it follows the tag wherever it is
// assigned. So the amendment is deliberate and narrow: exactly one compiler
// (`egret-perm-compile::clearance`, name prefix
// `COMPILER_CLEARANCE_PREFIX`) emits a tag access policy, and it emits only a
// DENY — never an allow, which is the direction that could short-circuit ahead
// of a compiled deny grant. The tag-first short-circuit is what makes a
// clearance deny WIN over any resource grant, which is precisely what a
// clearance has to do (design spec `2026-09-28-classification-clearance-sp4c-design.md`,
// D1). Hand-written tag access policies stay refused: `apply_diff_in_tx` stamps
// the tag service `origin = 'generated'` (SP4a Important 5), so
// `refuse_if_generated` keeps `create_policy` out of it.
pub use ranger_json::tag_access_to_ranger_json;
// SP4a Task 5 (egret-perm's rule compiler) also needs the tag MASKING and
// ROW-FILTER builders. D2 forbade only `tag_access_to_ranger_json` — a tag
// ACCESS policy reintroduces Ranger's tag-first short-circuit; a tag
// masking/row-filter policy carries no such hazard, and egret already emits
// both through the hand-authored editor path (`tag_editor_to_ranger_json`,
// above). SP4c Task 2 amends that half of D2 — see the
// `tag_access_to_ranger_json` re-export just above.
pub use ranger_json::{tag_masking_to_ranger_json, tag_rowfilter_to_ranger_json};
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
///
/// `pub(crate)`: [`crate::ambiguous_arbitration`] needs the DECISIVE bucket of a
/// mask or row-filter policy, and the two literals `"dataMaskPolicyItems"` /
/// `"rowFilterPolicyItems"` must not be spelled a second time there — a second
/// spelling that drifted would make the collision guard compare `Value::Null`
/// against `Value::Null` and report every pair as agreeing, which is a silent
/// pass rather than a visible break.
pub(crate) fn item_key(policy_type: i64) -> &'static str {
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
    /// Who owns the row (migration 0180): `"operator"`, `"compiler"` or
    /// `"infra"`. The server refuses edits, toggles and deletes of an
    /// `"infra"` row, so the profile page withholds those controls.
    pub managed_by: String,
}

/// Parse a stored Ranger policy body into a `PolicySummary`.
pub fn summarize_policy(
    id: i64,
    service: &str,
    enabled: bool,
    managed_by: &str,
    body: &Value,
) -> PolicySummary {
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
        managed_by: managed_by.to_string(),
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
    /// tag-based masking/row-filters in: the profile's OWN `config.tagService`,
    /// resolved to a real profile. `None` when the profile carries no link or
    /// the link names nothing — the FE shows a "Tags" tab only when this is set.
    ///
    /// **SP4d debt Task 9 fix round:** this no longer falls back to the
    /// `<name>_tag` naming convention. It is the same answer
    /// `usable_tag_service` gives before letting an engine bind, and the same
    /// one the plugin download serves, so a tab is offered only when a tag
    /// policy authored in it could actually be enforced.
    pub tag_service: Option<String>,
    /// `"manual"` (hand-authored, fully editable) or `"generated"` (compiled
    /// by the SP4a permission model from its grants and rules).
    ///
    /// **Exposed because D4's requirement is that the profile says so BEFORE
    /// an edit is overwritten, not after** (whole-branch review, Important 4).
    /// The raw-JSON policy editor still ships in SP4a and is still the only
    /// policy UI, and a `'generated'` profile is now only PARTLY frozen — its
    /// `perm-*` policies are compiler-owned and refuse an edit with a 409,
    /// while foreign policies in the same profile stay editable. Without this
    /// field the editor cannot tell an operator which half they are looking
    /// at, and the first thing they learn is the 409 on save.
    pub origin: String,
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
    // level, and LIVE catalog metadata (real schemas/tables/columns from every
    // warehouse the engine reads) at the deeper levels — so a fresh policy can
    // pick from what actually exists, not only values already used in other
    // policies.
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
        // Every warehouse each bound engine reads, deduped across engines — an
        // engine has one per attached egret catalog since SP3a (SP3c-2 Task 5).
        let mut warehouses: Vec<uuid::Uuid> = Vec::new();
        for e in bound() {
            for w in &e.warehouse_ids {
                if !warehouses.contains(w) {
                    warehouses.push(*w);
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
        .map(|r| summarize_policy(r.id, &svc.name, r.enabled, &r.managed_by, &r.policy_jsonb))
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
    // **SP4d debt Task 9 fix round: the `<name>_tag` CONVENTION FALLBACK IS
    // GONE from here, and this reverses a pinned behaviour deliberately.**
    //
    // This used to build a candidate name — the configured `tagService`, else
    // the `<name>_tag` convention — and offer it if any profile carried that
    // name. Checking existence first meant it could not invent a WRITE target,
    // which is why Task 9 initially kept it. The ruling that removed it rests on
    // two facts rather than on taste:
    //
    // * **A console-created profile can never acquire a `tagService`.**
    //   `frontend/src/api/policies.ts::createProfile(name, type)` sends only
    //   those two fields, and Task 2's sweep established that NO `UPDATE` of
    //   `config_jsonb` exists anywhere in `src/` (re-measured 2026-10-10). So
    //   for every console-created Trino profile the fallback was the only reason
    //   a Tags tab appeared — and the tab led to a workflow that cannot
    //   complete, ever, because `resolve_tag_service_and_body` now requires the
    //   configured link. A tab that leads to a guaranteed 400 is worse than no
    //   tab: it tells a steward the capability exists.
    // * **Its stated purpose was write safety, which the write side no longer
    //   needs from it.**
    //
    // So all five readers of `tagService` now agree, which is the property this
    // guard was costing: `download_policies` (the block it serves),
    // `egret_perm_compile::trigger` (what it compiles), `usable_tag_service`
    // (whether an engine may bind), `resolve_tag_service_and_body` (where a tag
    // policy is filed) and this (whether the tab is offered) all resolve the
    // field the same way. `usable_tag_services` IS the join the first three use.
    //
    // The product gap this exposes — no way to give an existing profile a
    // `tagService` — is plan Task 18, not a regression introduced here.
    let usable_tag: std::collections::HashMap<i64, String> = st
        .store
        .usable_tag_services(&services.iter().map(|s| s.id).collect::<Vec<_>>())
        .await?;
    let profiles = services
        .into_iter()
        .map(|s| {
            // Only Trino profiles carry tag policies, and only when their OWN
            // `config.tagService` resolves to a real profile — the same two
            // halves `usable_tag_service` requires before letting an engine bind.
            let tag_service = if s.r#type == "trino" {
                usable_tag.get(&s.id).cloned()
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
                origin: s.origin,
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
    ///
    /// A `tagService` key is accepted only when `type` is `"trino"` — see the
    /// handler's doc comment.
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
///
/// `config` is stored VERBATIM except for two refusals — the type rule below,
/// and SP4d debt Task 9's value-shape rule: a `tagService` whose value is not a
/// non-empty string names no profile, and four of the five readers of that field
/// go through `as_str()` while the fifth reads `->>`, so the shape produces four
/// different behaviours. See `egret_policy::store`'s
/// `refuse_unnameable_tag_service`.
///
/// This makes it the one reachable surface that
/// could pair a non-Trino `type` with a `tagService` — a state in which every tag
/// policy under the profile is silently inert, because the tag download writes the
/// resource level `schema` and only Trino's service-def has one. `create_service`
/// refuses it (400); see `egret_policy::store`'s
/// `refuse_tag_service_on_non_trino_profile` for the measurement and for the
/// enumeration of every write path that can reach the state.
#[utoipa::path(
    post,
    path = "/api/control-plane/policies/services",
    tag = "policy-admin",
    request_body = CreateProfileBody,
    responses(
        (status = 200, description = "Profile created", body = CreatedProfile),
        (status = 400, description = "Invalid type, missing name, a `tagService` in `config` on a profile whose type is not `trino` (its service-def has no `schema` resource, so every tag policy under it would be silently inert), or a `tagService` whose value is not a non-empty string (it names no policy profile, and the plugin download would carry no `tagPolicies` block)", body = ErrorBody),
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
        (status = 409, description = "Profile is still enforced by an engine, or is compiled by the SP4a permission model (generated) and cannot be deleted through the editor", body = ErrorBody),
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
        (status = 400, description = "The profile is not of type trino; the default set is Trino-shaped and its engine would reject it. Nothing is written", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:seed permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "The profile is compiled by the SP4a permission model (generated) and cannot be hand-edited", body = ErrorBody),
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
    /// The value written into the raw-JSON Ranger policy editor (user email /
    /// group name). Deliberately unchanged by `username` below — that editor
    /// surface is Ranger-native and an email can be the right value there.
    pub name: String,
    /// Display label.
    pub label: String,
    /// The user's allocated bare username (`users.username`, minted by
    /// [`egret_auth::users::UserRepo::ensure_username`]) — this is the value
    /// the engine actually sees (SP4a D7 measured `current_user` as `admin`,
    /// never `admin@egret.local`). `None` for a `group` principal, and for a
    /// `user` principal that has never triggered allocation. Never guess this
    /// by slugifying `name`/`label`: `ensure_username` lowercases, slugifies,
    /// *and* appends `-2`/`-3`/… on collision, which no client can reproduce.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// GET /principals?q=&kind= — egret users + groups for the policy picker.
///
/// Correctness note: `name` is the user's `email`, kept for the raw-JSON
/// Ranger policy editor, which deliberately writes emails. A `user`
/// principal's `username` field carries its allocated `users.username` when
/// one exists — the engine-visible identity — so a caller that needs the
/// engine-native `ref` (e.g. the permission-UI's `PrincipalPicker`) should
/// read `username`, not derive one from `name`. If a deployment's
/// Trino/Ranger group-mapping keys identities on something else entirely, an
/// admin can still free-type a value directly in the raw-JSON editor.
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
        let users = st
            .users
            .list(200, 0)
            .await
            .map_err(|_| PolicyHttpError::Internal)?;
        let ids: Vec<uuid::Uuid> = users.iter().map(|u| u.id.0).collect();
        let usernames = st
            .users
            .usernames(&ids)
            .await
            .map_err(|_| PolicyHttpError::Internal)?;
        for u in users {
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
                    username: usernames.get(&u.id.0).cloned(),
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
                    username: None,
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
/// profile's `config.tagService` link. Returns the tag `ServiceRow` + the
/// serialized policy (body bound to the tag service, `serviceType:"tag"`).
///
/// # SP4d debt Task 9: the `{profile}_tag` fallback is GONE from here
///
/// This used to fall back to the `<profile>_tag` naming convention when the
/// profile carried no usable `tagService`. **Measured: that fallback had no
/// correct-outcome population at all.** It fired exactly when
/// `config.tagService` was absent, JSON `null`, a non-string, or empty — and in
/// every one of those cases `download_policies` reads the same field with
/// `as_str()` and **no fallback**, so it resolves `None` and ships the download
/// with no `tagPolicies` block. The resource plugin therefore never consults the
/// tag policies this path had just filed: every mask and row filter under them
/// silently inert, which is Task 2's symptom on the SUPPORTED profile type.
/// `egret_perm_compile::trigger::recompile_service` reads it the same way and
/// skips with a warning, and `usable_tag_service`'s SQL join reads it the same
/// way and makes `PUT /engines/{id}` refuse the bind. So the fallback's entire
/// firing population was the mis-filing one, and nothing could rely on it for a
/// served outcome.
///
/// Two further findings from that measurement:
///
/// * **It was also Task 2's hole on the editor-write side.** Nothing here checks
///   that `profile` is a `trino` profile. A StarRocks profile can never carry a
///   `tagService` (`refuse_tag_service_on_non_trino_profile` + migration 0176),
///   so the fallback ALWAYS fired for one, and a stranded `{sr}_tag` row —
///   `engines.rs`'s "left by a deleted same-name engine: reuse, never delete" —
///   made the write succeed. Requiring the configured link closes that without a
///   second type check: a non-Trino profile cannot have one.
/// * **`list_profiles` had a SECOND, different fallback — and the fix round
///   removed that one too.** It checked `names.contains(candidate)` before
///   offering the name, so it could not invent a write target, and Task 9
///   initially kept it on that ground. The ruling went the other way once the
///   consequence was stated plainly: nothing can give an existing profile a
///   `tagService` (no `UPDATE` of `config_jsonb` exists in `src/`, and
///   `createProfile` sends no `config`), so the tab that fallback offered led to
///   a workflow that **cannot complete, ever** — a guaranteed 400 from right
///   here. `list_profiles` now resolves through the same join
///   `usable_tag_service` uses; see the comment in that handler.
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
    // Read the field exactly the way `download_policies`, `trigger.rs` and
    // `usable_tag_service` read it — `as_str()`, and a value that names
    // something — so a policy this path stores is one the plugin can be served.
    // No fallback: see this function's doc comment for the measurement.
    let tag_service_name = trino
        .config_jsonb
        .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            PolicyHttpError::BadRequest(format!(
                "policy profile {profile:?} has no `tagService` configured, so a tag policy \
                 authored against it could not be enforced: the plugin download reads that \
                 field with no fallback, and would ship this profile's policies with no \
                 `tagPolicies` block at all — every tag mask and row filter in it silently \
                 inert. Nothing updates an existing profile's config, so the link has to be \
                 there when the profile is created: creating an engine on it auto-binds a \
                 `{profile}_tag` service and writes the link, or `POST \
                 /api/control-plane/policies/services` accepts \
                 `config.tagService` directly."
            ))
        })?
        .to_string();
    let tag_svc = st.store.get_service(&tag_service_name).await?;
    let pol = tag_editor_to_ranger_json(v, &tag_svc.name).map_err(PolicyHttpError::BadRequest)?;
    Ok((tag_svc, pol))
}

/// The tags a MASKING tag policy addresses, or empty if this body is not one
/// — SP4d debt Task 5.
///
/// **Reads the SERIALIZED body, not the editor form**, so the guard keys on
/// exactly what `tagify` wrote and what the engine would download, rather than
/// on a second reading of the request. `tagify` writes one value today
/// (`"A tag policy targets exactly one tag"`); this iterates the array anyway,
/// because a guard that silently inspected only element 0 would fail OPEN the
/// day that comment stops being true.
fn tag_mask_addresses(body: &Value) -> Vec<&str> {
    if body.get("policyType").and_then(Value::as_i64) != Some(1) {
        return Vec::new();
    }
    body.pointer("/resources/tag/values")
        .and_then(Value::as_array)
        .map(|vs| vs.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// **The editor's half of SP4d debt Task 6's one rule.** Would storing `pol`
/// under `service_id` leave that profile serving two mask (or two row-filter)
/// policies that arbitrate one request differently? The rule itself is
/// [`crate::ambiguous_arbitration::ambiguous_arbitration_in`], spelled once and
/// shared with `egret_perm_compile::trigger::compile_desired`; the refusal is
/// the same sentence from both.
///
/// `replacing` is the policy id a `PUT` is about to overwrite, excluded from the
/// comparison set — **without it a `PUT` would pair the new body against the OLD
/// version of itself**, which is a refusal that cannot be escaped (every retry
/// hits the same stored row) and the first thing a reviewer should check here.
///
/// Reads nothing when the body is not a mask or a row filter, which is the
/// common case: an access policy is not in this class at all (its arbitration
/// is `>` and deny-wins makes order immaterial — see the guard's module doc).
/// The read it does do is `enabled_policies_for_service`, the SAME assembler the
/// download route and `explain` use, so the set compared is the set served.
///
/// Runs BEFORE the insert/update, so a refusal has written nothing and the route
/// returns a plain 400 rather than rolling anything back — the same shape
/// `unreadable_table_mask_for`'s callers have.
async fn refuse_ambiguous_arbitration(
    st: &PolicyAdminState,
    service_id: i64,
    replacing: Option<i64>,
    pol: &RangerPolicy,
) -> Result<(), PolicyHttpError> {
    if !matches!(
        pol.body.get("policyType").and_then(Value::as_i64),
        Some(1) | Some(2)
    ) {
        return Ok(());
    }
    let existing: Vec<Value> = st
        .store
        .enabled_policies_for_service(service_id)
        .await?
        .into_iter()
        .filter(|p| replacing.is_none_or(|id| p.get("id").and_then(Value::as_i64) != Some(id)))
        .collect();
    let candidates: Vec<(String, &Value)> = existing
        .iter()
        .map(|p| {
            (
                p.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                p,
            )
        })
        .chain(std::iter::once((pol.name.clone(), &pol.body)))
        .collect();
    if let Some(pair) =
        crate::ambiguous_arbitration::ambiguous_arbitration_in(candidates.iter().map(
            |(name, body)| crate::ambiguous_arbitration::PolicyUnderWrite {
                service: service_id,
                name,
                body,
            },
        ))
    {
        return Err(PolicyHttpError::BadRequest(pair.refusal()));
    }
    Ok(())
}

/// **Every guard a TAG-policy authoring write runs, in one place — SP4d debt
/// Task 16.**
///
/// Task 16's defect was that `PUT /policies/{id}` had no `is_tag_kind` branch at
/// all, so an editor-authored tag mask could be created and deleted but never
/// edited (`editor_to_ranger_json` answered `"unknown policy kind:
/// tag-masking"`). Adding the branch means a second authoring surface for the
/// same shapes, and the thing most likely to go wrong is that the two surfaces
/// end up running different guards. So they run this, and the only argument that
/// differs between them is `replacing`.
///
/// `replacing` is `None` for a create and `Some(id)` for an edit; it reaches
/// [`refuse_ambiguous_arbitration`], which excludes that row from the comparison
/// set — without it a `PUT` pairs the new body against the OLD version of
/// itself and produces a refusal no retry can escape.
///
/// **Task 2's `tagService`-on-non-Trino refusal is reached by construction, not
/// by a call from here.** That guard lives on the profile write
/// (`create_service`, plus migration 0176's `CHECK`), and
/// [`resolve_tag_service_and_body`] now requires a CONFIGURED `tagService` — a
/// thing a non-Trino profile cannot have — so both authoring surfaces refuse a
/// tag policy aimed at one before this function runs. Before Task 9 removed the
/// `{profile}_tag` fallback that was NOT true: the fallback let a tag policy be
/// filed under a StarRocks profile's stranded tag service.
async fn guard_tag_policy_authoring(
    st: &PolicyAdminState,
    tag_service_id: i64,
    replacing: Option<i64>,
    pol: &RangerPolicy,
) -> Result<(), PolicyHttpError> {
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
    // **The editor's half of SP4d debt Task 5's one rule.** A tag MASKING
    // policy whose tag is attached to a whole TABLE makes that table
    // unreadable rather than masked — measured live on Trino 470 — and this
    // is the second surface that can author that half, alongside
    // `POST /api/control-plane/perm/rules`. `egret_bi`'s resource-tag write
    // calls the SAME function from the attachment side; read
    // `crate::unreadable_table_mask` for the measurement and for why there is
    // no migration. Task 16 note: that module's write-path enumeration used to
    // record `PUT /policies/{id}` as unable to reach this shape *because the
    // tag branch did not exist*; it does now, and it is this call that keeps
    // the ruling true.
    for tag in tag_mask_addresses(&pol.body) {
        if let Some(pair) = st
            .store
            .unreadable_table_mask_for_tag_mask(tag, &pol.name)
            .await?
        {
            return Err(PolicyHttpError::BadRequest(pair.refusal()));
        }
    }
    // SP4d debt Task 6: a tag-addressed MASK or ROW FILTER is the same class as
    // a path-addressed one — `tag_masking_to_ranger_json` and
    // `tag_rowfilter_to_ranger_json` delegate to the very builders the resource
    // path uses and only retarget the selector, so the pair is reachable here
    // too. Checked against the TAG service's own set, which is the download the
    // tag plugin polls.
    refuse_ambiguous_arbitration(st, tag_service_id, replacing, pol).await
}

/// **A guard pointed at the wrong policy set is not running** — so a `PUT` must
/// be refused when the service its body NAMES is not the service the row it is
/// replacing actually LIVES under.
///
/// Found by measurement while routing Task 16's new tag branch through the
/// create path's guards, and **pre-existing on the resource-addressed path
/// since Task 6**. `store.update_policy` never writes `service_id`, so a
/// mismatched `service` could not MOVE a policy; what it could do is have every
/// guard in this handler evaluated against a different profile's policy set
/// while the row stayed where it was. Task 6's arbitration guard is the one that
/// matters: name an empty profile and the comparison set is empty, so the pair
/// it exists to refuse is accepted, and the row — still under its original
/// service — is then served alongside the policy it ties with. The mutation
/// `M8-service-mismatch` is the measurement: with this check removed, the
/// bypass succeeds and `put_policies_refuses_a_service_that_is_not_the_policys_own`
/// is the test that dies.
///
/// It also closes an incoherence of its own: the stored body's `service` and
/// `serviceType` fields would name a profile the row does not belong to, and
/// `serviceType` is what `editor_to_ranger_json` stamps from the NAMED
/// profile's type — so a policy stored under a trino profile could carry
/// `serviceType: "starrocks"`.
///
/// Refuses only a hand-made request. The console always names the service the
/// row belongs to: the editor route carries the profile, and a tag policy's row
/// lives under exactly that profile's tag service
/// (`frontend/src/routes/governance/ProfileDetailPage.tsx`'s `editPath` links
/// every Tags-tab row through the main profile for this reason).
fn refuse_service_mismatch(
    row_service_id: i64,
    named: &crate::store::ServiceRow,
    id: i64,
) -> Result<(), PolicyHttpError> {
    if row_service_id == named.id {
        return Ok(());
    }
    Err(PolicyHttpError::BadRequest(format!(
        "policy {id} belongs to policy profile id {row_service_id}, but this body targets \
         {:?} (id {}). A policy cannot be moved between profiles by editing it — delete it \
         and create it under the other profile. Send the profile the policy already belongs \
         to; for a tag policy that is the trino profile whose `tagService` names its tag \
         profile.",
        named.name, named.id
    )))
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
        (status = 400, description = "Invalid editor input, missing policy name, a tag policy whose profile has no configured `tagService` (the plugin download would carry no `tagPolicies` block, so the policy could never be enforced), a tag masking policy whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked), or a mask/row-filter policy that would be served alongside another one selecting the same resource at the same priority with different items (which of the two the engine applies is not expressed by the artifact)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:create permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "A policy with this name already exists, or the target profile is compiled by the SP4a permission model (generated) and cannot be hand-edited", body = ErrorBody),
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
        // **SP4d debt Task 6's concern 5, taken here.** `store.create_policy`
        // makes its refusals INSIDE its own transaction, i.e. after the editor
        // guards below, so a steward adding a hand-written policy to a
        // compiler-owned profile was told "this would be ambiguous" (400) when
        // the operative fact is "this profile is compiled, edit its grants and
        // rules" (409). `preflight_policy_insert` runs the SAME function in the
        // SAME order on a pooled connection — not a second reading of `origin`,
        // which is what Task 6 declined to write, and not a second ORDERING
        // either: lifting `refuse_if_generated` alone inverted the ruling that
        // a reserved NAME wins over a compiled PROFILE, and
        // `generated_profile_guard` caught it.
        st.store.preflight_policy_insert(svc.id, &pol.name).await?;
        guard_tag_policy_authoring(&st, svc.id, None, &pol).await?;
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
    // Task 6's concern 5 again, on the resource-addressed half — see the tag
    // branch above for why the shared pre-flight and not a second `origin` read.
    st.store.preflight_policy_insert(svc.id, &pol.name).await?;
    // SP4d debt Task 6, item 1 — the editor's half. See
    // `refuse_ambiguous_arbitration`.
    refuse_ambiguous_arbitration(&st, svc.id, None, &pol).await?;
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
///
/// # SP4d debt Task 16: the tag branch
///
/// Until Task 16 this handler had no `is_tag_kind` branch, so a `tag-*` form
/// reached `editor_to_ranger_json` and came back `"unknown policy kind:
/// tag-masking"`. `create_policy` had the branch and `delete_policy_handler`
/// needs none, so an editor-authored tag mask or tag row filter could be
/// **created and deleted but never edited** — reachable straight from the UI,
/// which sends `updatePolicy(id, {kind: "tag-masking", service: <trino
/// profile>, …})` whenever the Tags tab's row link is followed
/// (`frontend/src/routes/governance/PolicyEditorPage.tsx`, `buildPayload`).
///
/// That gap is why `crate::unreadable_table_mask`'s refusal named only two
/// remedies: "edit the policy to address it by path" was not available, so the
/// real remedy for an editor-origin pair was delete-and-recreate — a workflow
/// nobody would write down. The branch and that third remedy land together.
///
/// Both authoring verbs run the same guards through
/// [`guard_tag_policy_authoring`]; the one argument that differs is
/// `replacing: Some(id)`, so the arbitration guard does not pair the new body
/// against the row it is replacing.
#[utoipa::path(
    put,
    path = "/api/control-plane/policies/{id}",
    tag = "policy-admin",
    params(("id" = i64, Path, description = "Policy id")),
    request_body = Object,
    responses(
        (status = 200, description = "Policy replaced", body = CreatedPolicyId),
        (status = 400, description = "Invalid editor input, missing policy name, a `service` that is not the profile this policy belongs to (a policy cannot be moved between profiles by editing it, and every guard here would otherwise be evaluated against the wrong profile's policy set), a tag policy whose profile has no configured `tagService`, a tag masking policy whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked), or a mask/row-filter body that would be served alongside another policy selecting the same resource at the same priority with different items (which of the two the engine applies is not expressed by the artifact)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited", body = ErrorBody),
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
    // The row this `PUT` replaces, read before anything else, so both branches
    // can check the body's `service` against the profile the policy actually
    // lives under — see `refuse_service_mismatch`. 404s here for an unknown id,
    // which is the status this route already answered for one.
    let row = st.store.get_policy(id).await?;

    // SP4d debt Task 16 — mirrors `create_policy`'s branch. The form's
    // `service` is the trino PROFILE (that is what the editor route carries and
    // what the Tags tab links through), and the row itself lives under that
    // profile's tag service, so the arbitration set is the tag service's own.
    //
    // `refuse_if_generated` is deliberately NOT lifted here, unlike on the
    // create path: SP4a Task 8 fix round 2 (I-1) moved the EDIT/DELETE guard
    // from profile scope to policy scope precisely because a profile-wide
    // refusal on an edit built a one-way door. `store.update_policy`'s
    // `refuse_if_compiler_owned` is the right guard and it runs inside the
    // write transaction.
    if is_tag_kind(&v) {
        let (svc, pol) = resolve_tag_service_and_body(&st, &v).await?;
        refuse_service_mismatch(row.service_id, &svc, id)?;
        // SP4d-3 Task 2: the infrastructure refusals win over the authoring
        // guards, as `preflight_policy_insert` does on the create path.
        st.store.preflight_policy_update(id, &pol.name).await?;
        guard_tag_policy_authoring(&st, svc.id, Some(id), &pol).await?;
        st.store
            .update_policy(id, &pol.name, &pol.body, pol.is_enabled)
            .await?;
        return Ok(Json(json!({ "id": id })));
    }

    let service_name = require_service(&v)?;
    // Resolve the target service's Ranger type (and validate it exists) so the
    // serialized body carries the correct `serviceType` (trino|starrocks).
    let svc = st.store.get_service(service_name).await?;
    refuse_service_mismatch(row.service_id, &svc, id)?;
    let pol = editor_to_ranger_json(&v, &svc.r#type).map_err(PolicyHttpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
    st.store.preflight_policy_update(id, &pol.name).await?;
    // SP4d debt Task 6, item 1 — the editor's half, with `replacing: Some(id)`
    // so the new body is not compared against the row it is replacing.
    refuse_ambiguous_arbitration(&st, svc.id, Some(id), &pol).await?;
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
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited", body = ErrorBody),
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
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited", body = ErrorBody),
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
        let s = summarize_policy(42, "trino", true, "operator", &body);
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
            summarize_policy(1, "trino", true, "operator", &mask).policy_type,
            "masking"
        );
        let rf = json!({"name":"r","policyType":2,"rowFilterPolicyItems":[
            {"users":["bob"],"accesses":[{"type":"select"}]}]});
        assert_eq!(
            summarize_policy(2, "trino", true, "operator", &rf).policy_type,
            "rowfilter"
        );
    }
}
