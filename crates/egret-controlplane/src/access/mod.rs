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
        .route("/users/{id}", patch(update_user).delete(delete_user))
        .route("/users/{id}/password", post(reset_password))
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
        .route(
            "/access/roles/{id}",
            get(get_role).patch(update_role).delete(delete_role),
        )
        .route("/access/roles/{id}/permissions", put(set_role_permissions))
        .route("/access/roles/{id}/assignees", get(list_role_assignees))
        .route("/permissions", get(list_permissions))
        .route("/access/effective", get(get_effective_access))
        .route("/access/who-can-access", get(who_can_access))
}

/// Reject removing a role assignment that would leave no active platform
/// admin — shared by the user- and group-role unassign endpoints.
///
/// "Active platform admin" is the engine-admin definition
/// (`egret_auth::roles::engine_admin_user_ids_via`, username-takeover spec
/// 2026-10-23 §10): a disabled admin cannot sign in, so it does not keep the
/// platform managed. Only a demotion that takes the set from non-empty to empty
/// is refused.
async fn guard_last_admin(
    st: &CpState,
    principal_type: &str,
    principal_id: Uuid,
    role_id: Uuid,
) -> Result<(), CpError> {
    let role = st.roles.get(role_id).await?.ok_or(CpError::NotFound)?;
    if role.name != PLATFORM_ADMIN_ROLE {
        return Ok(());
    }
    let mut conn = st
        .roles
        .pool()
        .acquire()
        .await
        .map_err(|e| egret_core::Error::Internal(e.into()))?;
    let now = egret_auth::roles::engine_admin_user_ids_via(&mut conn).await?;
    let after = egret_auth::roles::engine_admin_user_ids_without_grant_via(
        &mut conn,
        principal_type,
        principal_id,
    )
    .await?;
    if !now.is_empty() && after.is_empty() {
        return Err(CpError::Conflict(
            "Cannot remove the last platform admin".to_string(),
        ));
    }
    Ok(())
}

/// Reject an action that would remove the last active platform admin, or that
/// a caller aims at their own account. Reused by disable + delete (Task 4).
///
/// "Active" is the engine-admin definition
/// (`egret_auth::roles::engine_admin_user_ids_via`): a disabled admin does not
/// count, so the last ACTIVE admin cannot be disabled or deleted while a
/// disabled one remains (username-takeover spec 2026-10-23 §10).
///
/// A self-target is a `BadRequest` (400) — a bad actor/target, not a state
/// conflict — while removing the last admin is a `Conflict` (409), matching the
/// sibling `guard_last_admin`. Like `guard_last_admin`, this check-then-act is
/// TOCTOU-racy — a known, accepted limitation: two concurrent disables aimed at
/// different admins in a 2-admin system could both read `len == 2`, pass, and
/// leave zero admins. The window is tiny and admin-only; a hard invariant would
/// need a DB constraint or a serialized transaction.
async fn guard_not_last_admin(
    st: &CpState,
    caller: &egret_core::Principal,
    target: UserId,
) -> Result<(), CpError> {
    if caller.user_id == target {
        return Err(CpError::BadRequest(
            "you cannot disable or delete your own account".into(),
        ));
    }
    let mut conn = st
        .roles
        .pool()
        .acquire()
        .await
        .map_err(|e| egret_core::Error::Internal(e.into()))?;
    let admins = egret_auth::roles::engine_admin_user_ids_via(&mut conn).await?;
    if admins.contains(&target) && admins.iter().all(|a| *a == target) {
        return Err(CpError::Conflict(
            "Cannot remove the last platform admin".into(),
        ));
    }
    Ok(())
}

