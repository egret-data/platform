//! Control-plane `/service-accounts` surface (SA1): service accounts as
//! first-class principals — create/list/get, the disable/enable kill switch,
//! delete, and group/role membership.
//!
//! A service account is a `users` row (`provider='service'`), so every write
//! here reuses the paths the human routes already take: status changes go
//! through [`super::disable_user_guarded`] / `set_user_status_and_recompile`
//! (status + engine recompile + session revoke, exactly as `PATCH /users/{id}`),
//! delete through `UserRepo::hard_delete` (via `ServiceAccountRepo::delete`),
//! group membership through `GroupRepo::add_member`/`remove_member`, and role
//! grants through the recompiling platform-role helpers.
//!
//! Two guards keep a service account out of human-only trust paths: it cannot
//! join an OIDC-synced group (its membership is the IdP's, and a service
//! account has no IdP identity), and it cannot hold `platform_admin`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::service_accounts::{SaScope, ServiceAccount, ServiceAccountRepo};
use egret_core::http::ErrorBody;
use egret_core::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{disable_user_guarded, unassign_role_guarded};
use crate::error::CpError;
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new()
        .route(
            "/service-accounts",
            get(list_service_accounts).post(create_service_account),
        )
        .route(
            "/service-accounts/{id}",
            get(get_service_account).delete(delete_service_account),
        )
        .route(
            "/service-accounts/{id}/disable",
            post(disable_service_account),
        )
        .route(
            "/service-accounts/{id}/enable",
            post(enable_service_account),
        )
        .route(
            "/service-accounts/{id}/groups",
            post(add_service_account_group),
        )
        .route(
            "/service-accounts/{id}/groups/{group_id}",
            delete(remove_service_account_group),
        )
        .route(
            "/service-accounts/{id}/roles",
            post(add_service_account_role),
        )
        .route(
            "/service-accounts/{id}/roles/{role_id}",
            delete(remove_service_account_role),
        )
}

