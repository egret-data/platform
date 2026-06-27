//! HTTP state + routers for workspace administration and membership.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::{Error, Pagination, UserId, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::context::WsContext;
use crate::members::MemberRepo;
use crate::provision::{SharedWarehouseProvisioner, StorageSpec};
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

#[derive(Clone)]
pub struct WorkspaceState {
    pub workspaces: WorkspaceRepo,
    pub members: MemberRepo,
    pub authz: Authorizer,
    pub users: UserRepo,
    pub sessions: SessionService,
    /// Optional hook that provisions the workspace's lakekeeper warehouse after
    /// the row is created (CE-2 T4). `None` when the embedded catalog engine is
    /// disabled — workspace creation then succeeds without a warehouse. Supplied
    /// by the composition root (`egret-bin`); see [`crate::provision`].
    pub warehouse_provisioner: SharedWarehouseProvisioner,
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

#[derive(Serialize)]
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

/// Per-workspace storage override accepted at workspace-create time (CE-5).
///
/// Deserialize-only input: `secret_key` is a write-only credential and this type
/// is intentionally NOT `Serialize`, so the secret can never be echoed back. The
/// handler maps it field-for-field to a [`StorageSpec`], which the provisioner
/// seals through the secret vault.
#[derive(Deserialize)]
pub struct StorageSpecDto {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default = "default_path_style")]
    pub path_style: bool,
}

fn default_provider() -> String {
    "s3".to_string()
}
fn default_path_style() -> bool {
    true
}

impl From<StorageSpecDto> for StorageSpec {
    fn from(d: StorageSpecDto) -> Self {
        StorageSpec {
            provider: d.provider,
            endpoint: d.endpoint,
            region: d.region,
            bucket: d.bucket,
            key_prefix: d.key_prefix,
            access_key: d.access_key,
            secret_key: d.secret_key,
            path_style: d.path_style,
        }
    }
}

#[derive(Deserialize)]
pub struct CreateWorkspace {
    pub slug: String,
    pub name: String,
    /// Optional per-workspace storage backend (CE-5). When omitted, the
    /// workspace provisions against the system-wide default storage config.
    #[serde(default)]
    pub storage: Option<StorageSpecDto>,
}
#[derive(Deserialize)]
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

async fn create_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Json(body): Json<CreateWorkspace>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    st.authz
        .require_platform(&p, "workspace:create")
        .await
        .map_err(AuthError)?;
    let ws = st
        .workspaces
        .create(&body.slug, &body.name, p.user_id)
        .await
        .map_err(AuthError)?;

    // Provision the workspace's lakekeeper warehouse (CE-2 T4). The warehouse is
    // what makes the workspace usable (queries/catalog browse target it), so a
    // provisioning failure is surfaced as a 500.
    //
    // Failure-handling choice (best-effort, documented): the workspace row is
    // already committed by the time we get here, so a provisioning failure leaves
    // a workspace WITHOUT a warehouse. We deliberately do NOT roll the row back —
    // `provision_workspace_warehouse` is idempotent, so the correct recovery is a
    // retried create (or an admin re-provision), which will find the existing
    // workspace's mapping absent and complete the warehouse. Rolling back here
    // would risk orphaning a warehouse that lakekeeper DID create before the
    // mapping write failed. We log loudly and return 500 so the caller knows the
    // workspace is not yet ready.
    let storage: Option<StorageSpec> = body.storage.map(Into::into);
    if let Some(provisioner) = &st.warehouse_provisioner {
        if let Err(e) = provisioner.provision(ws.id, &ws.slug, storage).await {
            tracing::error!(
                workspace_id = %ws.id.0,
                slug = %ws.slug,
                error = %e,
                "workspace created but warehouse provisioning failed; workspace is not yet usable \
                 (re-create/re-provision is idempotent and will complete it)"
            );
            return Err(AuthError(Error::Internal(anyhow::anyhow!(
                "workspace created but warehouse provisioning failed: {e}"
            ))));
        }
    }

    Ok(Json(ws.into()))
}

