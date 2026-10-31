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
    /// Recompiles a StarRocks profile inside an editor write that changes one
    /// of its data masks (final review I-3). Not optional: an editor without
    /// it would leave the clearance-mask fold stale.
    pub recompiler: Arc<dyn ProfileRecompiler>,
}

/// Recompiles policy profiles on the caller's open transaction — the seam
/// `egret-perm-compile` fills (this crate cannot depend on it).
///
/// # Why the editor needs it (final review I-3)
///
/// On StarRocks the clearance's priority-1 mask folds, for each cleared user,
/// what the profile's OTHER data masks give that user, hand-written ones
/// included (`egret_perm_compile::clearance_mask`). The fold is computed at
/// compile time, so an editor write that creates, edits, enables, disables or
/// deletes a hand-written data mask on a StarRocks profile must recompile that
/// profile in the same transaction; otherwise the plugin keeps serving the
/// old fold above the edited mask, and a tightened mask stays loose for the
/// cleared users (fail-open) until some unrelated write recompiles.
#[async_trait::async_trait]
pub trait ProfileRecompiler: Send + Sync {
    /// Recompile `service_ids` (ascending) on `conn`. An error refuses the
    /// editor's write, which rolls back with it.
    async fn recompile_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<(), PolicyHttpError>;
}

/// Whether a write to `body` under `service_id` changes what a StarRocks
/// clearance mask folds: a data mask (`policyType` 1) on a `generated`
/// StarRocks profile (final re-review N-2).
///
/// `origin = 'generated'` alone is the test: every compile that writes a
/// clearance policy stamps the profile `generated` in the same transaction
/// (`egret-perm-compile` `trigger.rs`, sticky), so a profile holding a compiled
/// clearance mask is always `generated`. A profile that is not has no clearance
/// mask for the write to leave stale, so refolding would only let an unrelated refusal of the recompile (an
/// expansion ceiling, a stored overlap) block a steward from disabling or
/// deleting a hand-written mask: the way out of that state.
async fn changes_the_clearance_fold(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    body: &Value,
) -> Result<bool, PolicyHttpError> {
    if body.get("policyType").and_then(Value::as_i64) != Some(1) {
        return Ok(false);
    }
    let folds: Option<bool> = sqlx::query_scalar(
        "SELECT type = $2 AND origin = 'generated' \
           FROM policy.authz_services WHERE id = $1",
    )
    .bind(service_id)
    .bind(crate::STARROCKS_DEF_NAME)
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::PolicyStoreError::from)?;
    Ok(folds.unwrap_or(false))
}

/// Whether profile `service_id` is downloaded by an engine that carries an
/// impersonation grant (`perm.engine_grants` with a target). Such a grant's
/// target is re-checked at every compile against every policy of the profile
/// (security re-review M-a), so any hand-written write there must recompile
/// the profile in its own transaction: a policy that makes the target
/// near-root, or gives them a new hop, must reap the grant's `perm-engine-`
/// policy at once, and undoing it must restore the policy.
async fn carries_impersonation_grants(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<bool, PolicyHttpError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.engine_grants g \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
          WHERE e.authz_service_id = $1 AND g.target_user_id IS NOT NULL)",
    )
    .bind(service_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(crate::PolicyStoreError::from)?)
}

