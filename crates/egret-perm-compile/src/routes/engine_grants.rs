//! Engine-scoped grants over HTTP (F11, spec §5): the capability catalogue,
//! the per-engine listing with the profile's seeded baselines as managed
//! entries, every engine grant (for People, E15), and the three writes.
//!
//! Reads are gated `policy:read`, writes `policy:update`, through
//! `egret_core::authz::require`, as every `perm` route is.
//!
//! **Lock order of a write** (spec §5.3, plus the F11 Task 6 amendment): the
//! impersonation target's per-user membership lock
//! (`egret_auth::groups::lock_member_users_via`, the FIRST lock
//! `UserRepo::hard_delete` takes), then the per-group locks of every group the
//! write names, then the engine row `FOR SHARE`, then the profile locks
//! ascending (`lock_services_in_tx`), then the model rows. The user lock is
//! what closes the race between creating an impersonation grant and deleting
//! its target: `hard_delete` reads the profiles to recompile under that lock,
//! so a grant inserted concurrently either commits first (and the delete sees
//! and recompiles it) or waits and then finds its target gone.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm::{
    create_engine_grant_in_tx, delete_engine_grant_in_tx, get_engine_grant_via,
    list_engine_grants_for_engine_via, list_engine_grants_via, update_engine_grant_in_tx,
    EngineGrant, EngineType, Principal, PrincipalKind, PrincipalSet,
};
use egret_policy::catalogs_for_service_via;
use egret_policy_api::infra_names::{
    ADMIN_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_NAME_PREFIX, EXECUTE_QUERY_POLICY_NAME,
    IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME, SELF_IMPERSONATE_POLICY_NAME,
};

use super::{lock_named_groups, CompileResponse, PermHttpError, PermState};
use crate::engine_capabilities::{
    capability, check_values, Capability, ValueKind, CAPABILITIES, NOT_GRANTABLE,
};
use crate::engine_grants::{engine_grant_policy_name, impersonation_target_ok, is_reserved_target};
use crate::principal_refs::{resolve_principal_refs, still_stored};
use crate::trigger::{lock_services_in_tx, recompile_services_in_tx, CompileSummary};

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// `GET /perm/engine-capabilities`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilitiesResponse {
    pub engines: Vec<EngineCapabilities>,
}

/// One engine type's catalogue.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilities {
    /// `trino` | `starrocks`.
    pub engine_type: String,
    pub capabilities: Vec<CapabilityView>,
    pub not_grantable: Vec<NotGrantableView>,
}

/// One grantable capability.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityView {
    pub id: String,
    pub label: String,
    pub description: String,
    /// The Ranger resource keys in hierarchy order.
    pub resources: Vec<CapabilityResource>,
    /// The one Ranger access type.
    pub access: String,
}

/// One resource key of a capability and the value a grant supplies for it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResource {
    pub key: String,
    /// `username` | `identifier` | `catalog` | `function_signature`, or
    /// `fixed` (the compiler supplies `*`; the grant gives no value).
    pub value_kind: String,
}

/// An engine-level permission egret does not offer, and why (spec E4).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotGrantableView {
    pub what: String,
    pub why: String,
}

/// The engine a listing is about.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineRef {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub origin: String,
    pub status: String,
}

/// The policy profile an engine downloads.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRef {
    pub service_id: i64,
    pub name: String,
}

/// Every engine a grant reaches: every engine bound to the profile it compiles
/// into, ordered by name (spec E9). One engine in every installation created
/// since I2; more only on a legacy shared profile.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    pub engines: Vec<ReachEngine>,
    pub shared_profile: bool,
}

/// One engine of a [`Reach`].
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReachEngine {
    pub id: Uuid,
    pub name: String,
    pub origin: String,
}

/// One engine grant as the API shows it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineGrantView {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub capability: String,
    /// The values in the catalogue's key order, an impersonation target as the
    /// user's CURRENT username (it is stored by id). A target with no username
    /// is left out; `targetUserId` still names them.
    pub values: Vec<String>,
    /// The impersonation target's user id, for an impersonation capability.
    pub target_user_id: Option<Uuid>,
    /// Allow-only, include-only (spec E5).
    pub principals: Vec<Principal>,
    /// `perm-engine-{id}`.
    pub policy_name: String,
    pub reach: Reach,
}

/// What a managed (seeded, `managed_by = 'infra'`) policy of the profile is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagedKind {
    /// The per-user baseline every delegating Trino engine gets.
    UserBaseline,
    /// The delegator's impersonate-anyone policy (impersonate mode).
    Delegator,
    /// `egret-admin - <chain>`: platform admins, every hierarchy.
    AdminAllAccess,
    /// `all - <chain>`: group `admin`, seeded by `seed-defaults`.
    DefaultAllAccess,
}

