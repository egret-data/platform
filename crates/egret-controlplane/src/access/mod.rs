//! Control-plane Access router: Users, Groups, Roles, and the Permission
//! catalog — CRUD + role-assignment endpoints, nested under
//! `/api/control-plane`.
//!
//! Follows the module/router/handler idiom of `secrets.rs`: read and
//! user/group-lifecycle handlers gate on `require_permission(&p,
//! "resource:action")?` (delegatable; `platform_admin` always bypasses), but
//! every handler that mutates the authorization graph itself — role
//! definitions, role-permission sets, role assignments, and group membership
//! (which can carry roles) — gates on `require_platform_admin(&p)?` instead,
//! since a delegated `role:*`/`user:assign_role`/`group:manage_members`
//! permission would otherwise let a non-admin grant themselves any
//! permission. RBAC roles are mounted at `/access/roles` (not `/roles`) to
//! avoid colliding with the embedded-catalog `roles`/`role_id` routes already
//! served by `warehouses.rs`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::roles::is_seeded;
use egret_core::rbac;
use egret_core::UserId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use egret_core::http::ErrorBody;

use crate::auth::{require_permission, require_platform_admin};
use crate::error::CpError;
use crate::CpState;

/// The platform role every last-admin guard protects.
const PLATFORM_ADMIN_ROLE: &str = "platform_admin";
/// Cap for the admin Users/Groups/Roles list views (control-plane admin
/// surfaces; not paginated in the UI yet).
const LIST_LIMIT: i64 = 1000;

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", patch(update_user))
        .route("/users/{id}/roles", post(assign_user_role))
        .route("/users/{id}/roles/{role_id}", delete(unassign_user_role))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", patch(rename_group).delete(delete_group))
        .route(
            "/groups/{id}/members",
            get(list_group_members).post(add_group_member),
        )
        .route(
            "/groups/{id}/members/{user_id}",
            delete(remove_group_member),
        )
        .route("/groups/{id}/roles", post(assign_group_role))
        .route("/groups/{id}/roles/{role_id}", delete(unassign_group_role))
        .route("/access/roles", get(list_roles).post(create_role))
        .route("/access/roles/{id}", patch(update_role).delete(delete_role))
        .route("/access/roles/{id}/permissions", put(set_role_permissions))
        .route("/access/roles/{id}/assignees", get(list_role_assignees))
        .route("/permissions", get(list_permissions))
}

/// Reject removing a role assignment that holds the last `platform_admin`
/// assignment platform-wide — shared by the user- and group-role unassign
/// endpoints.
async fn guard_last_admin(st: &CpState, role_id: Uuid) -> Result<(), CpError> {
    let role = st.roles.get(role_id).await?.ok_or(CpError::NotFound)?;
    if role.name == PLATFORM_ADMIN_ROLE && st.roles.count_platform_admins().await? == 1 {
        return Err(CpError::Conflict(
            "Cannot remove the last platform admin".to_string(),
        ));
    }
    Ok(())
}

// ---------- Users ----------