/// Lock the `policy.authz_services` rows of every evaluation unit
/// `service_id` belongs to, ascending: the rows a recompile of it takes, in
/// its order, so the editor's write and a concurrent recompile queue.
async fn lock_unit_rows(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(), PolicyHttpError> {
    let mut rows: Vec<i64> = crate::unit_overlap::units_for_service_via(conn, service_id)
        .await?
        .into_iter()
        .flat_map(|u| [u.resource, u.tag_service])
        .flatten()
        .chain([service_id])
        .collect();
    rows.sort_unstable();
    rows.dedup();
    crate::lock_service_rows_via(conn, &rows).await?;
    Ok(())
}

/// The stored `(service_id, policy_jsonb)` of policy `id`, if it exists.
async fn stored_policy(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<Option<(i64, Value)>, PolicyHttpError> {
    Ok(sqlx::query_as::<_, (i64, Value)>(
        "SELECT service_id, policy_jsonb FROM policy.authz_policies WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::PolicyStoreError::from)?)
}

/// Whether removing, disabling or enabling stored policy `id` changes a
/// StarRocks clearance fold, or (security re-review M-a) lands on a profile
/// whose engines carry impersonation grants; when it does, the profile's rows
/// are locked first, in the order a recompile takes them (I-3). Returns the
/// profile to recompile.
async fn fold_profile_of_stored(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<Option<i64>, PolicyHttpError> {
    let Some((service_id, body)) = stored_policy(conn, id).await? else {
        return Ok(None);
    };
    if changes_the_clearance_fold(conn, service_id, &body).await? {
        crate::lock_service_rows_via(conn, &[service_id]).await?;
        return Ok(Some(service_id));
    }
    // Security re-review M-a: impersonation targets are re-checked against
    // every policy of the profile.
    if carries_impersonation_grants(conn, service_id).await? {
        lock_unit_rows(conn, service_id).await?;
        return Ok(Some(service_id));
    }
    Ok(None)
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

    // resource_path = catalog.schema.table.column, joining the first value of each level
    // present. A service names its second level `schema` (Trino) or `database` (StarRocks),
    // never both, so listing both keeps either in position.
    let res = body.get("resources").and_then(Value::as_object);
    let level = |k: &str| -> Option<String> {
        res.and_then(|r| r.get(k))
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut resource_path = ["catalog", "schema", "database", "table", "column"]
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
    /// Policies by owner, the profile's own plus its `tag_service`'s (DG #4a
    /// Q1): `compiler` (egret-compiled), `infra` (egret-seeded), `operator`
    /// (hand-written). `policy_count` stays the profile's own total.
    pub policy_counts: PolicyCounts,
}

/// Policies of a profile by `managed_by` (DG #4a Q1).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct PolicyCounts {
    pub compiler: i64,
    pub infra: i64,
    pub operator: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileEngine {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
    /// `provisioned` or `external` (DG #4a Q1), so a `policy:read` reader
    /// without `engine:read` can tell them apart.
    pub origin: String,
    /// The engine type: `trino`, `starrocks`, … (DG #4a Q1).
    pub r#type: String,
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
            "/policies/services/{service}/tag-service",
            axum::routing::put(set_tag_service),
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
    out.truncate(PRINCIPAL_LIMIT as usize);
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
    let by_owner = st.store.policy_counts_by_owner().await?;
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
    // (Closed by #786: `PUT /policies/services/{service}/tag-service`.)
    let usable_tag: std::collections::HashMap<i64, String> = st
        .store
        .usable_tag_services(&services.iter().map(|s| s.id).collect::<Vec<_>>())
        .await?;
    let ids_by_name: std::collections::HashMap<String, i64> =
        services.iter().map(|s| (s.name.clone(), s.id)).collect();
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
            // The profile plus its tag service, whose id `services` holds.
            let tag_id = tag_service
                .as_ref()
                .and_then(|t| ids_by_name.get(t.as_str()).copied());
            let mut policy_counts = PolicyCounts::default();
            for id in std::iter::once(s.id).chain(tag_id) {
                if let Some((c, i, o)) = by_owner.get(&id) {
                    policy_counts.compiler += c;
                    policy_counts.infra += i;
                    policy_counts.operator += o;
                }
            }
            ProfileDto {
                engines: engines
                    .iter()
                    .filter(|e| e.authz_service_id == Some(s.id))
                    .map(|e| ProfileEngine {
                        id: e.id,
                        name: e.name.clone(),
                        status: e.status.clone(),
                        origin: e.origin.clone(),
                        r#type: e.r#type.clone(),
                    })
                    .collect(),
                policy_count: counts.get(&s.id).copied().unwrap_or(0),
                policy_counts,
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
    /// Ranger service type: `"trino"`, `"starrocks"` or `"tag"`.
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
        (status = 400, description = "Invalid type (must be `trino`, `starrocks` or `tag`), missing name, a `tagService` that names a profile that is missing or not of type `tag`, a `tagService` in `config` on a profile whose type is not `trino` (its service-def has no `schema` resource, so every tag policy under it would be silently inert), or a `tagService` whose value is not a non-empty string (it names no policy profile, and the plugin download would carry no `tagPolicies` block)", body = ErrorBody),
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
    if !matches!(b.r#type.as_str(), "trino" | "starrocks" | "tag") {
        return Err(PolicyHttpError::BadRequest(
            "type must be \"trino\", \"starrocks\" or \"tag\"".into(),
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

/// What `trino-config` answers: the files, and what makes them not work as
/// rendered (KI-149).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TrinoConfigResponse {
    pub files: Vec<ConfigFile>,
    /// The same download-URL warning the external engine's plugin
    /// configuration gives (`egret_engine_mgmt_api::ranger_config::download_url_warning`):
    /// a loopback or unparseable URL no engine can reach. Empty when none.
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetTagServiceBody {
    /// The `tag` profile to name, or `null` to remove the tag service.
    #[serde(rename = "tagService")]
    #[schema(value_type = Option<String>)]
    pub tag_service: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileTagService {
    pub name: String,
    /// The profile's tag service after the write; `null` when it has none.
    #[serde(rename = "tagService")]
    pub tag_service: Option<String>,
}

/// PUT /policies/services/{service}/tag-service — set, change or remove the
/// `config.tagService` of an existing profile (#786).
///
/// A Trino profile made by the console (name and type) had no way to acquire a
/// tag service, so no engine could ever be bound to it ("has no usable tag
/// service"). This is that way, with every guard the create path has (only a
/// `trino` profile may carry one, the value is a non-empty string, it names an
/// existing `tag` profile; see `egret_policy::store::set_tag_service_via`).
///
/// **Conservative ruling: refuse rather than recompile.** A profile an engine
/// downloads is refused (409): changing its tag service swaps the tag policies
/// that engine enforces, and the tag service is checked against an engine at
/// bind time. A profile the permission model compiles (`generated`) is refused
/// too: the compiler files its tag rules into the tag service it reads, and this
/// route does not recompile. Unbind, change, bind again. Recorded as the
/// governance event `profile:tag_service`.
#[utoipa::path(
    put,
    path = "/api/control-plane/policies/services/{service}/tag-service",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Policy profile (Ranger service) name")),
    request_body = SetTagServiceBody,
    responses(
        (status = 200, description = "The profile's tag service after the write. Setting the value it already has writes nothing", body = ProfileTagService),
        (status = 400, description = "The profile is not of type `trino`; the value is not a non-empty string or `null`; or it names a profile that is missing or not of type `tag`. Nothing is written", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "An engine downloads the profile (unbind it first), or egret compiles it (`generated`)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_tag_service(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(service): Path<String>,
    Json(b): Json<SetTagServiceBody>,
) -> Result<Json<ProfileTagService>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
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
            "profile {service} is downloaded by {}: changing its tag service would swap the \
             tag policies that engine enforces. Unbind the engine, change the tag service, then \
             bind it again",
            bound.join(", ")
        )));
    }
    if svc.origin == "generated" {
        return Err(PolicyHttpError::Conflict(format!(
            "profile {service} is compiled by egret's permission model, which files its tag \
             rules into the tag service it names; its tag service is not changed by hand"
        )));
    }
    let mut tx = st.store.begin().await?;
    let changed = crate::store::set_tag_service_via(&mut tx, svc.id, &b.tag_service).await?;
    let after = match changed {
        Some(change) => {
            egret_audit::governance::record_change(
                &mut tx,
                egret_audit::governance::Change {
                    action: egret_audit::governance::action::PROFILE_TAG_SERVICE,
                    resource_type: egret_audit::governance::resource::POLICY_PROFILE,
                    resource_id: svc.id.to_string(),
                    summary: json!({
                        "service": service,
                        "before": { "tagService": change.before },
                        "after": { "tagService": change.after },
                    }),
                    principal: &p,
                },
            )
            .await
            .map_err(crate::PolicyStoreError::from)?;
            change.after
        }
        None => b.tag_service.as_str().map(str::to_string),
    };
    tx.commit().await.map_err(crate::PolicyStoreError::from)?;
    Ok(Json(ProfileTagService {
        name: service,
        tag_service: after,
    }))
}

/// GET /policies/services/{service}/trino-config — the two files a Trino
/// coordinator needs to enforce this profile via the native ranger plugin.
///
/// Rendered by `egret_engine_mgmt_api::ranger_config::ranger_plugin_files`, the
/// same code that writes the files egret mounts into its own engines.
#[utoipa::path(
    get,
    path = "/api/control-plane/policies/services/{service}/trino-config",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Policy profile (Ranger service) name")),
    responses(
        (status = 200, description = "The Trino coordinator config files for this profile's native ranger plugin, from the same renderer that writes the files egret mounts into its own engines (`access-control.properties` and `ranger-trino-security.xml`), and `warnings`: the same download-URL warning as the external engine's plugin configuration (a loopback or unparseable `[catalog] public_url`). Since SP4d-4 the security file also carries the policy source, cache directory and client timeouts, the properties file points at `/etc/trino/ranger-trino-security.xml`, and the poll interval is 30000 ms. Before KI-149 the body was the bare array of files", body = TrinoConfigResponse),
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
) -> Result<Json<TrinoConfigResponse>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let _ = st.store.get_service(&service).await?; // 404 if the profile doesn't exist

    // ONE renderer: the same code that writes the files egret mounts into its own
    // engines (SP4d-4 Task 9). The poll interval is the one shared constant.
    let rendered = egret_engine_mgmt_api::ranger_config::ranger_plugin_files(
        egret_engine_mgmt_api::EngineKind::Trino,
        &service,
        &st.engine_egret_url,
        egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS.into(),
        None,
    );
    Ok(Json(TrinoConfigResponse {
        files: rendered
            .files
            .into_iter()
            .map(|(name, content)| ConfigFile { name, content })
            .collect(),
        warnings: egret_engine_mgmt_api::ranger_config::download_url_warning(&st.engine_egret_url)
            .into_iter()
            .collect(),
    }))
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

/// The picker's cap, applied to each search and to the merged answer.
const PRINCIPAL_LIMIT: i64 = 50;

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
    let q = qp.q.unwrap_or_default();
    let want_users = qp.kind.as_deref() != Some("group");
    let want_groups = qp.kind.as_deref() != Some("user");
    let mut out: Vec<Principal> = Vec::new();
    if want_users {
        let users = st
            .users
            .search(&q, PRINCIPAL_LIMIT)
            .await
            .map_err(|_| PolicyHttpError::Internal)?;
        let ids: Vec<uuid::Uuid> = users.iter().map(|u| u.id.0).collect();
        let usernames = st
            .users
            .usernames(&ids)
            .await
            .map_err(|_| PolicyHttpError::Internal)?;
        for u in users {
            let label = if u.name.is_empty() {
                u.email.clone()
            } else {
                u.name.clone()
            };
            out.push(Principal {
                kind: "user",
                username: usernames.get(&u.id.0).cloned(),
                name: u.email,
                label,
            });
        }
    }
    if want_groups {
        for g in st
            .groups
            .search(&q, PRINCIPAL_LIMIT)
            .await
            .map_err(|_| PolicyHttpError::Internal)?
        {
            out.push(Principal {
                kind: "group",
                name: g.name.clone(),
                label: g.name,
                username: None,
            });
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
    /// Present only when this save stored an emptied resource level as `*`
    /// (SP4d-4 Task 11): the level names, root first.
    #[serde(rename = "filledLevels", skip_serializing_if = "Option::is_none")]
    pub filled_levels: Option<Vec<String>>,
    /// Present with `filledLevels`: what the fill means in Ranger — the policy
    /// now covers every child of those levels, and it moved earlier in the
    /// evaluation order of its priority band.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Present only when the fill moved the saved policy ahead of another
    /// same-band policy that may decide the same request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<PolicyWriteWarning>>,
}

/// One warning on a policy write's response (`kind: "evaluation_order_changed"`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PolicyWriteWarning {
    pub kind: String,
    /// The other policy's name.
    pub policy: String,
    /// The other policy's id, when known.
    #[serde(rename = "policyId")]
    pub policy_id: Option<i64>,
    /// `access` | `mask` | `row_filter`.
    #[serde(rename = "policyKind")]
    pub policy_kind: String,
    /// Operator-readable explanation.
    pub sentence: String,
}

/// What a save that filled emptied levels says to the operator. It never says
/// the more specific policy wins: Ranger evaluates the BROADER one first.
const FILLED_LEVELS_NOTE: &str = "Levels left empty in the form were saved as `*` (every child \
    of the level above). Ranger does not apply a policy that stops above the column to column \
    requests, so an empty level would have granted no column read. A `*` level also moves \
    this policy EARLIER in Ranger's evaluation order within its priority band: Ranger \
    evaluates the broader policy (more `*` levels, other things equal) first, not the more \
    specific one. Where it may now be consulted before another policy that can decide the \
    same request, `warnings` names that policy.";

/// The 200 body of a create/update: `{id}` alone for an ordinary save, plus
/// `filledLevels`, `note` and any `warnings` when the save filled levels.
fn write_response(
    id: i64,
    pol: &RangerPolicy,
    shifts: &[crate::ambiguous_arbitration::EvalOrderShift],
) -> Value {
    let mut out = json!({ "id": id });
    let Some(filled) = &pol.filled else {
        return out;
    };
    out["filledLevels"] = json!(filled.names);
    out["note"] = json!(FILLED_LEVELS_NOTE);
    if !shifts.is_empty() {
        out["warnings"] = shifts
            .iter()
            .map(|s| {
                json!({
                    "kind": "evaluation_order_changed",
                    "policy": s.other,
                    "policyId": s.other_id,
                    "policyKind": match s.policy_type { 1 => "mask", 2 => "row_filter", _ => "access" },
                    "sentence": s.sentence(&pol.name),
                })
            })
            .collect();
    }
    out
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
///   `tagService` (no `UPDATE` of `config_jsonb` existed in `src/` before #786, and
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

/// Whether a Ranger body is a mask (1) or a row filter (2) — the class the
/// overlap rule is about.
fn is_mask_or_filter(body: &Value) -> bool {
    matches!(
        body.get("policyType").and_then(Value::as_i64),
        Some(1) | Some(2)
    )
}

/// What [`write_policy_checked`] writes.
#[derive(Debug, Clone, Copy)]
enum PolicyWrite {
    Create,
    Replace(i64),
}

/// **The editor's write, with its overlap check made atomic** (SP4d-4 Task 3
/// fix round 1, review I1). One transaction:
///
/// 1. for a mask or row filter, `FOR UPDATE` on every `policy.authz_services`
///    row of every evaluation unit the write lands in, ascending, through
///    [`crate::lock_service_rows_via`] — the same rows, the same order and the
///    same helper the rule compiler's `lock_services_in_tx` takes for a
///    recompile of that unit (a profile and its tag service; or a tag service
///    and every profile naming it). A tag assignment recompiles under those
///    locks too, so an editor write and a concurrent assignment, recompile or
///    second editor write on one unit now queue;
/// 2. [`refuse_ambiguous_arbitration`] on that transaction, so it reads the
///    unit as the last committed writer left it;
/// 3. the store's own guarded INSERT/UPDATE, on the same transaction;
/// 4. commit.
///
/// Before this, the check read on a pooled connection with no lock, and an
/// editor write racing a tag assignment could see neither the other's row and
/// commit a pair both checks had passed.
async fn write_policy_checked(
    st: &PolicyAdminState,
    service_id: i64,
    write: PolicyWrite,
    pol: &RangerPolicy,
    actor: &egret_core::Principal,
) -> Result<(i64, Vec<crate::ambiguous_arbitration::EvalOrderShift>), PolicyHttpError> {
    let mut tx = st.store.begin().await?;
    if is_mask_or_filter(&pol.body) {
        let mut rows: Vec<i64> = crate::unit_overlap::units_for_service_via(&mut tx, service_id)
            .await?
            .into_iter()
            .flat_map(|u| [u.resource, u.tag_service])
            .flatten()
            .chain([service_id])
            .collect();
        rows.sort_unstable();
        rows.dedup();
        crate::lock_service_rows_via(&mut tx, &rows).await?;
    }
    let replacing = match write {
        PolicyWrite::Create => None,
        PolicyWrite::Replace(id) => Some(id),
    };
    let before = match replacing {
        Some(id) => ranger_policy_summary_via(&mut tx, id).await?,
        None => None,
    };
    // Final review I-3: a data mask written on a StarRocks profile (or one a
    // replace overwrites) changes what the clearance mask folds.
    let replaced_was_fold_input = match replacing {
        Some(id) => match stored_policy(&mut tx, id).await? {
            Some((sid, body)) => changes_the_clearance_fold(&mut tx, sid, &body).await?,
            None => false,
        },
        None => false,
    };
    let mut refold = replaced_was_fold_input
        || changes_the_clearance_fold(&mut tx, service_id, &pol.body).await?;
    // Security re-review M-a: a profile whose engines carry impersonation
    // grants is recompiled by every write, the unit's rows locked first (a
    // mask or filter above has locked them already; re-taking is free).
    if !refold && carries_impersonation_grants(&mut tx, service_id).await? {
        lock_unit_rows(&mut tx, service_id).await?;
        refold = true;
    }
    refuse_ambiguous_arbitration(&mut tx, service_id, replacing, pol).await?;
    // SP4d-4 Task 11: a save that filled emptied levels with `*` lowers the
    // policy's evalOrder; report whom it now precedes. Runs after the
    // refusal, so a pair that guard refuses is refused, not merely warned.
    let shifts = match &pol.filled {
        None => Vec::new(),
        Some(f) => {
            let stored =
                crate::store::enabled_policies_for_service_via(&mut tx, service_id).await?;
            let is_replaced = |p: &Value| {
                replacing.is_some_and(|id| p.get("id").and_then(Value::as_i64) == Some(id))
            };
            crate::ambiguous_arbitration::eval_order_shifts(
                &f.unfilled_body,
                &pol.body,
                stored
                    .iter()
                    .filter(|p| !is_replaced(p))
                    .map(|p| (p.get("name").and_then(Value::as_str).unwrap_or_default(), p)),
            )
        }
    };
    let id = match write {
        PolicyWrite::Create => {
            let id = crate::insert_policy_owned_by_via(
                &mut tx, service_id, &pol.name, &pol.body, "operator",
            )
            .await?;
            if !pol.is_enabled {
                // The INSERT takes the DB default (enabled); reflect the form's toggle.
                crate::update_policy_via(&mut tx, id, &pol.name, &pol.body, false).await?;
            }
            id
        }
        PolicyWrite::Replace(id) => {
            crate::update_policy_via(&mut tx, id, &pol.name, &pol.body, pol.is_enabled).await?;
            id
        }
    };
    // DG #3: the governance event, on this transaction (`before` was read
    // above, before the write).
    let after = ranger_policy_summary_via(&mut tx, id).await?;
    let action = match write {
        PolicyWrite::Create => egret_audit::governance::action::RANGER_POLICY_CREATE,
        PolicyWrite::Replace(_) => egret_audit::governance::action::RANGER_POLICY_UPDATE,
    };
    record_policy_change(&mut tx, action, id, before, after, actor).await?;
    if refold {
        st.recompiler
            .recompile_in_tx(&mut tx, &[service_id])
            .await?;
    }
    tx.commit().await.map_err(crate::PolicyStoreError::from)?;
    Ok((id, shifts))
}

/// The longest resource value list a governance summary keeps.
const SUMMARY_RESOURCE_VALUES: usize = 20;

/// A stored policy as a governance summary (DG #3, spec §3.1): `{service,
/// name, policy_type, enabled, resources}`, each resource's value list capped
/// at [`SUMMARY_RESOURCE_VALUES`] (`resources_truncated: true` when one was).
/// `None` when no such policy exists.
async fn ranger_policy_summary_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<Option<Value>, PolicyHttpError> {
    let row: Option<(String, bool, Value, String)> = sqlx::query_as(
        "SELECT p.name, p.enabled, p.policy_jsonb, s.name \
           FROM policy.authz_policies p JOIN policy.authz_services s ON s.id = p.service_id \
          WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::PolicyStoreError::from)?;
    let Some((name, enabled, body, service)) = row else {
        return Ok(None);
    };
    let policy_type = match body.get("policyType").and_then(Value::as_i64).unwrap_or(0) {
        0 => json!("access"),
        1 => json!("masking"),
        2 => json!("rowfilter"),
        other => json!(other),
    };
    let mut truncated = false;
    let mut resources = serde_json::Map::new();
    if let Some(res) = body.get("resources").and_then(Value::as_object) {
        for (key, level) in res {
            let values: Vec<Value> = level
                .get("values")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            truncated |= values.len() > SUMMARY_RESOURCE_VALUES;
            resources.insert(
                key.clone(),
                Value::Array(values.into_iter().take(SUMMARY_RESOURCE_VALUES).collect()),
            );
        }
    }
    let mut summary = json!({
        "service": service,
        "name": name,
        "policy_type": policy_type,
        "enabled": enabled,
        "resources": resources,
    });
    if truncated {
        summary["resources_truncated"] = json!(true);
    }
    Ok(Some(summary))
}

/// Record a `ranger_policy:*` governance event on the write's transaction.
async fn record_policy_change(
    conn: &mut sqlx::PgConnection,
    action: &'static str,
    id: i64,
    before: Option<Value>,
    after: Option<Value>,
    actor: &egret_core::Principal,
) -> Result<(), PolicyHttpError> {
    egret_audit::governance::record_change(
        conn,
        egret_audit::governance::Change {
            action,
            resource_type: egret_audit::governance::resource::RANGER_POLICY,
            resource_id: id.to_string(),
            summary: json!({ "before": before, "after": after }),
            principal: actor,
        },
    )
    .await
    .map_err(crate::PolicyStoreError::from)?;
    Ok(())
}

/// **The editor's half of SP4d debt Task 6's one rule.** Would storing `pol`
/// under `service_id` leave an evaluation unit it lands in — the profile and
/// its tag service, with the tag assignments that place tag policies (SP4d-4
/// Task 3) — serving two mask (or two row-filter) policies that may decide one
/// request and would decide it differently? The rule itself is
/// [`crate::unit_overlap::unit_overlaps`] (which takes every within-download
/// pair from [`crate::ambiguous_arbitration::ambiguous_arbitrations_in`]),
/// spelled once and shared with `egret_perm_compile::trigger::compile_desired`.
///
/// `replacing` is the policy id a `PUT` is about to overwrite, swapped out of
/// the AFTER set — **without it a `PUT` would pair the new body against the OLD
/// version of itself**, which is a refusal that cannot be escaped (every retry
/// hits the same stored row) and the first thing a reviewer should check here.
///
/// Only pairs the write CREATES are refused (the AFTER set's pairs minus the
/// stored set's); see the body for why.
///
/// Reads nothing when the body is not a mask or a row filter, which is the
/// common case: an access policy is not in this class at all (its arbitration
/// is `>` and deny-wins makes order immaterial — see the guard's module doc).
/// The read it does do is `enabled_policies_for_service`, the SAME assembler the
/// download route and `explain` use, so the set compared is the set served.
///
/// Runs inside the write's own transaction, after [`write_policy_checked`] has
/// locked every service row of every unit the write lands in and before the
/// INSERT/UPDATE (SP4d-4 Task 3 fix round 1, review I1), so the set it reads
/// cannot change under it and a refusal rolls back a transaction that has
/// written nothing.
async fn refuse_ambiguous_arbitration(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    replacing: Option<i64>,
    pol: &RangerPolicy,
) -> Result<(), PolicyHttpError> {
    if !is_mask_or_filter(&pol.body) {
        return Ok(());
    }
    // **Every unit this write lands in** (SP4d-4 Task 3, user ruling): the
    // profile plus its tag service, or — for a tag service — each Trino
    // profile that names it, with the tag assignments that decide where a tag
    // policy applies. `crate::unit_overlap::unit_overlaps` is the ONE rule the
    // compiler calls too; here it is handed the stored set and the body.
    let mut units = Vec::new();
    for ids in crate::unit_overlap::units_for_service_via(conn, service_id).await? {
        units.push(crate::unit_overlap::load_unit_via(conn, ids).await?);
    }
    for unit in units {
        let is_replaced = |sid: i64, p: &Value| {
            sid == service_id
                && replacing.is_some_and(|id| p.get("id").and_then(Value::as_i64) == Some(id))
        };
        let is_tag_written = unit.ids.and_then(|u| u.tag_service) == Some(service_id);
        // BEFORE: the unit as stored. The row a PUT replaces is in it, under
        // the name it will carry AFTER the write, so a rename alone does not
        // make a stored pair look new.
        let before_names: Vec<String> = unit
            .policies
            .iter()
            .map(|(sid, _, p)| {
                if is_replaced(*sid, p) {
                    pol.name.clone()
                } else {
                    p.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                }
            })
            .collect();
        let before: Vec<crate::unit_overlap::UnitPolicy<'_>> = unit
            .policies
            .iter()
            .zip(&before_names)
            .map(
                |((sid, is_tag, body), name)| crate::unit_overlap::UnitPolicy {
                    service: *sid,
                    is_tag_service: *is_tag,
                    name,
                    body,
                },
            )
            .collect();
        // AFTER: the stored unit with the replaced row swapped for the body.
        let after: Vec<crate::unit_overlap::UnitPolicy<'_>> = unit
            .unit_policies()
            .into_iter()
            .filter(|p| !is_replaced(p.service, p.body))
            .chain(std::iter::once(crate::unit_overlap::UnitPolicy {
                service: service_id,
                is_tag_service: is_tag_written,
                name: &pol.name,
                body: &pol.body,
            }))
            .collect();
        // **Refuse only a pair this write CREATES** (SP4d-4 Task 2 review I2).
        // Refusing any pair in the unit broke Part A's promise that a write
        // REMOVING a pair succeeds: with stored pairs (A,B) and (C,D), narrowing
        // A was refused on (C,D) and narrowing C on (A,B), so the editor could
        // never fix either. A pair of hand-written policies that is already
        // stored is therefore left alone by this guard; the rule compiler
        // refuses only pairs with a COMPILED member (SP4d-4 Task 3 fix round
        // 1), so it is reported, not refused — by `explain`
        // (`EvalOrderAmbiguous`) and by the read-only overlap report SP4d-4
        // Task 4 adds (`GET /perm/overlaps`). A pair the write creates or
        // completes — a new overlapping policy, an edit that widens into
        // another policy, a re-enable through this route — is not in BEFORE
        // and is refused.
        //
        // Identity is `(kind, first, first's items, second, second's items,
        // coordinate)` (SP4d-4 Task 3): an edit that keeps a stored partner but
        // changes what either side decides, or where they meet, makes a
        // DIFFERENT pair and is refused; an edit that touches neither (a
        // rename, a description) keeps the identity and passes.
        let created = crate::unit_overlap::created_overlaps(
            &crate::unit_overlap::unit_overlaps(&before, &unit.assignments),
            crate::unit_overlap::unit_overlaps(&after, &unit.assignments),
        );
        if let Some(pair) = created.into_iter().next() {
            // 409, not 400: the body is well-formed; it conflicts with a policy
            // already stored (SP4d-4 Part A, "The refusal"). The editor's
            // sentence adds the one remedy only a hand-written policy has,
            // Override, wherever an authored priority can fix the pair.
            //
            // The body is the one every overlap surface answers (fix round 1,
            // M1): a compiled member is named by its RULE (id and name), a
            // hand-written one by its policy name with `id: null`.
            let rules = crate::unit_overlap::compiled_rule_names_via(
                conn,
                &[pair.first.as_str(), pair.second.as_str()],
            )
            .await?;
            let label = |n: &str| match rules.get(n) {
                Some((_, rule)) => format!("rule {rule:?}"),
                // One label for a hand-written member on every surface (fix
                // round 2, N3): the compiler and the tag assignment write
                // `policy "H"` too.
                None => format!("policy {}", crate::ambiguous_arbitration::quoted(n)),
            };
            let member = |n: &str| match rules.get(n) {
                Some((id, rule)) => json!({ "id": id, "name": rule }),
                None => json!({ "id": null, "name": n }),
            };
            return Err(PolicyHttpError::Overlap {
                body: json!({
                    "error": pair.sentence(&label, true),
                    "code": "rule_overlap",
                    "rules": [member(&pair.first), member(&pair.second)],
                    "coordinate": pair.coordinate,
                    "kind": pair.kind_wire_name(),
                }),
            });
        }
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
/// end up running different guards. So they run this, and then both write
/// through [`write_policy_checked`], whose `PolicyWrite` is the only thing that
/// differs between them: `Create`, or `Replace(id)`, which reaches
/// [`refuse_ambiguous_arbitration`] as `replacing` and excludes that row from
/// the comparison set — without it a `PUT` pairs the new body against the OLD
/// version of itself and produces a refusal no retry can escape. (The
/// arbitration check moved out of this function in SP4d-4 Task 3 fix round 1,
/// review I1, so it runs inside the write's transaction under the unit's row
/// locks.)
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
            return Err(PolicyHttpError::BadRequestBody {
                body: pair.refusal_body(),
            });
        }
    }
    // SP4d debt Task 6: a tag-addressed MASK or ROW FILTER is the same class as
    // a path-addressed one — `tag_masking_to_ranger_json` and
    // `tag_rowfilter_to_ranger_json` delegate to the very builders the resource
    // path uses and only retarget the selector, so the pair is reachable here
    // too. Checked against every unit the TAG service is in — by
    // `write_policy_checked`, which both tag surfaces write through, inside the
    // write's transaction and under the unit's row locks (SP4d-4 Task 3 fix
    // round 1, review I1).
    Ok(())
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
        (status = 200, description = "Policy created. When the save stored an emptied resource level as `*`, the body adds `filledLevels`, a `note` (the broader policy is evaluated first within a priority band) and, where the fill moved this policy ahead of another same-band policy that may decide the same request, `warnings`", body = CreatedPolicyId),
        (status = 400, description = "Invalid editor input (including a Ranger-shaped body, i.e. `policyItems`, `denyPolicyItems`, `dataMaskPolicyItems` or `rowFilterPolicyItems`, which is refused naming the keys because the form's grants are `items`; an EMPTY catalog level, which `*` would turn into every catalog, and a named resource level beneath an absent one), missing policy name, a tag policy whose profile has no configured `tagService` (the plugin download would carry no `tagPolicies` block, so the policy could never be enforced), or a tag masking policy whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked; code `table_tag_mask_conflict`, with `tag`, `tagId`, `warehouseId`, `schema`, `table`, `mask`, `maskOrigin`, `moreTables`)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:create permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 409, description = "A policy with this name already exists, the target profile is compiled by the SP4a permission model (generated) and cannot be hand-edited, or a mask/row-filter policy would form a rule overlap with another policy or compiled rule that may decide the same request with different items (which of the two the engine applies is not something either policy expresses; the sentence names both and suggests Override where a priority can fix it). An overlap body adds `code: \"rule_overlap\"`, `rules` (both members: a compiled rule's `id` and name, `id` null for a hand-written policy), `coordinate` and `kind` (`mask` | `row_filter`)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_policy(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Json(v): Json<Value>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:create").await?;
    ranger_json::refuse_ranger_shaped_body(&v).map_err(PolicyHttpError::BadRequest)?;

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
        guard_tag_policy_authoring(&st, &pol).await?;
        let (id, _) = write_policy_checked(&st, svc.id, PolicyWrite::Create, &pol, &p).await?;
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
    // SP4d debt Task 6, item 1 — the editor's half, checked and written under
    // the unit's row locks. See `write_policy_checked`.
    let (id, shifts) = write_policy_checked(&st, svc.id, PolicyWrite::Create, &pol, &p).await?;
    Ok(Json(write_response(id, &pol, &shifts)))
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
        (status = 200, description = "Policy replaced. Same optional `filledLevels` / `note` / `warnings` as create", body = CreatedPolicyId),
        (status = 400, description = "Invalid editor input (including a Ranger-shaped body, i.e. `policyItems`, `denyPolicyItems`, `dataMaskPolicyItems` or `rowFilterPolicyItems`, which is refused naming the keys because the form's grants are `items`; an EMPTY catalog level, which `*` would turn into every catalog, and a named resource level beneath an absent one), missing policy name, a `service` that is not the profile this policy belongs to (a policy cannot be moved between profiles by editing it, and every guard here would otherwise be evaluated against the wrong profile's policy set), a tag policy whose profile has no configured `tagService`, or a tag masking policy whose tag is attached to a whole TABLE (which makes that table unreadable rather than masked; code `table_tag_mask_conflict`, with `tag`, `tagId`, `warehouseId`, `schema`, `table`, `mask`, `maskOrigin`, `moreTables`)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such policy", body = ErrorBody),
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited, or the mask/row-filter body would form a rule overlap with another policy or compiled rule that may decide the same request with different items (which of the two the engine applies is not something either policy expresses; the sentence names both and suggests Override where a priority can fix it). An overlap body adds `code: \"rule_overlap\"`, `rules` (both members: a compiled rule's `id` and name, `id` null for a hand-written policy), `coordinate` and `kind` (`mask` | `row_filter`), or the change is a data mask on a StarRocks profile and the profile cannot be recompiled with it (its classification masks fold the profile's other masks, so the write recompiles the profile in the same transaction and is refused when that recompile is)", body = ErrorBody),
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
    ranger_json::refuse_ranger_shaped_body(&v).map_err(PolicyHttpError::BadRequest)?;
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
        guard_tag_policy_authoring(&st, &pol).await?;
        write_policy_checked(&st, svc.id, PolicyWrite::Replace(id), &pol, &p).await?;
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
    let (_, shifts) = write_policy_checked(&st, svc.id, PolicyWrite::Replace(id), &pol, &p).await?;
    Ok(Json(write_response(id, &pol, &shifts)))
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
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited, or the change is a data mask on a StarRocks profile and the profile cannot be recompiled with it (its classification masks fold the profile's other masks, so the write recompiles the profile in the same transaction and is refused when that recompile is)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_policy_handler(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:delete").await?;
    // Final review I-3: deleting a StarRocks data mask refolds the profile's
    // clearance masks in the same transaction.
    let mut tx = st.store.begin().await?;
    let refold = fold_profile_of_stored(&mut tx, id).await?;
    let before = ranger_policy_summary_via(&mut tx, id).await?;
    crate::delete_policy_via(&mut tx, id).await?;
    record_policy_change(
        &mut tx,
        egret_audit::governance::action::RANGER_POLICY_DELETE,
        id,
        before,
        None,
        &p,
    )
    .await?;
    if let Some(service_id) = refold {
        st.recompiler
            .recompile_in_tx(&mut tx, &[service_id])
            .await?;
    }
    tx.commit().await.map_err(crate::PolicyStoreError::from)?;
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
        (status = 409, description = "The policy's profile is compiled by the SP4a permission model (generated) and cannot be hand-edited, or the change is a data mask on a StarRocks profile and the profile cannot be recompiled with it (its classification masks fold the profile's other masks, so the write recompiles the profile in the same transaction and is refused when that recompile is)", body = ErrorBody),
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
    // Final review I-3: as delete, for an enabled flag.
    let mut tx = st.store.begin().await?;
    let refold = fold_profile_of_stored(&mut tx, id).await?;
    let before = ranger_policy_summary_via(&mut tx, id).await?;
    crate::set_policy_enabled_via(&mut tx, id, b.enabled).await?;
    let after = ranger_policy_summary_via(&mut tx, id).await?;
    let action = if b.enabled {
        egret_audit::governance::action::RANGER_POLICY_ENABLE
    } else {
        egret_audit::governance::action::RANGER_POLICY_DISABLE
    };
    record_policy_change(&mut tx, action, id, before, after, &p).await?;
    if let Some(service_id) = refold {
        st.recompiler
            .recompile_in_tx(&mut tx, &[service_id])
            .await?;
    }
    tx.commit().await.map_err(crate::PolicyStoreError::from)?;
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
        set_tag_service,
        seed_defaults,
        get_service_def,
        resource_lookup,
        list_principals,
    ),
    components(schemas(
        PolicySummary,
        ProfileDto,
        ProfileEngine,
        PolicyCounts,
        CreateProfileBody,
        CreatedProfile,
        ConfigFile,
        TrinoConfigResponse,
        SetTagServiceBody,
        ProfileTagService,
        Principal,
        EnabledBody,
        CreatedPolicyId,
        PolicyWriteWarning,
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
    fn summarize_starrocks_policy_keeps_the_database_level() {
        // StarRocks names its second level `database`, not `schema`.
        let body = json!({
            "name": "sr-read",
            "policyType": 0,
            "isEnabled": true,
            "resources": {
                "catalog":  {"values": ["lake"]},
                "database": {"values": ["sales"]},
                "table":    {"values": ["items"]},
                "column":   {"values": ["*"]}
            },
            "policyItems": [
                {"users": ["dave"], "accesses": [{"type": "select"}]}
            ]
        });
        let s = summarize_policy(7, "sr4live", true, "operator", &body);
        assert_eq!(s.resource_path, "lake.sales.items.*");
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