/// One seeded policy of the profile, read-only (spec E12).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEntry {
    pub policy_id: i64,
    pub policy_name: String,
    pub kind: ManagedKind,
    /// The catalogue id whose resource key set equals this policy's, if any.
    pub capability: Option<String>,
    pub resources: BTreeMap<String, Vec<String>>,
    pub accesses: Vec<String>,
    pub users: Vec<String>,
    pub groups: Vec<String>,
    pub enabled: bool,
}

/// `GET /perm/engines/{engine_id}/grants`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineGrantsResponse {
    pub engine: EngineRef,
    /// `null` when the engine downloads no profile.
    pub profile: Option<ProfileRef>,
    pub reach: Reach,
    pub grants: Vec<EngineGrantView>,
    pub managed: Vec<ManagedEntry>,
}

/// An engine grant as authored over HTTP. The engine is the path's.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewEngineGrant {
    pub capability: String,
    /// In the catalogue's key order; an impersonation target as a username.
    #[serde(default)]
    pub values: Vec<String>,
    /// `user`, `group` (never `public`) or `role`; at least one.
    #[serde(default)]
    pub principals: Vec<Principal>,
}

/// What a write did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineGrantWriteResponse {
    pub id: Uuid,
    pub compile: CompileResponse,
    pub reach: Reach,
}

// ---------------------------------------------------------------------------
// shared reads
// ---------------------------------------------------------------------------

fn engine_type_name(e: EngineType) -> &'static str {
    match e {
        EngineType::Trino => "trino",
        EngineType::StarRocks => "starrocks",
    }
}

#[derive(sqlx::FromRow)]
struct EngineRow {
    id: Uuid,
    name: String,
    r#type: String,
    origin: String,
    status: String,
    authz_service_id: Option<i64>,
}

async fn read_engine(
    conn: &mut sqlx::PgConnection,
    engine_id: Uuid,
) -> Result<EngineRow, PermHttpError> {
    sqlx::query_as(
        "SELECT id, name, type, origin, status, authz_service_id \
           FROM engine_mgmt.engines WHERE id = $1",
    )
    .bind(engine_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(PermHttpError::db)?
    .ok_or_else(|| PermHttpError::Missing(format!("No engine has id {engine_id}.")))
}

/// Every engine bound to `profile`, ordered by name (spec E9).
async fn reach_of(
    conn: &mut sqlx::PgConnection,
    profile: Option<i64>,
    engine: &EngineRow,
) -> Result<Reach, PermHttpError> {
    let engines: Vec<ReachEngine> = match profile {
        Some(sid) => sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, origin FROM engine_mgmt.engines \
              WHERE authz_service_id = $1 ORDER BY name, id",
        )
        .bind(sid)
        .fetch_all(&mut *conn)
        .await
        .map_err(PermHttpError::db)?
        .into_iter()
        .map(|(id, name, origin)| ReachEngine { id, name, origin })
        .collect(),
        None => vec![ReachEngine {
            id: engine.id,
            name: engine.name.clone(),
            origin: engine.origin.clone(),
        }],
    };
    Ok(Reach {
        shared_profile: engines.len() > 1,
        engines,
    })
}

/// The values to show: the stored ones with the target's current username in
/// its slot.
async fn view_values(
    conn: &mut sqlx::PgConnection,
    g: &EngineGrant,
) -> Result<Vec<String>, PermHttpError> {
    let Some(cap) = capability(&g.capability) else {
        return Ok(g.values.clone());
    };
    let target: Option<String> = match g.target_user_id {
        Some(uid) => sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&mut *conn)
            .await
            .map_err(PermHttpError::db)?
            .flatten(),
        None => None,
    };
    let mut stored = g.values.iter();
    let mut out = Vec::new();
    for kind in cap.value_kinds() {
        if kind == ValueKind::Username {
            if let Some(t) = &target {
                out.push(t.clone());
            }
        } else if let Some(v) = stored.next() {
            out.push(v.clone());
        }
    }
    Ok(out)
}

async fn grant_view(
    conn: &mut sqlx::PgConnection,
    g: EngineGrant,
    reach: Reach,
) -> Result<EngineGrantView, PermHttpError> {
    let values = view_values(conn, &g).await?;
    Ok(EngineGrantView {
        id: g.id,
        engine_id: g.engine_id,
        capability: g.capability,
        values,
        target_user_id: g.target_user_id,
        principals: g.principals.include,
        policy_name: engine_grant_policy_name(g.id),
        reach,
    })
}