/// A service account is a `users` row (Data API P2) with no password and no
/// editable human profile, so the profile-shaped operations on these routes
/// refuse it rather than half-working.
///
/// Scoped to the PROFILE deliberately. `status` is NOT a profile field: spec
/// §6.1 names `users.status <> 'active'` as the kill switch for a compromised
/// service account, and `ClientStore::verify` honours it, but the only API that
/// can set it is the `status` branch of [`update_user`]. Refusing a service
/// account for the whole of `PATCH /users/{id}` therefore deleted the documented
/// kill switch — and with it the `user:disable` re-check, `guard_not_last_admin`
/// and session revocation that branch already performs. So the refusal covers
/// name/email and lets `status` through.
async fn refuse_service_account_profile(st: &CpState, id: UserId) -> Result<(), CpError> {
    let u = st.users.find_by_id(id).await?.ok_or(CpError::NotFound)?;
    if u.provider == "service" {
        return Err(CpError::BadRequest(
            "this is a service account; manage it through the Data API service-account routes"
                .into(),
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
    /// The engine username, `null` until one is minted. Read-only: a delete
    /// retires it for good (username-takeover spec 2026-10-23 §4.4), so the
    /// delete dialog names it.
    pub username: Option<String>,
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
    egret_core::authz::require(&*st.pdp, &p, "user:read").await?;
    let users = st.users.list(LIST_LIMIT, 0).await?;
    let ids: Vec<Uuid> = users.iter().map(|u| u.id.0).collect();
    let mut usernames = st.users.usernames(&ids).await?;
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
            username: usernames.remove(&u.id.0),
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
    egret_core::authz::require(&*st.pdp, &p, "user:create").await?;
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
            username: None,
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
        (status = 200, description = "User profile and/or status updated (disabling also revokes the user's live sessions immediately)", body = UserDto),
        (status = 400, description = "Invalid status value, or a caller disabling their own account", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update (or user:disable) permission", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
        (status = 409, description = "Would remove the last platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_user(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserInput>,
) -> Result<Json<UserDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    let user_id = UserId(id);
    let existing = st
        .users
        .find_by_id(user_id)
        .await?
        .ok_or(CpError::NotFound)?;

    if let Some(name) = &body.name {
        // Only the profile edit is refused for a service account — see
        // `refuse_service_account_profile`. `status` below stays reachable
        // because it is the kill switch `ClientStore::verify` reads.
        refuse_service_account_profile(&st, user_id).await?;
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
            egret_core::authz::require(&*st.pdp, &p, "user:disable").await?;
            guard_not_last_admin(&st, &p, user_id).await?;
            // Username takeover (spec 2026-10-23 §4.2): a disabled admin
            // leaves every `egret-admin` set in the same commit.
            egret_perm_compile::set_user_status_and_recompile(st.roles.pool(), id, status)
                .await
                .map_err(CpError::from)?;
            // Immediate lockout: kill the user's live sessions now, rather than
            // waiting for `SessionService::validate` to reject them on their
            // next request.
            st.sessions.revoke_all_for_user(user_id).await?;
        } else {
            egret_perm_compile::set_user_status_and_recompile(st.roles.pool(), id, status)
                .await
                .map_err(CpError::from)?;
        }
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
    let username = st.users.usernames(&[id]).await?.remove(&id);
    Ok(Json(UserDto {
        id: updated.id.0,
        email: updated.email,
        name: updated.name,
        provider: updated.provider,
        status: updated.status,
        roles,
        groups,
        username,
    }))
}

/// `DELETE /users/{id}` — hard-delete a user (Task 4). Guards: not yourself,
/// not the last active platform admin (`guard_not_last_admin`, shared with the
/// disable branch of `update_user`). Cleans up the user's sessions, group
/// memberships, and platform role/permission grants, then removes the row
/// itself, in one transaction (`UserRepo::hard_delete`).
#[utoipa::path(
    delete,
    path = "/api/control-plane/users/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "User id")),
    responses(
        (status = 204, description = "User hard-deleted along with sessions, group memberships and grants. Their engine username is retired permanently: no user can be given it again, so refs and policies naming it match nobody. To revoke access but keep the username, set `status` to `disabled` instead."),
        (status = 400, description = "Caller attempted to delete their own account", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:delete permission", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
        (status = 409, description = "Would remove the last platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_user(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:delete").await?;
    let user_id = UserId(id);
    st.users
        .find_by_id(user_id)
        .await?
        .ok_or(CpError::NotFound)?;
    guard_not_last_admin(&st, &p, user_id).await?;
    // J-F3 Task 1b: deleting a user removes them from every group (FK cascade),
    // and a group compiles to its members — so the policies their groups reach
    // are recompiled in the delete's own transaction
    // (`egret_auth::groups`' module doc).
    st.users
        .hard_delete(user_id, &p, &egret_perm_compile::PermGroupRecompiler)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct ResetPasswordInput {
    password: String,
}

/// `POST /users/{id}/password` — admin reset of a LOCAL user's password.
/// Rehashes the new plaintext, then revokes the user's live sessions (mirrors
/// the disable branch of `update_user`) so a session issued under the old
/// password can't linger.
#[utoipa::path(
    post,
    path = "/api/control-plane/users/{id}/password",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = ResetPasswordInput,
    responses(
        (status = 204, description = "Password reset; the user's live sessions are revoked"),
        (status = 400, description = "Empty password, or the target is not a local account", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn reset_password(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetPasswordInput>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    refuse_service_account_profile(&st, UserId(id)).await?;
    let user_id = UserId(id);
    let user = st
        .users
        .find_by_id(user_id)
        .await?
        .ok_or(CpError::NotFound)?;
    if user.provider != "local" {
        return Err(CpError::BadRequest(
            "cannot set a password on a non-local (SSO) account".into(),
        ));
    }
    if body.password.is_empty() {
        return Err(CpError::BadRequest("password must not be empty".into()));
    }
    let hash = egret_auth::password::hash_password(&body.password)?;
    st.users.set_password(user_id, &hash).await?;
    st.sessions.revoke_all_for_user(user_id).await?;
    Ok(StatusCode::NO_CONTENT)
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
    egret_core::authz::require(&*st.pdp, &p, "user:assign_role").await?;
    // **SP4c Task 3, input 3: who holds the clearing role.** The grant and the
    // recompile it forces share one transaction. A role NAME is unresolvable in
    // every engine egret provisions (D0/D0b), so egret enumerates a role's
    // HOLDERS at compile time — into a clearance's `denyExceptions` and into a
    // `role` grant's users — and that enumeration is stale the moment this row is
    // written. See `egret_perm_compile::role_membership` for why the recompile
    // cannot live inside `egret_auth::roles` (it would be a dependency cycle).
    egret_perm_compile::assign_platform_role_and_recompile(
        st.roles.pool(),
        "user",
        id,
        body.role_id,
    )
    .await
    .map_err(CpError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "role:assign",
        "grant",
        Some(body.role_id.to_string()),
        serde_json::json!({
            "principal_type": "user",
            "principal_id": id,
            "role_id": body.role_id,
        }),
    ));
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
    egret_core::authz::require(&*st.pdp, &p, "user:assign_role").await?;
    guard_last_admin(&st, "user", id, role_id).await?;
    // SP4c Task 3: the REVOKE direction, which is the one that fails OPEN when
    // its recompile is missed — the revoked holder stays named in every clearance
    // exception and every role-granted policy, still reading a classified column
    // whose clearance was taken away, with the `access_grants` row egret's own UI
    // reads showing the revoke as done.
    egret_perm_compile::unassign_platform_role_and_recompile(st.roles.pool(), "user", id, role_id)
        .await
        .map_err(CpError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "role:unassign",
        "grant",
        Some(role_id.to_string()),
        serde_json::json!({
            "principal_type": "user",
            "principal_id": id,
            "role_id": role_id,
        }),
    ));
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
    egret_core::authz::require(&*st.pdp, &p, "group:read").await?;
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
    egret_core::authz::require(&*st.pdp, &p, "group:create").await?;
    let id = st
        .groups
        .create_local(&body.name, &egret_perm_compile::PermGroupRecompiler)
        .await?;
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
        (status = 409, description = "Cannot rename an OIDC-synced group; or another group already has the new name; or grants or rule exemptions still name the old or the new name (the message lists them to a caller with policy:read, and otherwise gives only their count; remove them first)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rename_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameGroupInput>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "group:update").await?;
    // Principals name a group by NAME, so the repo refuses (409, listing
    // them) while anything names the old or the new name (2026-10-18 R1).
    let rows = st
        .groups
        .rename(
            id,
            &body.name,
            &egret_perm_compile::PermGroupRecompiler,
            reference_detail(&st, &p).await?,
        )
        .await?;
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
        (status = 409, description = "Cannot delete an OIDC-synced group; or grants or rule exemptions still name the group (the message lists them to a caller with policy:read, and otherwise gives only their count; remove them first)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "group:delete").await?;
    // The delete cascades every membership away — recompiled. Refused (409,
    // listing them) while grants or rule exemptions name the group (R1).
    let rows = st
        .groups
        .delete_local(
            id,
            &egret_perm_compile::PermGroupRecompiler,
            reference_detail(&st, &p).await?,
        )
        .await?;
    if rows == 0 {
        return Err(CpError::Conflict(
            "Cannot delete an OIDC-synced group".to_string(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// How much a group delete or rename refused because grants or rule
/// exemptions name the group says about them (default decision 2026-10-18,
/// review M-2). The entries are policy metadata, and `group:delete` /
/// `group:update` can be held without `policy:read`: such a caller gets the
/// count and who to ask, not the grant ids and rule names.
async fn reference_detail(
    st: &CpState,
    p: &egret_core::Principal,
) -> Result<egret_auth::groups::ReferenceDetail, CpError> {
    use egret_auth::groups::ReferenceDetail;
    match egret_core::authz::require(&*st.pdp, p, "policy:read").await {
        Ok(()) => Ok(ReferenceDetail::Entries),
        Err(egret_core::Error::Forbidden) => Ok(ReferenceDetail::CountOnly),
        Err(e) => Err(e.into()),
    }
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
    egret_core::authz::require(&*st.pdp, &p, "group:read").await?;
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
        (status = 409, description = "Cannot add members to an OIDC-synced group", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_group_member(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberInput>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "group:manage_members").await?;
    // J-F3 Task 1b: a group compiles to its members' usernames, so the write
    // and the recompile of every policy the group reaches share one transaction
    // (`egret_perm_compile::group_membership`). Design R4: an OIDC-synced
    // group answers 409 — the repo refuses it, since its membership is the
    // identity provider's.
    st.groups
        .add_member(
            id,
            UserId(body.user_id),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await?;
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
    egret_core::authz::require(&*st.pdp, &p, "group:manage_members").await?;
    // J-F3 Task 1b: the REMOVE direction, the one that fails OPEN when its
    // recompile is missed — the ex-member stays named in every policy the
    // group reaches.
    st.groups
        .remove_member(
            id,
            UserId(user_id),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await?;
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
    egret_core::authz::require(&*st.pdp, &p, "user:assign_role").await?;
    // Username takeover (spec 2026-10-23 §4.2, E10): a group's role grant
    // changes who holds the role exactly as a user's does — for
    // `platform_admin`, every member's engine ALL access — so it takes the
    // same recompiling path.
    egret_perm_compile::assign_platform_role_and_recompile(
        st.roles.pool(),
        "group",
        id,
        body.role_id,
    )
    .await
    .map_err(CpError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "role:assign",
        "grant",
        Some(body.role_id.to_string()),
        serde_json::json!({
            "principal_type": "group",
            "principal_id": id,
            "role_id": body.role_id,
        }),
    ));
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
    egret_core::authz::require(&*st.pdp, &p, "user:assign_role").await?;
    guard_last_admin(&st, "group", id, role_id).await?;
    egret_perm_compile::unassign_platform_role_and_recompile(st.roles.pool(), "group", id, role_id)
        .await
        .map_err(CpError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "role:unassign",
        "grant",
        Some(role_id.to_string()),
        serde_json::json!({
            "principal_type": "group",
            "principal_id": id,
            "role_id": role_id,
        }),
    ));
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
    /// The role this one inherits permissions from (same scope). `None` at the
    /// top of a chain.
    pub parent_role_id: Option<Uuid>,
    /// Effective-minus-own permissions, for read-only display. Populated on the
    /// single-role GET; empty on the list endpoint.
    pub inherited_permissions: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateRoleInput {
    scope: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parent_role_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateRoleInput {
    description: String,
    #[serde(default)]
    parent_role_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct SetPermissionsInput {
    permissions: Vec<String>,
}

/// Effective-minus-own permissions for a role: the union of every ancestor's
/// own permissions (walking `parent_role_id`), minus what the role grants
/// itself. The chain is acyclic (enforced in `RoleRepo::set_parent` + a DB
/// trigger), but cap defensively so a corrupt graph can't loop forever.
async fn inherited_perms(
    st: &CpState,
    role: &egret_auth::roles::RoleRow,
) -> Result<Vec<String>, CpError> {
    let own: std::collections::BTreeSet<String> = st
        .roles
        .permissions_of(role.id)
        .await?
        .into_iter()
        .collect();
    let mut acc: std::collections::BTreeSet<String> = Default::default();
    let mut cur = role.parent_role_id;
    let mut hops = 0;
    while let Some(c) = cur {
        for perm in st.roles.permissions_of(c).await? {
            acc.insert(perm);
        }
        hops += 1;
        if hops > 64 {
            break;
        }
        cur = st.roles.get(c).await?.and_then(|r| r.parent_role_id);
    }
    Ok(acc.difference(&own).cloned().collect())
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
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
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
            parent_role_id: r.parent_role_id,
            // The list view stays cheap: inherited perms are only computed on
            // the single-role GET.
            inherited_permissions: vec![],
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/access/roles/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Role id")),
    responses(
        (status = 200, description = "A single role with its parent and inherited permissions", body = RoleDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 404, description = "No such role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<RoleDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    let permission_count = st.roles.permissions_of(id).await?.len() as i64;
    let assignee_count = st.roles.assignees_of(id).await?.len() as i64;
    let inherited_permissions = inherited_perms(&st, &role).await?;
    Ok(Json(RoleDto {
        id: role.id,
        scope: role.scope,
        name: role.name,
        description: role.description,
        permission_count,
        assignee_count,
        parent_role_id: role.parent_role_id,
        inherited_permissions,
    }))
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
    egret_core::authz::require(&*st.pdp, &p, "role:create").await?;
    if body.scope != "platform" && body.scope != "workspace" {
        return Err(CpError::BadRequest(format!(
            "invalid scope: {}",
            body.scope
        )));
    }
    let id = st
        .roles
        .create(
            &body.scope,
            &body.name,
            &body.description,
            body.parent_role_id,
        )
        .await
        .map_err(|e| match e {
            // A bad parent (cross-scope or cycle) trips the `roles_parent_guard`
            // DB trigger, which surfaces as an internal DB error. When the caller
            // supplied a parent, report it as a client error rather than a 500.
            egret_core::Error::Internal(_) if body.parent_role_id.is_some() => CpError::BadRequest(
                "invalid parent role (must share scope and be acyclic)".to_string(),
            ),
            other => other.into(),
        })?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    let inherited_permissions = inherited_perms(&st, &role).await?;
    Ok((
        StatusCode::CREATED,
        Json(RoleDto {
            id,
            scope: body.scope,
            name: body.name,
            description: body.description,
            permission_count: 0,
            assignee_count: 0,
            parent_role_id: role.parent_role_id,
            inherited_permissions,
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
    egret_core::authz::require(&*st.pdp, &p, "role:update").await?;
    st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    st.roles.update_description(id, &body.description).await?;
    // `set_parent` validates same-scope + acyclic in the app layer, returning
    // `InvalidInput` (→ `CpError::BadRequest`) for a bad parent.
    st.roles.set_parent(id, body.parent_role_id).await?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    let permission_count = st.roles.permissions_of(id).await?.len() as i64;
    let assignee_count = st.roles.assignees_of(id).await?.len() as i64;
    let inherited_permissions = inherited_perms(&st, &role).await?;
    Ok(Json(RoleDto {
        id: role.id,
        scope: role.scope,
        name: role.name,
        description: role.description,
        permission_count,
        assignee_count,
        parent_role_id: role.parent_role_id,
        inherited_permissions,
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
    egret_core::authz::require(&*st.pdp, &p, "role:delete").await?;
    let role = st.roles.get(id).await?.ok_or(CpError::NotFound)?;
    if is_seeded(&role.name) {
        return Err(CpError::Conflict(
            "Seeded roles cannot be deleted".to_string(),
        ));
    }
    // **A role delete IS a role-membership write.** `access_grants.role_id` is
    // `ON DELETE CASCADE` (migration 0068), so this revokes the role from every
    // holder at once — and if the role was clearing a classification, the
    // compiled `denyExceptions` would go on naming the ex-holders until
    // something recompiled, letting them read the classified column after the
    // clearance was taken away. Same three calls as assign/unassign, in the same
    // transaction; see `egret_perm_compile::role_membership`.
    egret_perm_compile::delete_platform_role_and_recompile(st.roles.pool(), id)
        .await
        .map_err(CpError::from)?;
    // The repo publishes this itself on the non-transactional path; here the
    // transaction was owned above, so it is published after the commit.
    st.roles.publish_roles_changed();
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
    egret_core::authz::require(&*st.pdp, &p, "role:assign_permission").await?;
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
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
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
    /// Enforcement scope: `"platform"` or `"workspace"`.
    #[schema(value_type = String)]
    pub scope: rbac::Scope,
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
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
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
                        scope: perm.scope,
                        roles,
                    }
                })
                .collect();
            PermissionGroupDto { group, permissions }
        })
        .collect();
    Ok(Json(out))
}

// ---------- Effective access (AX-3) ----------

#[derive(Debug, Deserialize)]
struct EffectiveQuery {
    principal_type: String,
    principal_id: String,
}

/// `GET /access/effective` — the effective-access resolver (AX-3): for a
/// principal, enumerate the platform roles, permissions (with provenance),
/// workspace memberships, object grants, and denies that feed them. Read-only,
/// so it gates on `role:read`. `principal_id` accepts a raw UUID or a
/// user email / group name (resolved to a UUID; 404 if unknown).
#[utoipa::path(
    get,
    path = "/api/control-plane/access/effective",
    tag = "control-plane-access",
    params(
        ("principal_type" = String, Query, description = "user | group"),
        ("principal_id" = String, Query, description = "UUID, or a user email / group name"),
    ),
    responses(
        (status = 200, description = "Effective access for the principal", body = egret_auth::effective::EffectiveAccess),
        (status = 400, description = "Invalid principal_type", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 404, description = "No such principal", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_effective_access(
    State(st): State<CpState>,
    Auth(p): Auth,
    axum::extract::Query(q): axum::extract::Query<EffectiveQuery>,
) -> Result<Json<egret_auth::effective::EffectiveAccess>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
    let pid = st
        .access_resolver
        .resolve_ident(&q.principal_type, &q.principal_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let ea = st.access_resolver.resolve(&q.principal_type, pid).await?;
    Ok(Json(ea))
}

#[derive(Debug, Deserialize)]
struct WhoCanAccessQuery {
    kind: String,
    id: Uuid,
}

/// `GET /access/who-can-access` — the reverse of `/access/effective` (AX-3):
/// for one object (`kind` + `id`), enumerate every principal who can reach it
/// and why (direct object grants, the object's workspace-role holders, and
/// platform admins). Read-only, so it gates on `role:read`. An unknown `kind`
/// or a missing object surfaces `egret_core::Error::NotFound` → 404.
#[utoipa::path(
    get,
    path = "/api/control-plane/access/who-can-access",
    tag = "control-plane-access",
    params(
        ("kind" = String, Query, description = "Object kind: chart | dashboard | dataset | notebook | saved_query | folder | app"),
        ("id" = Uuid, Query, description = "Object UUID"),
    ),
    responses(
        (status = 200, description = "Principals who can access the object", body = egret_auth::effective::WhoCanAccess),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing role:read permission", body = ErrorBody),
        (status = 404, description = "Unknown object kind or no such object", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn who_can_access(
    State(st): State<CpState>,
    Auth(p): Auth,
    axum::extract::Query(q): axum::extract::Query<WhoCanAccessQuery>,
) -> Result<Json<egret_auth::effective::WhoCanAccess>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
    let wca = st.access_resolver.who_can_access(&q.kind, q.id).await?;
    Ok(Json(wca))
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
        delete_user,
        reset_password,
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
        get_role,
        create_role,
        update_role,
        delete_role,
        set_role_permissions,
        list_role_assignees,
        list_permissions,
        get_effective_access,
        who_can_access,
    ),
    components(schemas(
        UserDto,
        CreateUserInput,
        UpdateUserInput,
        ResetPasswordInput,
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
        egret_auth::effective::EffectiveAccess,
        egret_auth::effective::PrincipalInfo,
        egret_auth::effective::RoleSource,
        egret_auth::effective::PermVia,
        egret_auth::effective::EffectivePermission,
        egret_auth::effective::WorkspaceMembership,
        egret_auth::effective::ObjectGrant,
        egret_auth::effective::DenyEntry,
        egret_auth::effective::WhoCanAccess,
        egret_auth::effective::ObjectRef,
        egret_auth::effective::AccessEntry,
        ErrorBody,
    )),
    tags((name = "control-plane-access", description = "Platform access control: users, groups, roles, and the permission catalog")),
)]
pub struct ControlPlaneAccessApi;