// Renamed in the emitted OpenAPI to `ControlPlaneUserDto`: egret-workspace
// already publishes a different-shaped `UserDto` (no roles/groups), and utoipa
// keys schemas by name globally after merge — an unqualified `UserDto` here
// would silently clobber it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = ControlPlaneUserDto)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub provider: String,
    pub status: String,
    pub roles: Vec<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct CreateUserInput {
    email: String,
    name: String,
    password: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct UpdateUserInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct AssignRoleInput {
    role_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/users",
    tag = "control-plane-access",
    responses(
        (status = 200, description = "Platform users with their roles and groups", body = Vec<UserDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_users(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<UserDto>>, CpError> {
    require_permission(&p, "user:read")?;
    let users = st.users.list(LIST_LIMIT, 0).await?;
    let mut out = Vec::with_capacity(users.len());
    for u in users {
        let roles = st
            .roles
            .roles_of("user", u.id.0)
            .await?
            .into_iter()
            .map(|r| r.name)
            .collect();
        let groups = st.groups.groups_of_user(u.id).await?;
        out.push(UserDto {
            id: u.id.0,
            email: u.email,
            name: u.name,
            provider: u.provider,
            status: u.status,
            roles,
            groups,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/users",
    tag = "control-plane-access",
    request_body = CreateUserInput,
    responses(
        (status = 201, description = "Local user created", body = UserDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:create permission", body = ErrorBody),
        (status = 409, description = "A user with this email already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_user(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateUserInput>,
) -> Result<(StatusCode, Json<UserDto>), CpError> {
    require_permission(&p, "user:create")?;
    let hash = egret_auth::password::hash_password(&body.password)?;
    let id = st
        .users
        .insert_local(&body.email, &body.name, &hash)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(UserDto {
            id: id.0,
            email: body.email,
            name: body.name,
            provider: "local".to_string(),
            status: "active".to_string(),
            roles: vec![],
            groups: vec![],
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/control-plane/users/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = UpdateUserInput,
    responses(
        (status = 200, description = "User profile and/or status updated", body = UserDto),
        (status = 400, description = "Invalid status value", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update (or user:disable) permission", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_user(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserInput>,
) -> Result<Json<UserDto>, CpError> {
    require_permission(&p, "user:update")?;
    let user_id = UserId(id);
    let existing = st
        .users
        .find_by_id(user_id)
        .await?
        .ok_or(CpError::NotFound)?;

    if let Some(name) = &body.name {
        st.users
            .update_profile(user_id, &existing.email, name)
            .await?;
    }
    if let Some(status) = &body.status {
        if status != "active" && status != "disabled" {
            return Err(CpError::BadRequest(format!("invalid status: {status}")));
        }
        if status == "disabled" {
            // Disabling is a stronger action than a plain profile update.
            require_permission(&p, "user:disable")?;
        }
        st.users.set_status(user_id, status).await?;
    }

    let updated = st
        .users
        .find_by_id(user_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let roles = st
        .roles
        .roles_of("user", id)
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect();
    let groups = st.groups.groups_of_user(user_id).await?;
    Ok(Json(UserDto {
        id: updated.id.0,
        email: updated.email,
        name: updated.name,
        provider: updated.provider,
        status: updated.status,
        roles,
        groups,
    }))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/users/{id}/roles",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = AssignRoleInput,
    responses(
        (status = 204, description = "Role assigned to the user"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such user or role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn assign_user_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignRoleInput>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.roles.assign("user", id, body.role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/users/{id}/roles/{role_id}",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "User id"),
        ("role_id" = Uuid, Path, description = "Role id to unassign"),
    ),
    responses(
        (status = 204, description = "Role unassigned from the user"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such role", body = ErrorBody),
        (status = 409, description = "Would remove the last platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn unassign_user_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    guard_last_admin(&st, role_id).await?;
    st.roles.unassign("user", id, role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- Groups ----------

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: Uuid,
    pub name: String,
    pub source: String,
    pub member_count: i64,
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct CreateGroupInput {
    name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct RenameGroupInput {
    name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct AddMemberInput {
    user_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/groups",
    tag = "control-plane-access",
    responses(
        (status = 200, description = "Platform groups with member counts and roles", body = Vec<GroupDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing group:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_groups(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<GroupDto>>, CpError> {
    require_permission(&p, "group:read")?;
    let groups = st.groups.list_with_meta().await?;
    let mut out = Vec::with_capacity(groups.len());
    for (g, member_count) in groups {
        let roles = st
            .roles
            .roles_of("group", g.id)
            .await?
            .into_iter()
            .map(|r| r.name)
            .collect();
        out.push(GroupDto {
            id: g.id,
            name: g.name,
            source: g.source,
            member_count,
            roles,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/groups",
    tag = "control-plane-access",
    request_body = CreateGroupInput,
    responses(
        (status = 201, description = "Local group created", body = GroupDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing group:create permission", body = ErrorBody),
        (status = 409, description = "A group with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateGroupInput>,
) -> Result<(StatusCode, Json<GroupDto>), CpError> {
    require_permission(&p, "group:create")?;
    let id = st.groups.create_local(&body.name).await?;
    Ok((
        StatusCode::CREATED,
        Json(GroupDto {
            id,
            name: body.name,
            source: "local".to_string(),
            member_count: 0,
            roles: vec![],
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/control-plane/groups/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Group id")),
    request_body = RenameGroupInput,
    responses(
        (status = 204, description = "Group renamed"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing group:update permission", body = ErrorBody),
        (status = 409, description = "Cannot rename an OIDC-synced group", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rename_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameGroupInput>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "group:update")?;
    let rows = st.groups.rename(id, &body.name).await?;
    if rows == 0 {
        return Err(CpError::Conflict(
            "Cannot rename an OIDC-synced group".to_string(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/groups/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Group id")),
    responses(
        (status = 204, description = "Group deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing group:delete permission", body = ErrorBody),
        (status = 409, description = "Cannot delete an OIDC-synced group", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "group:delete")?;
    let rows = st.groups.delete_local(id).await?;
    if rows == 0 {
        return Err(CpError::Conflict(
            "Cannot delete an OIDC-synced group".to_string(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GroupMemberDto {
    pub id: Uuid,
    pub email: String,
    pub name: String,
}

/// `GET /groups/{id}/members` — the group's current members, so the admin
/// console can prefill and diff its manage-members dialog. Read-only, so it
/// gates on `group:read` rather than `group:manage_members`.
#[utoipa::path(
    get,
    path = "/api/control-plane/groups/{id}/members",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Group id")),
    responses(
        (status = 200, description = "The group's current members", body = Vec<GroupMemberDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing group:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_group_members(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<GroupMemberDto>>, CpError> {
    require_permission(&p, "group:read")?;
    let members = st.groups.member_users(id).await?;
    Ok(Json(
        members
            .into_iter()
            .map(|(id, email, name)| GroupMemberDto { id, email, name })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/groups/{id}/members",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Group id")),
    request_body = AddMemberInput,
    responses(
        (status = 204, description = "User added to the group"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such group or user", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_group_member(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberInput>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.groups.add_member(id, UserId(body.user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/groups/{id}/members/{user_id}",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Group id"),
        ("user_id" = Uuid, Path, description = "User id to remove"),
    ),
    responses(
        (status = 204, description = "User removed from the group"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove_group_member(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.groups.remove_member(id, UserId(user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/control-plane/groups/{id}/roles",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Group id")),
    request_body = AssignRoleInput,
    responses(
        (status = 204, description = "Role assigned to the group"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such group or role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn assign_group_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignRoleInput>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.roles.assign("group", id, body.role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/groups/{id}/roles/{role_id}",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Group id"),
        ("role_id" = Uuid, Path, description = "Role id to unassign"),
    ),
    responses(
        (status = 204, description = "Role unassigned from the group"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such role", body = ErrorBody),
        (status = 409, description = "Would remove the last platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn unassign_group_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    guard_last_admin(&st, role_id).await?;
    st.roles.unassign("group", id, role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- Roles ----------

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    pub description: String,
    pub permission_count: i64,
    pub assignee_count: i64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct CreateRoleInput {
    scope: String,
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct UpdateRoleInput {
    description: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct SetPermissionsInput {
    permissions: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/access/roles",
    tag = "control-plane-access",
    responses(
        (status = 200, description = "RBAC roles with permission and assignee counts", body = Vec<RoleDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_roles(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<RoleDto>>, CpError> {
    require_permission(&p, "role:read")?;
    let roles = st.roles.list().await?;
    let mut out = Vec::with_capacity(roles.len());
    for r in roles {
        let permission_count = st.roles.permissions_of(r.id).await?.len() as i64;
        let assignee_count = st.roles.assignees_of(r.id).await?.len() as i64;
        out.push(RoleDto {
            id: r.id,
            scope: r.scope,
            name: r.name,
            description: r.description,
            permission_count,
            assignee_count,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/access/roles",
    tag = "control-plane-access",
    request_body = CreateRoleInput,
    responses(
        (status = 201, description = "Role created", body = RoleDto),
        (status = 400, description = "Invalid scope", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 409, description = "A role with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateRoleInput>,
) -> Result<(StatusCode, Json<RoleDto>), CpError> {
    require_platform_admin(&p)?;
    if body.scope != "platform" && body.scope != "workspace" {
        return Err(CpError::BadRequest(format!(
            "invalid scope: {}",
            body.scope
        )));
    }
    let id = st
        .roles
        .create(&body.scope, &body.name, &body.description)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(RoleDto {
            id,
            scope: body.scope,
            name: body.name,
            description: body.description,
            permission_count: 0,
            assignee_count: 0,
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/control-plane/access/roles/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Role id")),
    request_body = UpdateRoleInput,
    responses(
        (status = 200, description = "Role description updated", body = RoleDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRoleInput>,
) -> Result<Json<RoleDto>, CpError> {
    require_platform_admin(&p)?;
    st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    st.roles.update_description(id, &body.description).await?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    let permission_count = st.roles.permissions_of(id).await?.len() as i64;
    let assignee_count = st.roles.assignees_of(id).await?.len() as i64;
    Ok(Json(RoleDto {
        id: role.id,
        scope: role.scope,
        name: role.name,
        description: role.description,
        permission_count,
        assignee_count,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/access/roles/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Role id")),
    responses(
        (status = 204, description = "Role deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 404, description = "No such role", body = ErrorBody),
        (status = 409, description = "Seeded roles cannot be deleted", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    if is_seeded(&role.name) {
        return Err(CpError::Conflict(
            "Seeded roles cannot be deleted".to_string(),
        ));
    }
    st.roles.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/control-plane/access/roles/{id}/permissions",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Role id")),
    request_body = SetPermissionsInput,
    responses(
        (status = 204, description = "Role permission set replaced"),
        (status = 400, description = "Unknown permission key", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Requires platform_admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_role_permissions(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetPermissionsInput>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    for key in &body.permissions {
        if !rbac::is_known(key) {
            return Err(CpError::BadRequest(format!("unknown permission: {key}")));
        }
    }
    st.roles.set_permissions(id, &body.permissions).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RoleAssigneeDto {
    pub kind: String,
    pub name: String,
}

/// `GET /access/roles/{id}/assignees` — the role's current assignees (users
/// by email, groups by name), so the admin console can drill into who holds
/// a role. Read-only, so it gates on `role:read`.
#[utoipa::path(
    get,
    path = "/api/control-plane/access/roles/{id}/assignees",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Role id")),
    responses(
        (status = 200, description = "The role's current assignees (users and groups)", body = Vec<RoleAssigneeDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_role_assignees(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleAssigneeDto>>, CpError> {
    require_permission(&p, "role:read")?;
    let assignees = st.roles.assignees_named(id).await?;
    Ok(Json(
        assignees
            .into_iter()
            .map(|(kind, name)| RoleAssigneeDto { kind, name })
            .collect(),
    ))
}

// ---------- Permission catalog ----------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PermissionDto {
    #[schema(value_type = String)]
    pub key: &'static str,
    #[schema(value_type = String)]
    pub description: &'static str,
    pub roles: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PermissionGroupDto {
    #[schema(value_type = String)]
    pub group: &'static str,
    pub permissions: Vec<PermissionDto>,
}

/// `GET /permissions` — the code-defined catalog (`egret_core::rbac`), each
/// permission annotated with the role names that currently grant it.
#[utoipa::path(
    get,
    path = "/api/control-plane/permissions",
    tag = "control-plane-access",
    responses(
        (status = 200, description = "The code-defined permission catalog, grouped, annotated with granting roles", body = Vec<PermissionGroupDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_permissions(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<PermissionGroupDto>>, CpError> {
    require_permission(&p, "role:read")?;
    let pairs = st.roles.permission_role_pairs().await?;
    let mut roles_by_perm: HashMap<String, Vec<String>> = HashMap::new();
    for (permission, role_name) in pairs {
        roles_by_perm.entry(permission).or_default().push(role_name);
    }

    let out = rbac::groups()
        .into_iter()
        .map(|group| {
            let permissions = rbac::PERMISSIONS
                .iter()
                .filter(|perm| perm.group == group)
                .map(|perm| {
                    let mut roles = roles_by_perm.get(perm.key).cloned().unwrap_or_default();
                    roles.sort();
                    PermissionDto {
                        key: perm.key,
                        description: perm.description,
                        roles,
                    }
                })
                .collect();
            PermissionGroupDto { group, permissions }
        })
        .collect();
    Ok(Json(out))
}

/// OpenAPI fragment for the control-plane Access router (users/groups/roles/
/// permissions). Merged into `egret_controlplane::openapi()`. `UserDto` is
/// emitted as `ControlPlaneUserDto` (see its `#[schema(as = …)]`) to avoid
/// clobbering egret-workspace's differently-shaped `UserDto`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_users,
        create_user,
        update_user,
        assign_user_role,
        unassign_user_role,
        list_groups,
        create_group,
        rename_group,
        delete_group,
        list_group_members,
        add_group_member,
        remove_group_member,
        assign_group_role,
        unassign_group_role,
        list_roles,
        create_role,
        update_role,
        delete_role,
        set_role_permissions,
        list_role_assignees,
        list_permissions,
    ),
    components(schemas(
        UserDto,
        CreateUserInput,
        UpdateUserInput,
        AssignRoleInput,
        GroupDto,
        CreateGroupInput,
        RenameGroupInput,
        AddMemberInput,
        GroupMemberDto,
        RoleDto,
        CreateRoleInput,
        UpdateRoleInput,
        SetPermissionsInput,
        RoleAssigneeDto,
        PermissionDto,
        PermissionGroupDto,
        ErrorBody,
    )),
    tags((name = "control-plane-access", description = "Platform access control: users, groups, roles, and the permission catalog")),
)]
pub struct ControlPlaneAccessApi;