/// The kind of a managed policy, from its name (`infra_names`).
fn managed_kind(name: &str) -> ManagedKind {
    if name == IMPERSONATE_POLICY_NAME {
        ManagedKind::Delegator
    } else if name == SELF_IMPERSONATE_POLICY_NAME
        || name == EXECUTE_QUERY_POLICY_NAME
        || name == QUERY_TIMEOUT_POLICY_NAME
    {
        ManagedKind::UserBaseline
    } else if name.starts_with(&format!("{ADMIN_ALL_ACCESS_NAME_PREFIX} - ")) {
        ManagedKind::AdminAllAccess
    } else if name.starts_with(&format!("{DEFAULT_ALL_ACCESS_NAME_PREFIX} - ")) {
        ManagedKind::DefaultAllAccess
    } else {
        // `managed_by = 'infra'` is written only for the names above
        // (`PolicyStore::upsert_infra_policy` refuses any other); a future
        // infra name is shown as a baseline rather than dropped.
        ManagedKind::UserBaseline
    }
}

/// The catalogue entry of `engine` whose resource key SET equals `keys`.
fn capability_for_keys(engine: EngineType, keys: &[String]) -> Option<&'static Capability> {
    let mut want: Vec<&str> = keys.iter().map(String::as_str).collect();
    want.sort_unstable();
    CAPABILITIES.iter().find(|c| {
        if c.engine != engine {
            return false;
        }
        let mut have: Vec<&str> = c.resources.iter().map(|(k, _)| *k).collect();
        have.sort_unstable();
        have == want
    })
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `managed_by = 'infra'` policy of `profile` (spec §5.2).
async fn managed_entries(
    conn: &mut sqlx::PgConnection,
    profile: i64,
    engine: Option<EngineType>,
) -> Result<Vec<ManagedEntry>, PermHttpError> {
    let rows: Vec<(i64, String, serde_json::Value, bool)> = sqlx::query_as(
        "SELECT id, name, policy_jsonb, enabled FROM policy.authz_policies \
          WHERE service_id = $1 AND managed_by = 'infra' ORDER BY name, id",
    )
    .bind(profile)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    Ok(rows
        .into_iter()
        .map(|(policy_id, policy_name, body, enabled)| {
            let mut resources = BTreeMap::new();
            if let Some(obj) = body["resources"].as_object() {
                for (k, v) in obj {
                    resources.insert(k.clone(), strings(&v["values"]));
                }
            }
            let keys: Vec<String> = resources.keys().cloned().collect();
            let mut accesses = Vec::new();
            let mut users = Vec::new();
            let mut groups = Vec::new();
            for item in body["policyItems"].as_array().into_iter().flatten() {
                for a in item["accesses"].as_array().into_iter().flatten() {
                    if let Some(t) = a["type"].as_str() {
                        if !accesses.iter().any(|x| x == t) {
                            accesses.push(t.to_string());
                        }
                    }
                }
                users.extend(strings(&item["users"]));
                groups.extend(strings(&item["groups"]));
            }
            ManagedEntry {
                policy_id,
                kind: managed_kind(&policy_name),
                capability: engine
                    .and_then(|e| capability_for_keys(e, &keys))
                    .map(|c| c.id.to_string()),
                policy_name,
                resources,
                accesses,
                users,
                groups,
                enabled,
            }
        })
        .collect())
}