fn repo(st: &CpState) -> ServiceAccountRepo {
    ServiceAccountRepo::new(st.roles.pool().clone())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServiceAccountDto {
    pub user_id: Uuid,
    /// `platform` or `workspace`.
    pub scope: String,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Mirrors `users.status` (`active` / `disabled`).
    pub status: String,
}

impl From<ServiceAccount> for ServiceAccountDto {
    fn from(sa: ServiceAccount) -> Self {
        Self {
            user_id: sa.user_id,
            scope: match sa.scope {
                SaScope::Platform => "platform",
                SaScope::Workspace => "workspace",
            }
            .to_string(),
            workspace_id: sa.workspace_id,
            name: sa.name,
            description: sa.description,
            created_by: sa.created_by,
            created_at: sa.created_at,
            status: sa.status,
        }
    }
}

/// A service account with the groups and platform roles it holds.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServiceAccountDetailDto {
    #[serde(flatten)]
    pub account: ServiceAccountDto,
    /// Names of the groups the account is a member of.
    pub groups: Vec<String>,
    /// Names of the platform roles granted to the account directly.
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateServiceAccountBody {
    /// `platform` or `workspace`.
    #[schema(value_type = String)]
    pub scope: SaScope,
    /// Required for `workspace`, forbidden for `platform`.
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddGroupBody {
    pub group_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddRoleBody {
    pub role_id: Uuid,
    /// Only `platform` is supported on this surface.
    pub scope_type: String,
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    scope: Option<SaScope>,
    #[serde(default)]
    workspace_id: Option<Uuid>,
}

async fn require_sa(st: &CpState, id: Uuid) -> Result<ServiceAccount, CpError> {
    repo(st).get(id).await?.ok_or(CpError::NotFound)
}

async fn detail(st: &CpState, id: Uuid) -> Result<ServiceAccountDetailDto, CpError> {
    let account = require_sa(st, id).await?;
    let groups = st.groups.groups_of_user(UserId(id)).await?;
    let roles = st
        .roles
        .roles_of("user", id)
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect();
    Ok(ServiceAccountDetailDto {
        account: account.into(),
        groups,
        roles,
    })
}

#[utoipa::path(
    get,
    path = "/api/control-plane/service-accounts",
    tag = "control-plane-access",
    params(
        ("scope" = Option<String>, Query, description = "Filter by `platform` or `workspace`"),
        ("workspace_id" = Option<Uuid>, Query, description = "Filter by workspace"),
    ),
    responses(
        (status = 200, description = "Service accounts, newest first", body = Vec<ServiceAccountDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn list_service_accounts(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ServiceAccountDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:read").await?;
    let sas = repo(&st).list(q.scope, q.workspace_id).await?;
    Ok(Json(sas.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts",
    tag = "control-plane-access",
    request_body = CreateServiceAccountBody,
    responses(
        (status = 200, description = "Service account created", body = ServiceAccountDto),
        (status = 400, description = "Scope and workspace disagree, or the name is unusable", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:create permission", body = ErrorBody),
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 409, description = "A service account with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn create_service_account(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateServiceAccountBody>,
) -> Result<Json<ServiceAccountDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:create").await?;
    let sa = repo(&st)
        .create(
            body.scope,
            body.workspace_id,
            &body.name,
            body.description.as_deref().unwrap_or(""),
            p.user_id.0,
        )
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        sa.workspace_id,
        "service_account:create",
        "service_account",
        Some(sa.user_id.to_string()),
        serde_json::json!({ "name": sa.name, "scope": ServiceAccountDto::from(sa.clone()).scope }),
    ));
    Ok(Json(sa.into()))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/service-accounts/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    responses(
        (status = 200, description = "The service account with its group and role memberships", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:read permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn get_service_account(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:read").await?;
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/disable",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    responses(
        (status = 200, description = "Disabled: its credentials stop verifying, engine profiles are recompiled and live sessions revoked", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 409, description = "Would remove the last active platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn disable_service_account(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    // The same kill-switch path as the `status` branch of `PATCH /users/{id}`.
    disable_user_guarded(&st, UserId(id)).await?;
    st.sessions.revoke_all_for_user(UserId(id)).await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "service_account:disable",
        "service_account",
        Some(id.to_string()),
        serde_json::json!({}),
    ));
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/enable",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    responses(
        (status = 200, description = "Re-enabled", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn enable_service_account(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    egret_perm_compile::set_user_status_and_recompile(st.roles.pool(), id, "active")
        .await
        .map_err(CpError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "service_account:enable",
        "service_account",
        Some(id.to_string()),
        serde_json::json!({}),
    ));
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/service-accounts/{id}",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    responses(
        (status = 204, description = "Deleted along with its sessions, memberships and grants; its engine username is retired"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:delete permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 409, description = "The account still owns content", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn delete_service_account(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:delete").await?;
    let deleted = repo(&st)
        .delete(id, &p, &egret_perm_compile::PermGroupRecompiler)
        .await?;
    if !deleted {
        return Err(CpError::NotFound);
    }
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "service_account:delete",
        "service_account",
        Some(id.to_string()),
        serde_json::json!({}),
    ));
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/groups",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    request_body = AddGroupBody,
    responses(
        (status = 200, description = "Added to the group", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account or group", body = ErrorBody),
        (status = 409, description = "OIDC-synced groups are IdP-owned; a service account cannot join one", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn add_service_account_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddGroupBody>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    // `GroupRepo::add_member` refuses an OIDC-synced group with a 409 ("its
    // membership is owned by the identity provider"), before any write.
    st.groups
        .add_member(
            body.group_id,
            UserId(id),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await?;
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/service-accounts/{id}/groups/{group_id}",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Service account user id"),
        ("group_id" = Uuid, Path, description = "Group id"),
    ),
    responses(
        (status = 200, description = "Removed from the group", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 409, description = "Would remove the last active platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn remove_service_account_group(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, group_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    st.groups
        .remove_member(
            group_id,
            UserId(id),
            &egret_perm_compile::PermGroupRecompiler,
        )
        .await?;
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/roles",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    request_body = AddRoleBody,
    responses(
        (status = 200, description = "Role granted", body = ServiceAccountDetailDto),
        (status = 400, description = "platform_admin may not be granted to a service account, or the scope is unsupported", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account or role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn add_service_account_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddRoleBody>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    if body.scope_type != "platform" || body.workspace_id.is_some() {
        return Err(CpError::BadRequest(
            "only platform-scoped role grants are supported for a service account here".into(),
        ));
    }
    let role = st.roles.get(body.role_id).await?.ok_or(CpError::NotFound)?;
    if role.name == "platform_admin" {
        return Err(CpError::BadRequest(
            "platform_admin cannot be granted to a service account".into(),
        ));
    }
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
    Ok(Json(detail(&st, id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/service-accounts/{id}/roles/{role_id}",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Service account user id"),
        ("role_id" = Uuid, Path, description = "Role id to revoke"),
    ),
    responses(
        (status = 200, description = "Role revoked", body = ServiceAccountDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account or role", body = ErrorBody),
        (status = 409, description = "Would remove the last active platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn remove_service_account_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ServiceAccountDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    require_sa(&st, id).await?;
    unassign_role_guarded(&st, "user", id, role_id).await?;
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
    Ok(Json(detail(&st, id).await?))
}
