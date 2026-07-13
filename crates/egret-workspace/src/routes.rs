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
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub users: UserRepo,
    pub sessions: SessionService,
    /// Append-only audit sink (B4). Emits Mutation events at platform-role and
    /// membership changes; best-effort, never fails the request.
    pub audit: egret_audit::AuditSink,
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
}
impl From<crate::model::Workspace> for WorkspaceDto {
    fn from(w: crate::model::Workspace) -> Self {
        WorkspaceDto {
            id: w.id.to_string(),
            slug: w.slug,
            name: w.name,
            status: w.status,
        }
    }
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
    Ok(Json(list.into_iter().map(Into::into).collect()))
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
    Ok(Json(ws.into()))
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
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

pub fn me_router() -> Router<WorkspaceState> {
    Router::new().route("/workspaces", get(list_my_workspaces))
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
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(p.user_id.0),
        workspace_id: None,
        action: "platform_roles:set".into(),
        resource_type: "grant".into(),
        resource_id: Some(id.to_string()),
        detail: serde_json::json!({ "user_id": id, "roles": body.roles }),
    });
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
        .route("/{workspace}/members", get(list_members).post(add_member))
        .route("/{workspace}/members/{ptype}/{pid}", delete(remove_member))
        .route("/{workspace}/roles", get(list_workspace_roles))
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
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(ctx.principal.user_id.0),
        workspace_id: Some(ctx.workspace_id.0),
        action: "member:add".into(),
        resource_type: "grant".into(),
        resource_id: None,
        detail: serde_json::json!({
            "principal_type": body.principal_type,
            "principal_id": pid,
            "role": body.role,
        }),
    });
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
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(ctx.principal.user_id.0),
        workspace_id: Some(ctx.workspace_id.0),
        action: "member:remove".into(),
        resource_type: "grant".into(),
        resource_id: None,
        detail: serde_json::json!({
            "principal_type": ptype,
            "principal_id": pid,
        }),
    });
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

/// OpenAPI fragment for the platform-admin workspace router (`/api/admin/workspaces`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(create_ws, list_ws, get_ws, update_ws, archive_ws),
    components(schemas(WorkspaceDto, CreateWorkspace, UpdateWorkspace, Ack, ErrorBody)),
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
    paths(list_members, add_member, remove_member, list_workspace_roles),
    components(schemas(MemberDto, AddMember, WsRoleDto, WsPermDto, Ack, ErrorBody)),
    tags((name = "workspace-members", description = "Workspace membership (add/remove principals)")),
)]
pub struct WsMemberApi;