fn engine_type_of(row: &EngineRow) -> Option<EngineType> {
    match row.r#type.as_str() {
        "trino" => Some(EngineType::Trino),
        "starrocks" => Some(EngineType::StarRocks),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// reads
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/engine-capabilities",
    tag = "perm",
    responses(
        (status = 200, description = "The curated engine capability catalogue, per engine type, with what is deliberately not grantable and why", body = EngineCapabilitiesResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
    ),
)]
pub(crate) async fn list_engine_capabilities(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<EngineCapabilitiesResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let engines = [EngineType::Trino, EngineType::StarRocks]
        .into_iter()
        .map(|e| EngineCapabilities {
            engine_type: engine_type_name(e).to_string(),
            capabilities: CAPABILITIES
                .iter()
                .filter(|c| c.engine == e)
                .map(|c| CapabilityView {
                    id: c.id.to_string(),
                    label: c.label.to_string(),
                    description: c.description.to_string(),
                    resources: c
                        .resources
                        .iter()
                        .map(|(k, kind)| CapabilityResource {
                            key: (*k).to_string(),
                            value_kind: kind.map_or("fixed", |k| k.as_str()).to_string(),
                        })
                        .collect(),
                    access: c.access.to_string(),
                })
                .collect(),
            not_grantable: NOT_GRANTABLE
                .iter()
                .filter(|n| n.engine == e)
                .map(|n| NotGrantableView {
                    what: n.what.to_string(),
                    why: n.why.to_string(),
                })
                .collect(),
        })
        .collect();
    Ok(Json(EngineCapabilitiesResponse { engines }))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/engines/{engine_id}/grants",
    tag = "perm",
    params(("engine_id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "The engine, its profile, every engine it shares that profile with, its engine grants and the profile's seeded (managed) policies", body = EngineGrantsResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
    ),
)]
pub(crate) async fn list_engine_grants(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(engine_id): Path<Uuid>,
) -> Result<Json<EngineGrantsResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let engine = read_engine(&mut conn, engine_id).await?;
    let reach = reach_of(&mut conn, engine.authz_service_id, &engine).await?;
    let (profile, managed) = match engine.authz_service_id {
        Some(sid) => {
            let name: String =
                sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
                    .bind(sid)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            let managed = managed_entries(&mut conn, sid, engine_type_of(&engine)).await?;
            (
                Some(ProfileRef {
                    service_id: sid,
                    name,
                }),
                managed,
            )
        }
        None => (None, Vec::new()),
    };
    let mut grants = Vec::new();
    for g in list_engine_grants_for_engine_via(&mut conn, engine_id).await? {
        grants.push(grant_view(&mut conn, g, reach.clone()).await?);
    }
    Ok(Json(EngineGrantsResponse {
        engine: EngineRef {
            id: engine.id,
            name: engine.name,
            r#type: engine.r#type,
            origin: engine.origin,
            status: engine.status,
        },
        profile,
        reach,
        grants,
        managed,
    }))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/engine-grants",
    tag = "perm",
    responses(
        (status = 200, description = "Every engine grant, each with the engines it reaches (People pages match principals against `standsFor`)", body = Vec<EngineGrantView>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
    ),
)]
pub(crate) async fn list_all_engine_grants(
    State(st): State<PermState>,
    Auth(p): Auth,
) -> Result<Json<Vec<EngineGrantView>>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let mut out = Vec::new();
    let mut reaches: BTreeMap<Uuid, Reach> = BTreeMap::new();
    for g in list_engine_grants_via(&mut conn).await? {
        let reach = match reaches.get(&g.engine_id) {
            Some(r) => r.clone(),
            None => {
                let engine = read_engine(&mut conn, g.engine_id).await?;
                let r = reach_of(&mut conn, engine.authz_service_id, &engine).await?;
                reaches.insert(g.engine_id, r.clone());
                r
            }
        };
        out.push(grant_view(&mut conn, g, reach).await?);
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// writes
// ---------------------------------------------------------------------------

/// What [`write_engine_grant`] did.
pub(crate) struct EngineGrantOutcome {
    pub(crate) grant: EngineGrant,
    pub(crate) services: Vec<i64>,
    pub(crate) summaries: Vec<CompileSummary>,
    pub(crate) reach: Reach,
}

/// Lock engine `engine_id`'s row `FOR SHARE` and return its profile. A rebind
/// (`set_engine_authz_service_in_tx`) UPDATEs the row, which conflicts with
/// `FOR SHARE` (and not with `FOR KEY SHARE`), so the grant cannot be written
/// against a profile the engine has just left.
pub(crate) async fn lock_engine_row(
    tx: &mut Transaction<'_, Postgres>,
    engine_id: Uuid,
) -> Result<Option<i64>, PermHttpError> {
    let row: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT authz_service_id FROM engine_mgmt.engines WHERE id = $1 FOR SHARE",
    )
    .bind(engine_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    row.ok_or_else(|| PermHttpError::Missing(format!("No engine has id {engine_id}.")))
}

/// The refusals of step 1 (spec §5.3): an engine that cannot take an engine
/// grant. `Ok(profile)` otherwise.
fn writable_profile(engine: &EngineRow) -> Result<i64, PermHttpError> {
    if engine.r#type == "risingwave" {
        return Err(PermHttpError::Conflict(format!(
            "engine {} is RisingWave, which egret governs through no Ranger profile; it takes \
             no engine grant",
            engine.name
        )));
    }
    if engine.status == "deleting" {
        return Err(PermHttpError::Conflict(format!(
            "engine {} is being deleted",
            engine.name
        )));
    }
    engine.authz_service_id.ok_or_else(|| {
        PermHttpError::Conflict(format!(
            "engine {} downloads no policy profile, so a grant on it would be enforced \
             nowhere; bind a profile first",
            engine.name
        ))
    })
}

/// Resolve an impersonation target's username to its user id.
async fn target_user_id(
    conn: &mut sqlx::PgConnection,
    username: &str,
) -> Result<Uuid, PermHttpError> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE username = $1::citext")
        .bind(username)
        .fetch_optional(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
    id.ok_or_else(|| {
        PermHttpError::BadRequest(format!(
            "no user carries the username {username:?}; an impersonation target must be a \
             current user"
        ))
    })
}

