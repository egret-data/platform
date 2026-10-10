//! HTTP state + routers for workspace administration and membership.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::http::ErrorBody;
use egret_core::{Ack, Error, Pagination, UserId, WorkspaceId, WorkspacePath};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::access_requests::{AccessRequestEnriched, AccessRequestRepo};
use crate::access_views::AccessViewsRepo;
use crate::context::WsContext;
use crate::members::MemberRepo;
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

#[derive(Clone)]
pub struct WorkspaceState {
    pub workspaces: WorkspaceRepo,
    pub members: MemberRepo,
    /// Read-only workspace-scope role catalog (roles + their permissions).
    pub access_views: AccessViewsRepo,
    /// Join-request → approval workflow (P2b). Backs the requester endpoints
    /// (`/api/access-requests/*`).
    pub access_requests: AccessRequestRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub users: UserRepo,
    pub sessions: SessionService,
    /// Append-only audit sink (B4). Emits Mutation events at platform-role and
    /// membership changes; best-effort, never fails the request.
    pub audit: egret_audit::AuditSink,
    /// Overlay store — the ws_admin impersonation start endpoint writes the
    /// scoped overlay here (Task 9).
    pub impersonations: egret_auth::impersonation::ImpersonationRepo,
    /// The OIDC service, present only when SSO is enabled. Used by the ws_admin
    /// impersonation start endpoint to build the step-up authorize URL for an
    /// OIDC-provider caller. `None` on non-OIDC deploys and in unit tests.
    pub oidc: Option<egret_auth::oidc::OidcService>,
    /// The cookie-encryption key, present only when OIDC is enabled — needed to
    /// seal the impersonation intent into the `PrivateCookieJar`-encrypted state
    /// cookie. `None` when `oidc` is `None`.
    pub cookie_key: Option<axum_extra::extract::cookie::Key>,
    /// Whether session/state cookies are marked `Secure` (mirrors
    /// `config.session.secure`). Threaded into the OIDC step-up cookie builder.
    pub session_secure: bool,
}