async fn list_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<WorkspaceDto>>, AuthError> {
    st.authz
        .require_platform(&p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let page = Pagination {
        page: q.page.unwrap_or(1),
        per_page: q.per_page.unwrap_or(50),
    };
    let list = st.workspaces.list(page).await.map_err(AuthError)?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

async fn get_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    st.authz
        .require_platform(&p, "workspace:read")
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

async fn update_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkspace>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz
        .require_platform(&p, "workspace:update")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .update_name(WorkspaceId(id), &body.name)
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn archive_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz
        .require_platform(&p, "workspace:archive")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .archive(WorkspaceId(id))
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Me routes (Task 1 / Phase 0b) ────────────────────────────────────────────

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

#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub name: String,
    pub provider: String,
    pub status: String,
}

#[derive(Deserialize)]
pub struct SetPlatformRoles {
    pub roles: Vec<String>,
}

pub fn admin_user_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}/platform-roles", put(set_platform_roles))
}

async fn list_users(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<UserDto>>, AuthError> {
    st.authz
        .require_platform(&p, "user:read")
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

async fn get_user(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, AuthError> {
    st.authz
        .require_platform(&p, "user:read")
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

async fn set_platform_roles(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetPlatformRoles>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz
        .require_platform(&p, "user:assign_role")
        .await
        .map_err(AuthError)?;
    st.authz
        .replace_platform_roles(UserId(id), &body.roles)
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Workspace member routes (Task 9) ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddMember {
    pub principal_type: String,
    pub principal_id: Uuid,
    pub role: String,
}

#[derive(Serialize)]
pub struct MemberDto {
    pub principal_type: String,
    pub principal_id: String,
    pub role: String,
}

pub fn ws_member_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/{workspace}/members", get(list_members).post(add_member))
        .route("/{workspace}/members/{ptype}/{pid}", delete(remove_member))
        .route("/{workspace}/storage", get(get_storage))
}

/// `GET /{workspace}/storage` — return the workspace's per-workspace storage
/// config WITHOUT the secret (CE-5). The response body is a
/// [`WorkspaceStorageInfo`], which carries only `has_secret`, never the secret
/// value. `WsContext` resolves the `{workspace}` slug and enforces membership.
///
/// **Admin-only**: storage config is administrative metadata (which bucket /
/// endpoint / access-key id backs the workspace), not something a viewer needs —
/// viewers query the catalog via tokens, never the raw S3 config. So this is
/// gated by `workspace:manage_members` (granted to `ws_admin` by migration 0003,
/// the same admin-level perm the member endpoints use), NOT the broad
/// `catalog:read`. Platform admins bypass via `is_platform_admin()`.
///
/// Status codes:
/// - `404` when the workspace has no stored per-workspace storage row (it uses
///   the global default — there is no per-workspace config to expose);
/// - `500` (Internal) when no provisioner is configured (catalog engine
///   disabled) — a server-config condition, deliberately NOT conflated with the
///   genuine "no custom storage" 404 above.
///
/// The read goes through the `WarehouseProvisioner` trait already on
/// `WorkspaceState`, so this route stays free of any catalog/engine dependency.
async fn get_storage(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<crate::provision::WorkspaceStorageInfo>, AuthError> {
    st.authz
        .require_workspace(&ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    let provisioner = st.warehouse_provisioner.as_ref().ok_or_else(|| {
        AuthError(Error::Internal(anyhow::Error::msg(
            "catalog engine not configured",
        )))
    })?;
    let info = provisioner
        .storage_info(ctx.workspace_id)
        .await
        .map_err(|e| AuthError(Error::Internal(anyhow::anyhow!(e))))?
        .ok_or(AuthError(Error::NotFound("workspace storage".into())))?;
    Ok(Json(info))
}

async fn list_members(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<MemberDto>>, AuthError> {
    st.authz
        .require_workspace(&ctx, "workspace:manage_members")
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
            })
            .collect(),
    ))
}

async fn add_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Json(body): Json<AddMember>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz
        .require_workspace(&ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    st.members
        .add(
            ctx.workspace_id,
            &body.principal_type,
            body.principal_id,
            &body.role,
            ctx.principal.user_id,
        )
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn remove_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Path((_ws, ptype, pid)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz
        .require_workspace(&ctx, "workspace:manage_members")
        .await
        .map_err(AuthError)?;
    st.members
        .remove(ctx.workspace_id, &ptype, pid)
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