fn is_unique_violation(e: &egret_perm::PermError) -> bool {
    matches!(e, egret_perm::PermError::Db(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23505"))
}

/// Create (`id: None`) or replace (`id: Some`) one engine grant on the
/// caller's transaction and recompile the engine's profile — spec §5.3,
/// steps 1–9, in the lock order of this module's doc.
pub(crate) async fn write_engine_grant(
    tx: &mut Transaction<'_, Postgres>,
    engine_id: Uuid,
    id: Option<Uuid>,
    body: NewEngineGrant,
) -> Result<EngineGrantOutcome, PermHttpError> {
    // The stored row of a patch, and the engine it is fixed to.
    let stored = match id {
        Some(id) => {
            let Some(g) = get_engine_grant_via(tx, id).await? else {
                return Err(PermHttpError::NotFound);
            };
            if g.engine_id != engine_id {
                return Err(PermHttpError::NotFound);
            }
            Some(g)
        }
        None => None,
    };

    // 1. The engine.
    let engine = read_engine(tx, engine_id).await?;
    writable_profile(&engine)?;
    let cap = capability(&body.capability).ok_or_else(|| {
        PermHttpError::BadRequest(format!("unknown capability {:?}", body.capability))
    })?;
    if Some(cap.engine) != engine_type_of(&engine) {
        return Err(PermHttpError::BadRequest(format!(
            "{} is not a capability of engine {} ({})",
            cap.id, engine.name, engine.r#type
        )));
    }

    // 2. Values and principals, by shape.
    let values = check_values(cap, &body.values).map_err(PermHttpError::BadRequest)?;
    if body.principals.is_empty() {
        return Err(PermHttpError::BadRequest(
            "an engine grant must name at least one principal".into(),
        ));
    }
    for p in &body.principals {
        match p.kind {
            PrincipalKind::User | PrincipalKind::Role => {}
            PrincipalKind::Group if p.r#ref != egret_perm::PUBLIC_GROUP => {}
            PrincipalKind::Group => {
                return Err(PermHttpError::BadRequest(
                    "an engine grant cannot name the `public` group: an engine capability for \
                     everyone is what the seeded baselines are for"
                        .into(),
                ))
            }
            PrincipalKind::WorkspaceMembers => {
                return Err(PermHttpError::BadRequest(
                    "an engine grant cannot name workspace members: engines are not \
                     workspace-scoped"
                        .into(),
                ))
            }
        }
    }

    // Split the target out of the values: it is stored by id (E21).
    let mut target_name: Option<String> = None;
    let mut stored_values = Vec::new();
    for (kind, v) in cap.value_kinds().zip(values) {
        if kind == ValueKind::Username {
            target_name = Some(v);
        } else {
            stored_values.push(v);
        }
    }

    // 3a. The target's per-user lock, FIRST (module doc). Resolved, locked,
    // then re-read under the lock: a hard delete that held it has committed by
    // now, and the name may be gone.
    let target_id = match &target_name {
        Some(name) => {
            if is_reserved_target(name) {
                return Err(PermHttpError::Conflict(format!(
                    "{name:?} is a principal egret runs as on the engine; it cannot be \
                     impersonated"
                )));
            }
            let uid = target_user_id(tx, name).await?;
            egret_auth::groups::lock_member_users_via(tx, &[uid])
                .await
                .map_err(PermHttpError::db)?;
            let still: Option<String> =
                sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
                    .bind(uid)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(PermHttpError::db)?
                    .flatten();
            if still.as_deref().map(str::to_lowercase) != Some(name.to_lowercase()) {
                return Err(PermHttpError::Conflict(format!(
                    "the user {name:?} changed while this request waited; retry"
                )));
            }
            Some(uid)
        }
        None => None,
    };

    // 3b. Group locks over the body's and the stored row's principals.
    let mut principals = PrincipalSet {
        include: body.principals,
        exclude: Vec::new(),
    };
    lock_named_groups(
        tx,
        [&principals]
            .into_iter()
            .chain(stored.as_ref().map(|g| &g.principals)),
    )
    .await?;

    // 4. Resolve refs, keeping (R6) what the row still holds under the locks.
    let kept = match (id, stored.as_ref()) {
        (Some(id), Some(stale)) => match get_engine_grant_via(tx, id).await? {
            Some(fresh) => Some(still_stored(&stale.principals, &fresh.principals)),
            None => return Err(PermHttpError::NotFound),
        },
        _ => None,
    };
    resolve_principal_refs(tx, "principals", &mut principals, kept.as_ref()).await?;

    // 5. The engine row FOR SHARE, and its profile re-read under that lock.
    let profile = lock_engine_row(tx, engine_id).await?.ok_or_else(|| {
        PermHttpError::Conflict(format!(
            "engine {} no longer downloads a policy profile",
            engine.name
        ))
    })?;

    // 6. Target and catalog checks against that profile.
    if let Some(uid) = target_id {
        if let Err(reason) = impersonation_target_ok(tx, profile, uid).await? {
            return Err(PermHttpError::Conflict(reason));
        }
    }
    let catalog_values: Vec<&String> = cap
        .value_kinds()
        .filter(|k| *k != ValueKind::Username)
        .zip(stored_values.iter())
        .filter(|(k, _)| *k == ValueKind::Catalog)
        .map(|(_, v)| v)
        .collect();
    if !catalog_values.is_empty() {
        let reached: Vec<String> = catalogs_for_service_via(tx, profile)
            .await?
            .into_iter()
            .map(|c| c.name.to_lowercase())
            .collect();
        for c in catalog_values {
            if !reached.iter().any(|r| r == c) {
                return Err(PermHttpError::BadRequest(format!(
                    "engine {} does not reach catalog {c:?}; it reaches: {}",
                    engine.name,
                    if reached.is_empty() {
                        "no catalog".to_string()
                    } else {
                        reached.join(", ")
                    }
                )));
            }
        }
    }

    // 7. The profile lock.
    let services = vec![profile];
    lock_services_in_tx(tx, &services).await?;

    // 8. The row.
    let mut g = EngineGrant {
        id: id.unwrap_or_else(Uuid::nil),
        engine_id,
        capability: cap.id.to_string(),
        values: stored_values,
        target_user_id: target_id,
        principals,
    };
    let duplicate = || PermHttpError::Conflict("this engine already has that grant".into());
    match id {
        Some(_) => match update_engine_grant_in_tx(tx, &g).await {
            Ok(true) => {}
            Ok(false) => return Err(PermHttpError::NotFound),
            Err(e) if is_unique_violation(&e) => return Err(duplicate()),
            Err(e) => return Err(e.into()),
        },
        None => match create_engine_grant_in_tx(tx, &g).await {
            Ok(new_id) => g.id = new_id,
            Err(e) if is_unique_violation(&e) => return Err(duplicate()),
            Err(e) => return Err(e.into()),
        },
    }

    // 9. The recompile.
    let summaries = recompile_services_in_tx(tx, &services).await?;
    let reach = reach_of(tx, Some(profile), &engine).await?;
    Ok(EngineGrantOutcome {
        grant: g,
        services,
        summaries,
        reach,
    })
}

/// The audit detail of a write (spec §7): exactly `engineId`, `capability`,
/// `values`, `principals`, `services`.
pub(crate) fn audit_detail(g: &EngineGrant, services: &[i64]) -> serde_json::Value {
    serde_json::json!({
        "engineId": g.engine_id,
        "capability": g.capability,
        "values": g.values,
        "principals": g.principals.include,
        "services": services,
    })
}

/// The audit hook of the three writes (spec E18). Logs the `Mutation` shape
/// today; whichever of #3 and F11 merges second replaces the body with
/// `st.audit.emit(egret_audit::mutation_for(p, None, action, "engine_grant",
/// Some(id), detail))`.
pub(crate) fn audit_engine_grant_write(
    p: &egret_core::Principal,
    action: &str,
    id: Uuid,
    detail: &serde_json::Value,
) {
    tracing::info!(
        target: "egret::audit",
        actor = %p.user_id.0,
        action,
        resource_type = "engine_grant",
        resource_id = %id,
        detail = %detail,
        "engine grant written"
    );
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/engines/{engine_id}/grants",
    tag = "perm",
    params(("engine_id" = Uuid, Path, description = "Engine id")),
    request_body = NewEngineGrant,
    responses(
        (status = 201, description = "Engine grant created AND the engine's profile recompiled, in one transaction", body = EngineGrantWriteResponse),
        (status = 400, description = "Unknown capability, a capability of another engine type, a value that breaks the catalogue's rules, a catalog the engine does not reach, or empty/invalid/unresolvable principals", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine has no profile, is being deleted or is RisingWave; the impersonation target is reserved or an engine admin; the engine already has that grant; or the recompile refused (overlap, ceiling)", body = ErrorBody),
    ),
)]
pub(crate) async fn create_engine_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(engine_id): Path<Uuid>,
    Json(body): Json<NewEngineGrant>,
) -> Result<(axum::http::StatusCode, Json<EngineGrantWriteResponse>), PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let out = write_engine_grant(&mut tx, engine_id, None, body).await?;
    tx.commit().await.map_err(PermHttpError::db)?;
    audit_engine_grant_write(
        &p,
        "perm.engine_grant.create",
        out.grant.id,
        &audit_detail(&out.grant, &out.services),
    );
    Ok((
        axum::http::StatusCode::CREATED,
        Json(EngineGrantWriteResponse {
            id: out.grant.id,
            compile: CompileResponse::from_summaries(out.services, &out.summaries),
            reach: out.reach,
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/control-plane/perm/engine-grants/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Engine grant id")),
    request_body = NewEngineGrant,
    responses(
        (status = 200, description = "Engine grant replaced (the engine is fixed) AND its profile recompiled", body = EngineGrantWriteResponse),
        (status = 400, description = "As for create", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such engine grant", body = ErrorBody),
        (status = 409, description = "As for create", body = ErrorBody),
    ),
)]
pub(crate) async fn patch_engine_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<NewEngineGrant>,
) -> Result<Json<EngineGrantWriteResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let Some(existing) = get_engine_grant_via(&mut tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let out = write_engine_grant(&mut tx, existing.engine_id, Some(id), body).await?;
    tx.commit().await.map_err(PermHttpError::db)?;
    audit_engine_grant_write(
        &p,
        "perm.engine_grant.update",
        id,
        &audit_detail(&out.grant, &out.services),
    );
    Ok(Json(EngineGrantWriteResponse {
        id,
        compile: CompileResponse::from_summaries(out.services, &out.summaries),
        reach: out.reach,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/perm/engine-grants/{id}",
    tag = "perm",
    params(("id" = Uuid, Path, description = "Engine grant id")),
    responses(
        (status = 200, description = "Engine grant deleted AND its compiled policy reaped", body = EngineGrantWriteResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such engine grant", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_engine_grant(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<EngineGrantWriteResponse>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    let Some(g) = get_engine_grant_via(&mut tx, id).await? else {
        return Err(PermHttpError::NotFound);
    };
    // A delete names no new group, so no group lock; the engine row, then the
    // profile lock, before the DELETE.
    let engine = read_engine(&mut tx, g.engine_id).await?;
    let profile = lock_engine_row(&mut tx, g.engine_id).await?;
    let services: Vec<i64> = profile.into_iter().collect();
    lock_services_in_tx(&mut tx, &services).await?;
    if !delete_engine_grant_in_tx(&mut tx, id).await? {
        return Err(PermHttpError::NotFound);
    }
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    let reach = reach_of(&mut tx, profile, &engine).await?;
    tx.commit().await.map_err(PermHttpError::db)?;
    audit_engine_grant_write(
        &p,
        "perm.engine_grant.delete",
        id,
        &audit_detail(&g, &services),
    );
    Ok(Json(EngineGrantWriteResponse {
        id,
        compile: CompileResponse::from_summaries(services, &summaries),
        reach,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audit_detail_has_exactly_the_five_fields() {
        let g = EngineGrant {
            id: Uuid::new_v4(),
            engine_id: Uuid::new_v4(),
            capability: "trino.set_system_session_property".into(),
            values: vec!["query_max_memory".into()],
            target_user_id: None,
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::User,
                    r#ref: "alice".into(),
                }],
                exclude: vec![],
            },
        };
        let d = audit_detail(&g, &[7]);
        let mut keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["capability", "engineId", "principals", "services", "values"]
        );
        assert_eq!(d["services"], serde_json::json!([7]));
    }

    #[test]
    fn a_managed_policy_maps_to_the_capability_with_its_key_set() {
        let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            capability_for_keys(EngineType::Trino, &keys(&["trinouser"])).map(|c| c.id),
            Some("trino.impersonate_user")
        );
        assert_eq!(
            capability_for_keys(EngineType::Trino, &keys(&["sessionproperty", "catalog"]))
                .map(|c| c.id),
            Some("trino.set_catalog_session_property")
        );
        assert_eq!(
            capability_for_keys(EngineType::Trino, &keys(&["queryid"])),
            None
        );
        assert_eq!(
            managed_kind("egret-admin - trinouser"),
            ManagedKind::AdminAllAccess
        );
        assert_eq!(
            managed_kind("all - catalog, schema"),
            ManagedKind::DefaultAllAccess
        );
        assert_eq!(
            managed_kind(IMPERSONATE_POLICY_NAME),
            ManagedKind::Delegator
        );
        assert_eq!(
            managed_kind(EXECUTE_QUERY_POLICY_NAME),
            ManagedKind::UserBaseline
        );
    }
}

// ---------------------------------------------------------------------------
// explain (spec §5.4, E13)
// ---------------------------------------------------------------------------

/// `POST /perm/explain/capability`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainCapabilityRequest {
    /// A username; no groups or roles, as `/perm/explain`.
    pub subject: super::ExplainSubject,
    pub engine_id: Uuid,
    pub capability: String,
    /// In the catalogue's key order, an impersonation target as a username.
    #[serde(default)]
    pub values: Vec<String>,
}

/// Whether the engine's profile could be asked at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOutcome {
    PerProfile,
    NoProfile,
}

/// What egret did about tags for this question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTagResolution {
    /// Asked with no tags: the capability's resource has no `catalog` level
    /// (no egret tag can attach to it), or the download's tag block holds no
    /// policy (no tag policy can decide).
    NotApplicable,
    /// Asked with unresolved tags: a `catalog`-level capability while the tag
    /// block holds a policy. The evaluator answers `TagsUnresolved`.
    Declined,
}

/// The answer.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainCapabilityResponse {
    pub engine: EngineRef,
    pub profile: Option<ProfileRef>,
    pub reach: Reach,
    pub outcome: CapabilityOutcome,
    pub decision: Option<crate::explain_wire::WireDecision>,
    pub attribution: Option<super::ExplainAttribution>,
    pub policy_name: Option<String>,
    pub tag_resolution: CapabilityTagResolution,
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/explain/capability",
    tag = "perm",
    request_body = ExplainCapabilityRequest,
    description = "Evaluate one user and one engine capability on the artifact the engine's \
                   profile serves. Undecidable is returned as such: on a delegating Trino \
                   profile every impersonation question is Undecidable, because the \
                   self-impersonation baseline names `{USER}`, which egret's evaluator does not \
                   expand.",
    responses(
        (status = 200, description = "The decision, or `outcome: no_profile` with `decision: null` for an engine that downloads no profile", body = ExplainCapabilityResponse),
        (status = 400, description = "Unknown capability, a capability of another engine type, or a value that breaks the catalogue's rules", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The profile's engines run a plugin version egret does not model", body = ErrorBody),
    ),
)]
pub(crate) async fn explain_capability(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<ExplainCapabilityRequest>,
) -> Result<Json<ExplainCapabilityResponse>, PermHttpError> {
    use egret_ranger_eval::eval::{AccessRequest, EngineConfig, UnenrichedTags};
    use egret_ranger_eval::resource::RequestResource;
    use egret_ranger_eval::servicedef::ServiceDef;

    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let engine = read_engine(&mut conn, body.engine_id).await?;
    let cap = capability(&body.capability).ok_or_else(|| {
        PermHttpError::BadRequest(format!("unknown capability {:?}", body.capability))
    })?;
    let Some(engine_type) = engine_type_of(&engine).filter(|t| *t == cap.engine) else {
        return Err(PermHttpError::BadRequest(format!(
            "{} is not a capability of engine {} ({})",
            cap.id, engine.name, engine.r#type
        )));
    };
    let values = check_values(cap, &body.values).map_err(PermHttpError::BadRequest)?;
    let reach = reach_of(&mut conn, engine.authz_service_id, &engine).await?;
    let engine_ref = EngineRef {
        id: engine.id,
        name: engine.name.clone(),
        r#type: engine.r#type.clone(),
        origin: engine.origin.clone(),
        status: engine.status.clone(),
    };
    let Some(service_id) = engine.authz_service_id else {
        return Ok(Json(ExplainCapabilityResponse {
            engine: engine_ref,
            profile: None,
            reach,
            outcome: CapabilityOutcome::NoProfile,
            decision: None,
            attribution: None,
            policy_name: None,
            tag_resolution: CapabilityTagResolution::NotApplicable,
        }));
    };

    let svc = egret_policy::get_service_by_id_via(&mut conn, service_id).await?;
    let plugin_version = super::profile_plugin_version(&mut conn, service_id, engine_type).await?;
    let artifact = super::served_artifact_via(&mut conn, &svc).await?;
    let service_def = ServiceDef::parse(&artifact.service_def);
    let config = EngineConfig {
        engine: Some(super::ranger_engine(engine_type)),
        plugin_version: Some(plugin_version),
        ..EngineConfig::without_super_users()
    };

    let mut values_iter = values.iter();
    let mut resource = RequestResource::new();
    for (key, kind) in cap.resources {
        let value = match kind {
            Some(_) => values_iter.next().expect("checked count").clone(),
            None => "*".to_string(),
        };
        resource = resource.with(*key, value);
    }
    // Tags (spec §5.4): egret's service tags only carry catalog/schema/table/
    // column, so no tag attaches to a resource without a `catalog` level; with
    // one, an empty tag block cannot decide either. Otherwise egret does not
    // resolve tags here and says so.
    let has_catalog = cap.resources.iter().any(|(k, _)| *k == "catalog");
    let (resolved_tags, tag_resolution) = if has_catalog && !artifact.tag_policies.is_empty() {
        (None, CapabilityTagResolution::Declined)
    } else {
        (
            Some(UnenrichedTags::new(Vec::new())),
            CapabilityTagResolution::NotApplicable,
        )
    };
    let request = AccessRequest {
        user: body.subject.user,
        // Never groups or roles (`ExplainSubject`).
        groups: Default::default(),
        roles: Default::default(),
        access_type: cap.access.to_string(),
        resource,
        owner: None,
        resolved_tags,
    };
    let decision = egret_ranger_eval::eval::evaluate(&artifact, &request, &service_def, &config);
    let attribution = super::attribution_for(&mut *conn, &decision).await?;
    let policy_name = match &decision {
        egret_ranger_eval::eval::Decision::Allowed { policy_name, .. }
        | egret_ranger_eval::eval::Decision::Denied { policy_name, .. } => {
            Some(policy_name.clone())
        }
        egret_ranger_eval::eval::Decision::Undecidable { .. } => None,
    };
    Ok(Json(ExplainCapabilityResponse {
        engine: engine_ref,
        profile: Some(ProfileRef {
            service_id,
            name: svc.name,
        }),
        reach,
        outcome: CapabilityOutcome::PerProfile,
        decision: Some(crate::explain_wire::WireDecision::from(&decision)),
        attribution,
        policy_name,
        tag_resolution,
    }))
}
