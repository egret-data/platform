//! Control-plane Access router: Users, Groups, Roles, and the Permission
//! catalog — CRUD + role-assignment endpoints, nested under
//! `/api/control-plane`.
//!
//! Follows the module/router/handler idiom of `secrets.rs`: every handler
//! gates on `require_permission(&p, "resource:action")?` first
//! (`platform_admin` always bypasses). RBAC roles are mounted at
//! `/access/roles` (not `/roles`) to avoid colliding with the embedded-catalog
//! `roles`/`role_id` routes already served by `warehouses.rs`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_permission;
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
        .route("/groups/{id}/members", post(add_group_member))
        .route(
            "/groups/{id}/members/{user_id}",
            delete(remove_group_member),
        )
        .route("/groups/{id}/roles", post(assign_group_role))
        .route("/groups/{id}/roles/{role_id}", delete(unassign_group_role))
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize)]
struct CreateUserInput {
    email: String,
    name: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct UpdateUserInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignRoleInput {
    role_id: Uuid,
}

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

async fn assign_user_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignRoleInput>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "user:assign_role")?;
    st.roles.assign("user", id, body.role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unassign_user_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "user:assign_role")?;
    guard_last_admin(&st, role_id).await?;
    st.roles.unassign("user", id, role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- Groups ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: Uuid,
    pub name: String,
    pub source: String,
    pub member_count: i64,
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CreateGroupInput {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RenameGroupInput {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberInput {
    user_id: Uuid,
}

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

async fn add_group_member(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberInput>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "group:manage_members")?;
    st.groups.add_member(id, UserId(body.user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_group_member(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "group:manage_members")?;
    st.groups.remove_member(id, UserId(user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn assign_group_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignRoleInput>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "user:assign_role")?;
    st.roles.assign("group", id, body.role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unassign_group_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "user:assign_role")?;
    guard_last_admin(&st, role_id).await?;
    st.roles.unassign("group", id, role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