impl FromRef<WorkspaceState> for WorkspaceRepo {
    fn from_ref(s: &WorkspaceState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<WorkspaceState> for Authorizer {
    fn from_ref(s: &WorkspaceState) -> Self {
        s.authz.clone()
    }
}
impl FromRef<WorkspaceState> for SessionService {
    fn from_ref(s: &WorkspaceState) -> Self {
        s.sessions.clone()
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct WorkspaceDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    pub owner_name: Option<String>,
    /// Module keys this workspace currently has switched on. Populated
    /// wherever a caller reads a workspace's entitlements back (`list_ws`,
    /// `get_ws`, `/api/me/workspaces`) via [`dto_with_features`]; left empty
    /// by the create/lifecycle handlers, which echo the resource they just
    /// mutated rather than reading entitlement state.
    #[serde(default)]
    pub features: Vec<String>,
}
impl From<crate::model::Workspace> for WorkspaceDto {
    fn from(w: crate::model::Workspace) -> Self {
        WorkspaceDto {
            id: w.id.to_string(),
            slug: w.slug,
            name: w.name,
            status: w.status,
            created_at: w.created_at,
            created_by: w.created_by.map(|u| u.to_string()),
            owner_name: w.owner_name,
            features: Vec::new(),
        }
    }
}

/// Attach `features` — the module keys currently switched ON — to a fresh
/// `WorkspaceDto`. Shared by every handler that reads back a workspace's
/// current entitlements (`list_ws`, `get_ws`, `list_my_workspaces`), so the
/// field means the same thing everywhere it is populated: there is no
/// "wasn't computed" state that could be confused with "every module is off".
fn dto_with_features(
    w: crate::model::Workspace,
    off: Option<&std::collections::HashSet<String>>,
) -> WorkspaceDto {
    let mut dto = WorkspaceDto::from(w);
    dto.features = crate::features::FEATURE_KEYS
        .iter()
        // With no set at all, each key is its catalog default (W5a: not
        // "every key on" -- `workflow_containers` is off until a row says so).
        .filter(|k| match off {
            Some(s) => !s.contains(**k),
            None => crate::features::default_enabled(k).unwrap_or(false),
        })
        .map(|k| k.to_string())
        .collect();
    dto
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct WorkspaceStatsDto {
    pub member_count: i64,
    pub query_count: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TransferOwner {
    pub new_owner_id: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateWorkspace {
    pub slug: String,
    pub name: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateWorkspace {
    pub name: String,
}
#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub fn admin_workspace_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/workspaces", post(create_ws).get(list_ws))
        .route(
            "/workspaces/{id}",
            get(get_ws).put(update_ws).delete(archive_ws),
        )
        .route("/workspaces/{id}/restore", post(restore_ws))
        .route("/workspaces/{id}/stats", get(workspace_stats))
        .route("/workspaces/{id}/transfer", post(transfer_owner))
        .route(
            "/workspaces/{id}/features",
            get(ws_features).put(set_ws_features),
        )
}

#[utoipa::path(
    post,
    path = "/api/admin/workspaces",
    tag = "admin-workspaces",
    request_body = CreateWorkspace,
    responses(
        (status = 200, description = "Workspace created", body = WorkspaceDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Json(body): Json<CreateWorkspace>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:create")
        .await
        .map_err(AuthError)?;
    // Warehouse provisioning is NO LONGER done at workspace-create time. A
    // workspace now owns N user-managed warehouses, each created on demand via the
    // per-warehouse management API (`POST .../catalog/warehouses`) on a registered
    // storage connection.
    let ws = st
        .workspaces
        .create(&body.slug, &body.name, p.user_id)
        .await
        .map_err(AuthError)?;

    Ok(Json(ws.into()))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspaces",
    tag = "admin-workspaces",
    params(
        ("page" = Option<u32>, Query, description = "Page number (1-based)"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
    responses(
        (status = 200, description = "All workspaces", body = Vec<WorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<WorkspaceDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let page = Pagination {
        page: q.page.unwrap_or(1),
        per_page: q.per_page.unwrap_or(50),
    };
    let list = st.workspaces.list(page).await.map_err(AuthError)?;
    let ids: Vec<uuid::Uuid> = list.iter().map(|w| w.id.0).collect();
    let disabled = st
        .workspaces
        .disabled_features_for(&ids)
        .await
        .map_err(AuthError)?;
    Ok(Json(
        list.into_iter()
            .map(|w| {
                let off = disabled.get(&w.id.0);
                dto_with_features(w, off)
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspaces/{id}",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace detail", body = WorkspaceDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let ws = st
        .workspaces
        .get_by_id(WorkspaceId(id))
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;
    // A batch of one would be silly here — `get_ws` already has the single
    // workspace id it needs, so it reads the disabled set directly.
    let disabled = st
        .workspaces
        .disabled_features(WorkspaceId(id))
        .await
        .map_err(AuthError)?;
    Ok(Json(dto_with_features(ws, Some(&disabled))))
}

#[utoipa::path(
    put,
    path = "/api/admin/workspaces/{id}",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    request_body = UpdateWorkspace,
    responses(
        (status = 200, description = "Workspace updated", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkspace>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:update")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .update_name(WorkspaceId(id), &body.name)
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/admin/workspaces/{id}",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace archived", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn archive_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:archive")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .archive(WorkspaceId(id))
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspaces/{id}/stats",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace usage stats", body = WorkspaceStatsDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn workspace_stats(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceStatsDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let wid = WorkspaceId(id);
    let member_count = st.members.list(wid).await.map_err(AuthError)?.len() as i64;
    let query_count = st.workspaces.count_queries(wid).await.map_err(AuthError)?;
    Ok(Json(WorkspaceStatsDto {
        member_count,
        query_count,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspaces/{id}/restore",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace restored", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restore_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:archive")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .set_status(WorkspaceId(id), "active")
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspaces/{id}/transfer",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    request_body = TransferOwner,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceDto),
        (status = 400, description = "Invalid owner id", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn transfer_owner(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<TransferOwner>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:update")
        .await
        .map_err(AuthError)?;
    let new_owner = Uuid::parse_str(&body.new_owner_id)
        .map_err(|_| AuthError(Error::InvalidInput("new_owner_id must be a uuid".into())))?;
    let wid = WorkspaceId(id);

    // Ensure the workspace exists (404 otherwise).
    let _ = st
        .workspaces
        .get_by_id(wid)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;

    // Make the new owner a ws_admin (idempotent upsert; role-replace if already
    // a member). `MemberRepo::add(ws, ptype, pid, role, by)` — `by` is a UserId.
    st.members
        .add(wid, "user", new_owner, "ws_admin", p.user_id)
        .await
        .map_err(AuthError)?;

    st.workspaces
        .set_owner(wid, UserId(new_owner))
        .await
        .map_err(AuthError)?;

    st.audit.emit(egret_audit::mutation_for(
        &p,
        Some(id),
        "workspace:transfer_owner",
        "workspace",
        Some(id.to_string()),
        serde_json::json!({ "new_owner_id": new_owner }),
    ));

    let ws = st
        .workspaces
        .get_by_id(wid)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;
    Ok(Json(ws.into()))
}

// ── Workspace feature entitlements (Tasks 8/9) ───────────────────────────────
//
// Which of the five modules (`sql_lab`, `datasets`, `charts`, `dashboards`,
// `notebooks`) a workspace may use at all — distinct from RBAC, which decides
// what a *user* may do. Gated on the same platform-scoped permissions the
// sibling workspace-lifecycle routes already use: `workspace:read` for the
// read, `workspace:update` for the write. No new permission key: toggling a
// module is the same class of act as renaming or archiving a workspace.

/// One module and whether this workspace has it switched on.
#[derive(Serialize, utoipa::ToSchema)]
pub struct FeatureDto {
    pub key: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
}

/// The feature keys to change; any key this workspace's catalog entry already
/// has is left exactly as it is. This is a merge, not a full replacement,
/// despite arriving over `PUT`: `PUT` is still the right verb because the
/// operation is idempotent (sending the same body twice yields the same
/// state) — it is not required to mention every field of the resource. A
/// merge is also forward-safe: when a sixth module joins the catalog, a
/// caller that still only sends today's five keys leaves it untouched
/// instead of implicitly switching it on, which is what a true full
/// replacement (write/delete a row for every key, mentioned or not) would do.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetFeatures {
    /// Feature key -> enabled. Unknown keys are rejected with 400 and nothing
    /// is written (`WorkspaceRepo::set_features` validates the whole batch
    /// before opening its transaction). The *resulting* state — this batch
    /// merged onto what is already stored — is also validated against
    /// cross-module dependencies (currently: `dashboards` requires `charts`),
    /// so a body that never mentions `dashboards` can still be rejected if it
    /// would leave `dashboards` on and `charts` off.
    pub features: std::collections::BTreeMap<String, bool>,
}

fn features_dto(disabled: &std::collections::HashSet<String>) -> Vec<FeatureDto> {
    crate::features::FEATURES
        .iter()
        .map(|f| FeatureDto {
            key: f.key.to_string(),
            label: f.label.to_string(),
            description: f.description.to_string(),
            enabled: !disabled.contains(f.key),
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/admin/workspaces/{id}/features",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "The catalog of modules, each with this workspace's on/off state", body = Vec<FeatureDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ws_features(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<FeatureDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let disabled = st
        .workspaces
        .disabled_features(WorkspaceId(id))
        .await
        .map_err(AuthError)?;
    Ok(Json(features_dto(&disabled)))
}

#[utoipa::path(
    put,
    path = "/api/admin/workspaces/{id}/features",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    request_body = SetFeatures,
    responses(
        (status = 200, description = "The named keys were merged onto this workspace's state; the catalog with the resulting on/off state", body = Vec<FeatureDto>),
        (status = 400, description = "An unknown feature key, or a resulting state that violates a cross-module dependency (e.g. dashboards enabled with charts disabled); nothing was written", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_ws_features(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetFeatures>,
) -> Result<Json<Vec<FeatureDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:update")
        .await
        .map_err(AuthError)?;
    let wid = WorkspaceId(id);
    let before = st
        .workspaces
        .disabled_features(wid)
        .await
        .map_err(AuthError)?;
    let updates: Vec<(String, bool)> = body.features.iter().map(|(k, v)| (k.clone(), *v)).collect();
    // Always pass `Some(actor)` — `set_features` writes
    // `updated_by = EXCLUDED.updated_by`, so `None` would erase a previously
    // recorded actor.
    st.workspaces
        .set_features(wid, &updates, Some(p.user_id))
        .await
        .map_err(AuthError)?;
    let after = st
        .workspaces
        .disabled_features(wid)
        .await
        .map_err(AuthError)?;

    st.audit.emit(egret_audit::mutation_for(
        &p,
        Some(id),
        "workspace.features.update",
        "workspace",
        Some(id.to_string()),
        serde_json::json!({
            "before": before,
            "after": after,
        }),
    ));

    Ok(Json(features_dto(&after)))
}

// ── Me routes (Task 1 / Phase 0b) ────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/me/workspaces",
    tag = "me",
    responses(
        (status = 200, description = "Workspaces the caller can see (all, if platform admin)", body = Vec<WorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_my_workspaces(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
) -> Result<Json<Vec<WorkspaceDto>>, AuthError> {
    let is_admin = p.platform_roles.iter().any(|r| r == "platform_admin");
    let list = st
        .workspaces
        .list_for_principal(p.user_id, is_admin)
        .await
        .map_err(AuthError)?;

    // Containment: a ws-scoped impersonation (a ws_admin acting as a member) is
    // confined to the one workspace it was started in — even though the target may
    // belong to others. Show only that workspace, matching the `WsContext` guard
    // that denies every other one outright.
    let list = match p.impersonator.as_ref().and_then(|i| i.workspace_scope) {
        Some(scope) => list.into_iter().filter(|w| w.id == scope).collect(),
        None => list,
    };

    // One batched query for the whole list — the workspace switcher must not
    // issue a query per workspace.
    let ids: Vec<uuid::Uuid> = list.iter().map(|w| w.id.0).collect();
    let disabled = st
        .workspaces
        .disabled_features_for(&ids)
        .await
        .map_err(AuthError)?;

    Ok(Json(
        list.into_iter()
            .map(|w| {
                let off = disabled.get(&w.id.0);
                dto_with_features(w, off)
            })
            .collect(),
    ))
}

pub fn me_router() -> Router<WorkspaceState> {
    Router::new().route("/workspaces", get(list_my_workspaces))
}

// ── Access-request (join-request) routes — requester side (P2b) ───────────────
//
// Any authenticated user (`Auth(p)`, NOT workspace-scoped `WsContext`) may list
// the workspaces they can join, file a join request, list their own requests, and
// cancel a still-pending one. Approval lives on the workspace-scoped approver
// router. Mounted at `/api`.

/// A workspace a user may request to join (they are not already a member).
#[derive(Serialize, utoipa::ToSchema)]
pub struct JoinableWorkspaceDto {
    pub id: String,
    pub slug: String,
    pub name: String,
}

/// A join request as seen by the requester.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AccessRequestDto {
    pub id: String,
    pub workspace_id: String,
    pub workspace_name: Option<String>,
    pub requester_id: String,
    pub requester_name: Option<String>,
    pub requested_role: String,
    pub reason: String,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub decision_note: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

impl From<AccessRequestEnriched> for AccessRequestDto {
    fn from(r: AccessRequestEnriched) -> Self {
        AccessRequestDto {
            id: r.id.to_string(),
            workspace_id: r.workspace_id.to_string(),
            workspace_name: r.workspace_name,
            requester_id: r.requester_id.to_string(),
            requester_name: r.requester_name,
            requested_role: r.requested_role,
            reason: r.reason,
            status: r.status,
            decided_by: r.decided_by.map(|id| id.to_string()),
            decided_at: r.decided_at.map(|t| t.to_rfc3339()),
            decision_note: r.decision_note,
            expires_at: r.expires_at.map(|t| t.to_rfc3339()),
            created_at: r.created_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateAccessRequest {
    pub workspace_id: Uuid,
    pub role: Option<String>,
    pub reason: Option<String>,
}

pub fn access_request_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/access-requests/joinable", get(list_joinable))
        .route(
            "/access-requests",
            post(create_access_request).get(list_my_access_requests),
        )
        .route("/access-requests/mine", get(list_my_access_requests))
        .route("/access-requests/{id}/cancel", post(cancel_access_request))
}

#[utoipa::path(
    get,
    path = "/api/access-requests/joinable",
    tag = "access-requests",
    responses(
        (status = 200, description = "Active workspaces the caller may request to join", body = Vec<JoinableWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_joinable(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
) -> Result<Json<Vec<JoinableWorkspaceDto>>, AuthError> {
    let rows = st
        .access_requests
        .joinable(p.user_id)
        .await
        .map_err(AuthError)?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, slug, name)| JoinableWorkspaceDto {
                id: id.to_string(),
                slug,
                name,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/access-requests",
    tag = "access-requests",
    request_body = CreateAccessRequest,
    responses(
        (status = 201, description = "Join request filed", body = AccessRequestDto),
        (status = 400, description = "Already a member or workspace not joinable", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 409, description = "A pending request for this workspace already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_access_request(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Json(body): Json<CreateAccessRequest>,
) -> Result<(axum::http::StatusCode, Json<AccessRequestDto>), AuthError> {
    // Only workspaces the caller is NOT already a member of (and that are active)
    // are valid targets. `joinable` is the authoritative membership check: if the
    // target isn't in it, the caller is already a member or the ws is inactive.
    let joinable = st
        .access_requests
        .joinable(p.user_id)
        .await
        .map_err(AuthError)?;
    if !joinable.iter().any(|(id, _, _)| *id == body.workspace_id) {
        return Err(AuthError(Error::Conflict(
            "already a member or workspace not joinable".into(),
        )));
    }

    let role = body.role.as_deref().unwrap_or("ws_viewer");
    let reason = body.reason.as_deref().unwrap_or("");
    let req = st
        .access_requests
        .create(p.user_id, WorkspaceId(body.workspace_id), role, reason)
        .await
        .map_err(AuthError)?;

    // Return the enriched DTO shape (workspace/requester names are None on the
    // freshly-created row — the requester already knows both).
    let enriched = AccessRequestEnriched {
        id: req.id,
        requester_id: req.requester_id,
        workspace_id: req.workspace_id,
        requested_role: req.requested_role,
        reason: req.reason,
        status: req.status,
        decided_by: req.decided_by,
        decided_at: req.decided_at,
        decision_note: req.decision_note,
        expires_at: req.expires_at,
        created_at: req.created_at,
        requester_name: None,
        workspace_name: None,
    };
    Ok((axum::http::StatusCode::CREATED, Json(enriched.into())))
}

#[utoipa::path(
    get,
    path = "/api/access-requests/mine",
    tag = "access-requests",
    responses(
        (status = 200, description = "The caller's own join requests, newest first", body = Vec<AccessRequestDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_my_access_requests(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
) -> Result<Json<Vec<AccessRequestDto>>, AuthError> {
    let rows = st
        .access_requests
        .list_for_requester(p.user_id)
        .await
        .map_err(AuthError)?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/access-requests/{id}/cancel",
    tag = "access-requests",
    params(("id" = Uuid, Path, description = "Access-request id")),
    responses(
        (status = 200, description = "Request cancelled", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 404, description = "No pending request to cancel for this user", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn cancel_access_request(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.access_requests
        .cancel(id, p.user_id)
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Admin user routes (Task 8) ───────────────────────────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub name: String,
    pub provider: String,
    pub status: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetPlatformRoles {
    pub roles: Vec<String>,
}

pub fn admin_user_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}/platform-roles", put(set_platform_roles))
}

#[utoipa::path(
    get,
    path = "/api/admin/users",
    tag = "admin-users",
    params(
        ("page" = Option<u32>, Query, description = "Page number (1-based)"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
    responses(
        (status = 200, description = "All users", body = Vec<UserDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_users(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<UserDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "user:read")
        .await
        .map_err(AuthError)?;
    let page = Pagination {
        page: q.page.unwrap_or(1),
        per_page: q.per_page.unwrap_or(50),
    };
    let users = st
        .users
        .list(page.limit() as i64, page.offset() as i64)
        .await
        .map_err(AuthError)?;
    Ok(Json(
        users
            .into_iter()
            .map(|u| UserDto {
                id: u.id.to_string(),
                email: u.email,
                name: u.name,
                provider: u.provider,
                status: u.status,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/admin/users/{id}",
    tag = "admin-users",
    params(("id" = Uuid, Path, description = "User id")),
    responses(
        (status = 200, description = "User detail", body = UserDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_user(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "user:read")
        .await
        .map_err(AuthError)?;
    let u = st
        .users
        .find_by_id(UserId(id))
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("user".into())))?;
    Ok(Json(UserDto {
        id: u.id.to_string(),
        email: u.email,
        name: u.name,
        provider: u.provider,
        status: u.status,
    }))
}

#[utoipa::path(
    put,
    path = "/api/admin/users/{id}/platform-roles",
    tag = "admin-users",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = SetPlatformRoles,
    responses(
        (status = 200, description = "Platform roles replaced", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Not a platform admin (role assignment is not delegatable)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_platform_roles(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetPlatformRoles>,
) -> Result<Json<serde_json::Value>, AuthError> {
    // Replacing a user's platform role assignments (which can include
    // `platform_admin` itself) mutates the authorization graph directly, so
    // this must not be delegatable — a holder of a granular permission like
    // the former `user:assign_role` could otherwise grant themselves any
    // role. `user:assign_platform_role` is an admin-graph action: the PDP
    // `forbid`s it for everyone but `platform_admin` (non-delegatable, not in
    // the grantable catalog), same as the equivalent control-plane
    // role-assignment endpoints in `egret-controlplane/src/access/mod.rs`.
    egret_core::authz::require(&*st.pdp, &p, "user:assign_platform_role")
        .await
        .map_err(AuthError)?;
    st.authz
        .replace_platform_roles(UserId(id), &body.roles)
        .await
        .map_err(AuthError)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "platform_roles:set",
        "grant",
        Some(id.to_string()),
        serde_json::json!({ "user_id": id, "roles": body.roles }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Workspace member routes (Task 9) ─────────────────────────────────────────

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AddMember {
    pub principal_type: String,
    pub principal_id: String,
    pub role: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MemberDto {
    pub principal_type: String,
    pub principal_id: String,
    pub role: String,
    pub principal_name: Option<String>,
    pub granted_by_name: Option<String>,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub is_platform_admin: bool,
}

pub fn ws_member_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/{workspace}/impersonate", post(ws_impersonate_start))
        .route("/{workspace}/members", get(list_members).post(add_member))
        .route("/{workspace}/members/{ptype}/{pid}", delete(remove_member))
        .route("/{workspace}/roles", get(list_workspace_roles))
        .route("/{workspace}/shares", get(list_workspace_shares))
        .route(
            "/{workspace}/access-requests",
            get(list_workspace_access_requests),
        )
        .route(
            "/{workspace}/access-requests/{id}/approve",
            post(approve_access_request),
        )
        .route(
            "/{workspace}/access-requests/{id}/deny",
            post(deny_access_request),
        )
}

/// Body for `POST /api/ws/{workspace}/impersonate`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct StartWsImpersonation {
    pub target_user_id: String,
    pub reason: String,
    /// Required for a local-auth caller (step-up). OIDC callers re-auth via OIDC.
    pub password: Option<String>,
}

/// `POST /api/ws/{workspace}/impersonate` — a `ws_admin` of `{workspace}` (or a
/// platform_admin) starts impersonating a MEMBER of that workspace. The resulting
/// overlay is **workspace-scoped**: `WsContext` confines it to `{workspace}` even
/// though the target may belong to others (see the containment guard there).
///
/// Guards run in a deliberate order so a caller learns nothing before its own
/// authority is proven: (1) not already impersonating; (2) caller is `ws_admin`
/// here or a platform_admin; (3) the shared target guards (not-self, reason
/// non-empty, target exists, and — because the overlay THIS endpoint writes is
/// workspace-scoped — the target holds NO platform-scoped role grant at all;
/// `validate_impersonation_target`, and read its ⚠ paragraph: that third guard is
/// what stops this endpoint minting a scoped overlay with platform authority,
/// which no `/api/control-plane` route confines, and it is deliberately WIDER here
/// than on the platform endpoint); (4) the target is a
/// MEMBER of this workspace and is NOT itself a `ws_admin` of it. Step-up re-auth
/// then mirrors the platform endpoint: a local caller re-enters their password; an
/// OIDC caller gets a `409` with an authorize URL whose sealed intent carries this
/// `workspace_id`, and the overlay is created by the OIDC callback.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/impersonate",
    tag = "workspace-members",
    params(WorkspacePath),
    request_body = StartWsImpersonation,
    responses(
        (status = 200, description = "Impersonation started; the session now resolves to the target, scoped to this workspace"),
        (status = 400, description = "Bad target / empty reason", body = ErrorBody),
        (status = 401, description = "Not authenticated, or wrong step-up password", body = ErrorBody),
        (status = 403, description = "Not a ws_admin, already impersonating, target not a member, target is a ws_admin here, or the target holds ANY platform-scoped role grant — wider than the platform endpoint's rule, because the overlay this one writes is workspace-scoped and no /api/control-plane route confines such an overlay", body = ErrorBody),
        (status = 409, description = "OIDC caller: re-authenticate at `authorize_url`, then the callback creates the overlay"),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ws_impersonate_start(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    jar: axum_extra::extract::cookie::CookieJar,
    Json(body): Json<StartWsImpersonation>,
) -> Result<axum::response::Response, AuthError> {
    use axum::response::IntoResponse;

    // 1. No nesting: a caller already impersonating cannot start another. Checked
    //    before anything reads the target, so it leaks nothing.
    if ctx.principal.impersonator.is_some() {
        return Err(AuthError(Error::Forbidden));
    }
    // 2. Caller authority: a `ws_admin` of THIS workspace, or a platform_admin.
    //    Learn nothing about the target before this passes.
    if ctx.workspace_role != "ws_admin" && !ctx.is_platform_admin() {
        return Err(AuthError(Error::Forbidden));
    }
    // 3. Parse the target and run the SHARED target guards (not-self, reason
    //    non-empty, target exists, target holds no platform-scoped role grant) —
    //    identical to the platform start endpoint and the OIDC callback.
    let target = UserId(
        body.target_user_id
            .parse::<Uuid>()
            .map_err(|_| AuthError(Error::InvalidInput("invalid target_user_id".into())))?,
    );
    let reason = body.reason.trim().to_string();
    egret_auth::impersonation::validate_impersonation_target(
        &st.users,
        &st.sessions,
        ctx.principal.user_id,
        target,
        &reason,
        // `Some`: the overlay this endpoint writes is confined to THIS workspace,
        // which is what makes the wide target rule apply here and not on the
        // platform endpoint.
        Some(ctx.workspace_id),
    )
    .await
    .map_err(AuthError)?;
    // 4. Workspace-specific target guards: the target must be a MEMBER of THIS
    //    workspace (an empty role set means they are not) and must not be a
    //    `ws_admin` of it. Both catch group-derived roles, not just direct grants.
    //
    //    The rule lives in `validate_ws_impersonation_target_membership` and is
    //    CALLED here, because the OIDC step-up callback mints this same ws-scoped
    //    overlay after an IdP round trip and had omitted the pair entirely
    //    (whole-branch review, M-1) — one function, two call sites, so the two
    //    cannot drift.
    egret_auth::impersonation::validate_ws_impersonation_target_membership(
        &st.sessions,
        target,
        ctx.workspace_id,
    )
    .await
    .map_err(AuthError)?;
    // 5. Step-up re-authentication of the CALLER. A local caller re-enters their
    //    password inline; an OIDC caller must re-authenticate with the IdP, so we
    //    return a 409 carrying the authorize URL with the impersonation intent —
    //    including this `workspace_id` — sealed into the encrypted state cookie.
    //    The overlay is then created by the OIDC callback (which re-checks
    //    `ws_admin` on this workspace before writing it).
    let caller = st
        .users
        .find_by_id(ctx.principal.user_id)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::Unauthorized))?;
    if caller.provider != "local" {
        let (Some(oidc), Some(key)) = (st.oidc.as_ref(), st.cookie_key.as_ref()) else {
            return Err(AuthError(Error::Internal(anyhow::anyhow!(
                "OIDC step-up requested but OIDC is not configured"
            ))));
        };
        return Ok(egret_auth::routes::build_impersonation_step_up(
            oidc,
            key,
            st.session_secure,
            ctx.principal.user_id,
            target,
            reason,
            Some(ctx.workspace_id),
        ));
    }
    let hash = caller
        .password_hash
        .as_deref()
        .ok_or(AuthError(Error::Unauthorized))?;
    let ok = egret_auth::password::verify_password(body.password.as_deref().unwrap_or(""), hash)
        .map_err(AuthError)?;
    if !ok {
        return Err(AuthError(Error::Unauthorized));
    }
    // 6. Write the workspace-scoped overlay on THIS caller's session. A slipped-
    //    through nesting Conflict maps to Forbidden.
    let sid = session_id_from_jar(&st, &jar).await?;
    st.impersonations
        .start(egret_auth::impersonation::NewImpersonation {
            admin_session_id: sid,
            impersonator_id: ctx.principal.user_id,
            target_user_id: target,
            workspace_scope: Some(ctx.workspace_id),
            reason: reason.clone(),
            ttl_minutes: 30,
        })
        .await
        .map_err(|e| match e {
            Error::Conflict(_) => AuthError(Error::Forbidden),
            other => AuthError(other),
        })?;
    // 7. Audit, stamped with this workspace.
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(ctx.principal.user_id.0),
        impersonator: Some(ctx.principal.user_id.0),
        workspace_id: Some(ctx.workspace_id.0),
        action: "impersonation.start".into(),
        resource_type: "user".into(),
        resource_id: Some(target.0.to_string()),
        detail: serde_json::json!({
            "reason": reason,
            "scope": ctx.workspace_id.0.to_string(),
        }),
    });
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

/// Resolve THIS request's `sessions.id` from the session cookie. The overlay is
/// keyed on the caller's own session row (same convention as the platform start
/// endpoint's `session_id_from_jar`).
async fn session_id_from_jar(
    st: &WorkspaceState,
    jar: &axum_extra::extract::cookie::CookieJar,
) -> Result<Uuid, AuthError> {
    let token = egret_auth::cookie::read_session(jar, egret_auth::cookie::secure_cookies())
        .map(|c| c.value().to_string())
        .ok_or(AuthError(Error::Unauthorized))?;
    st.sessions
        .session_id_for_token(&token)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::Unauthorized))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/members",
    tag = "workspace-members",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Members of the workspace", body = Vec<MemberDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_members(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<MemberDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    let members = st.members.list(ctx.workspace_id).await.map_err(AuthError)?;
    Ok(Json(
        members
            .into_iter()
            .map(|m| MemberDto {
                principal_type: m.principal_type,
                principal_id: m.principal_id.to_string(),
                role: m.role,
                principal_name: m.principal_name,
                granted_by_name: m.granted_by_name,
                added_at: m.added_at,
                is_platform_admin: m.is_platform_admin,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/members",
    tag = "workspace-members",
    params(WorkspacePath),
    request_body = AddMember,
    responses(
        (status = 200, description = "Member added", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Json(body): Json<AddMember>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    // The picker supplies an email / group name; a role change supplies the existing
    // member's UUID. Accept a real UUID directly, else resolve email/name -> id.
    let pid: uuid::Uuid = match uuid::Uuid::parse_str(&body.principal_id) {
        Ok(u) => u,
        Err(_) => st
            .members
            .resolve_principal(&body.principal_type, &body.principal_id)
            .await
            .map_err(AuthError)?
            .ok_or_else(|| {
                AuthError(egret_core::Error::InvalidInput(format!(
                    "unknown {}: {}",
                    body.principal_type, body.principal_id
                )))
            })?,
    };
    // A role change is a replace; demoting the last ws_admin is a last-admin removal.
    if body.role != "ws_admin"
        && st
            .members
            .role_of(ctx.workspace_id, &body.principal_type, pid)
            .await
            .map_err(AuthError)?
            .as_deref()
            == Some("ws_admin")
        && st
            .members
            .count_ws_admins(ctx.workspace_id)
            .await
            .map_err(AuthError)?
            <= 1
    {
        return Err(AuthError(egret_core::Error::Conflict(
            "the workspace must keep at least one admin".into(),
        )));
    }
    st.members
        .add(
            ctx.workspace_id,
            &body.principal_type,
            pid,
            &body.role,
            ctx.principal.user_id,
        )
        .await
        .map_err(AuthError)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "member:add",
        "grant",
        None,
        serde_json::json!({
            "principal_type": body.principal_type,
            "principal_id": pid,
            "role": body.role,
        }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/members/{ptype}/{pid}",
    tag = "workspace-members",
    params(
        WorkspacePath,
        ("ptype" = String, Path, description = "Principal type (user | group)"),
        ("pid" = Uuid, Path, description = "Principal id"),
    ),
    responses(
        (status = 200, description = "Member removed", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Path((_ws, ptype, pid)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    // Don't let a workspace lose its last admin (it would become unmanageable
    // without platform intervention).
    if st
        .members
        .role_of(ctx.workspace_id, &ptype, pid)
        .await
        .map_err(AuthError)?
        .as_deref()
        == Some("ws_admin")
        && st
            .members
            .count_ws_admins(ctx.workspace_id)
            .await
            .map_err(AuthError)?
            <= 1
    {
        return Err(AuthError(egret_core::Error::Conflict(
            "the workspace must keep at least one admin".into(),
        )));
    }
    st.members
        .remove(ctx.workspace_id, &ptype, pid)
        .await
        .map_err(AuthError)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "member:remove",
        "grant",
        None,
        serde_json::json!({
            "principal_type": ptype,
            "principal_id": pid,
        }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Workspace role catalog (read-only) ───────────────────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
pub struct WsPermDto {
    pub key: String,
    pub description: String,
    pub group: String,
}
impl From<crate::access_views::WsPerm> for WsPermDto {
    fn from(p: crate::access_views::WsPerm) -> Self {
        WsPermDto {
            key: p.key,
            description: p.description,
            group: p.group,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct WsRoleDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parent_role_id: Option<String>,
    pub permissions: Vec<WsPermDto>,
}
impl From<crate::access_views::WsRole> for WsRoleDto {
    fn from(r: crate::access_views::WsRole) -> Self {
        WsRoleDto {
            id: r.id.to_string(),
            name: r.name,
            description: r.description,
            parent_role_id: r.parent_role_id.map(|p| p.to_string()),
            permissions: r.permissions.into_iter().map(Into::into).collect(),
        }
    }
}

/// `GET /ws/{workspace}/roles` — the workspace-scope role catalog with each
/// role's direct permissions. Read-only, so a workspace admin can SEE what each
/// role grants; gated on `workspace:manage_members` (the same gate as the
/// member routes) rather than the platform-level `role:read`.
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/roles",
    tag = "workspace-members",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Workspace-scope roles with their permissions", body = Vec<WsRoleDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_workspace_roles(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WsRoleDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    let roles = st
        .access_views
        .list_workspace_roles_with_perms()
        .await
        .map_err(AuthError)?;
    Ok(Json(roles.into_iter().map(Into::into).collect()))
}

// ── Workspace object-grant roll-up (read-only) ───────────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
pub struct ObjectGrantRollupDto {
    pub kind: String,
    pub object_id: String,
    pub object_name: Option<String>,
    pub principal_type: String,
    pub principal_id: String,
    pub principal_name: Option<String>,
    pub role: String,
    pub effect: String,
    /// Optional expiry (RFC3339). `None` = never expires. This is the
    /// management roll-up, so an expired grant is still returned (not filtered).
    pub expires_at: Option<String>,
}
impl From<crate::access_views::ObjectGrantRollup> for ObjectGrantRollupDto {
    fn from(g: crate::access_views::ObjectGrantRollup) -> Self {
        ObjectGrantRollupDto {
            kind: g.kind,
            object_id: g.object_id.to_string(),
            object_name: g.object_name,
            principal_type: g.principal_type,
            principal_id: g.principal_id.to_string(),
            principal_name: g.principal_name,
            role: g.role,
            effect: g.effect,
            expires_at: g.expires_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// `GET /ws/{workspace}/shares` — every OBJECT grant whose object lives in THIS
/// workspace (charts, dashboards, datasets, folders, saved queries, apps),
/// enriched with object name, role name, principal name and effect. Read-only, so
/// a workspace admin can SEE what has been shared; gated on
/// `workspace:manage_members` (the same gate as the member/role routes). The
/// object set is restricted to this workspace in SQL — a grant on an object in
/// another workspace is never returned (cross-workspace IDOR guard).
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/shares",
    tag = "workspace-members",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Object grants whose object lives in this workspace", body = Vec<ObjectGrantRollupDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_workspace_shares(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<ObjectGrantRollupDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    let grants = st
        .access_views
        .list_object_grants(ctx.workspace_id)
        .await
        .map_err(AuthError)?;
    Ok(Json(grants.into_iter().map(Into::into).collect()))
}

// ── Workspace access-request approver routes (P2b) ───────────────────────────
//
// The APPROVER side of the join-request workflow: a workspace admin reviews the
// pending queue and approves (→ member added with role + optional expiry via the
// repo's single grant path) or denies. Workspace-scoped and gated on
// `workspace:manage_members` (same as the member routes). The cross-workspace +
// pending guards live in `AccessRequestRepo::{approve,deny}` — surfaced here as
// 404 (`NotFound`) / 409 (`Conflict`) via `AuthError`.

#[derive(Deserialize)]
pub struct AccessRequestStatusQuery {
    /// Filter by request status; defaults to `pending`.
    pub status: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ApproveAccessRequest {
    /// Role to grant the requester; defaults to the originally requested role.
    pub role: Option<String>,
    /// Optional time-boxed expiry for the membership grant.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DenyAccessRequest {
    /// Optional note recorded on the denied request.
    pub note: Option<String>,
}

/// `GET /ws/{workspace}/access-requests?status=` — the workspace's join requests
/// filtered by `status` (default `pending`). Gated on `workspace:manage_members`.
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/access-requests",
    tag = "workspace-members",
    params(
        WorkspacePath,
        ("status" = Option<String>, Query, description = "Filter by status (default: pending)"),
    ),
    responses(
        (status = 200, description = "The workspace's join requests, newest first", body = Vec<AccessRequestDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_workspace_access_requests(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Query(q): Query<AccessRequestStatusQuery>,
) -> Result<Json<Vec<AccessRequestDto>>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    let status = q.status.as_deref().unwrap_or("pending");
    let rows = st
        .access_requests
        .list_for_workspace(ctx.workspace_id, status)
        .await
        .map_err(AuthError)?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// `POST /ws/{workspace}/access-requests/{id}/approve` — approve a pending request:
/// add the requester as a member with the chosen role + optional expiry via the
/// repo's single grant path, then mark it approved. The repo enforces the
/// cross-workspace guard (`NotFound` → 404) and pending guard (`Conflict` → 409).
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/access-requests/{id}/approve",
    tag = "workspace-members",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Access-request id"),
    ),
    request_body = ApproveAccessRequest,
    responses(
        (status = 200, description = "Request approved and member added", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such request in this workspace", body = ErrorBody),
        (status = 409, description = "Request already decided", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn approve_access_request(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<ApproveAccessRequest>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    st.access_requests
        .approve(
            id,
            ctx.workspace_id,
            body.role.as_deref(),
            body.expires_at,
            ctx.principal.user_id,
        )
        .await
        .map_err(AuthError)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "access_request:approve",
        "access_request",
        Some(id.to_string()),
        serde_json::json!({
            "role": body.role,
            "expires_at": body.expires_at.map(|t| t.to_rfc3339()),
        }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /ws/{workspace}/access-requests/{id}/deny` — deny a pending request with
/// an optional note. The repo enforces the cross-workspace + pending guards.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/access-requests/{id}/deny",
    tag = "workspace-members",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Access-request id"),
    ),
    request_body = DenyAccessRequest,
    responses(
        (status = 200, description = "Request denied", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 409, description = "Request not pending or not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn deny_access_request(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<DenyAccessRequest>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    st.access_requests
        .deny(
            id,
            ctx.workspace_id,
            body.note.as_deref(),
            ctx.principal.user_id,
        )
        .await
        .map_err(AuthError)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "access_request:deny",
        "access_request",
        Some(id.to_string()),
        serde_json::json!({ "note": body.note }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── OpenAPI fragments (E0·P4b-2) ─────────────────────────────────────────────
//
// One doc struct per router, merged in `egret_workspace::openapi()`. Full mounted
// paths are traced in `egret-server`'s `build_router`: `admin_workspace_router` +
// `admin_user_router` nested at `/api/admin`, `ws_member_router` at `/api/ws`,
// `me_router` at `/api/me`.

/// OpenAPI fragment for the "me" router (`/api/me`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_my_workspaces),
    components(schemas(WorkspaceDto, ErrorBody)),
    tags((name = "me", description = "The caller's own workspaces")),
)]
pub struct MeApi;

/// OpenAPI fragment for the requester-side access-request router (`/api`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_joinable, create_access_request, list_my_access_requests, cancel_access_request),
    components(schemas(JoinableWorkspaceDto, AccessRequestDto, CreateAccessRequest, Ack, ErrorBody)),
    tags((name = "access-requests", description = "Workspace join requests (requester side)")),
)]
pub struct AccessRequestApi;

/// OpenAPI fragment for the platform-admin workspace router (`/api/admin/workspaces`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        create_ws,
        list_ws,
        get_ws,
        update_ws,
        archive_ws,
        restore_ws,
        workspace_stats,
        transfer_owner,
        ws_features,
        set_ws_features,
    ),
    components(schemas(
        WorkspaceDto,
        WorkspaceStatsDto,
        TransferOwner,
        CreateWorkspace,
        UpdateWorkspace,
        FeatureDto,
        SetFeatures,
        Ack,
        ErrorBody,
    )),
    tags((name = "admin-workspaces", description = "Platform-admin workspace lifecycle")),
)]
pub struct AdminWorkspaceApi;

/// OpenAPI fragment for the platform-admin user router (`/api/admin/users`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_users, get_user, set_platform_roles),
    components(schemas(UserDto, SetPlatformRoles, Ack, ErrorBody)),
    tags((name = "admin-users", description = "Platform-admin user directory + platform-role assignment")),
)]
pub struct AdminUserApi;

/// OpenAPI fragment for the workspace-membership router (`/api/ws/{workspace}/members`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        ws_impersonate_start,
        list_members,
        add_member,
        remove_member,
        list_workspace_roles,
        list_workspace_shares,
        list_workspace_access_requests,
        approve_access_request,
        deny_access_request,
    ),
    components(schemas(
        StartWsImpersonation,
        MemberDto,
        AddMember,
        WsRoleDto,
        WsPermDto,
        ObjectGrantRollupDto,
        AccessRequestDto,
        ApproveAccessRequest,
        DenyAccessRequest,
        Ack,
        ErrorBody,
    )),
    tags((name = "workspace-members", description = "Workspace membership (add/remove principals)")),
)]
pub struct WsMemberApi;
