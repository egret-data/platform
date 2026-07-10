//! Workspace-scoped notebook REST routes + ownership enforcement.
//!
//! Ownership rules (defense-in-depth on top of the store's SQL guards):
//!   - **read** (GET one): allowed iff the notebook is in the caller's workspace
//!     AND (the caller owns it OR it is public). Cross-workspace → 404,
//!     not-own-not-public → 403.
//!   - **write** (PATCH / DELETE / all cell mutations): owner ONLY — there is
//!     deliberately **no** `platform_admin` override (unlike the query seam).
//!   - every cell route resolves its parent notebook and runs `write_scoped`
//!     before mutating, and the store additionally scopes by `notebook_id`.

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::http::ErrorBody;
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::{
    CellType, CreateCell, CreateNotebook, NotebookDetail, NotebookSettings, NotebookSummary,
    ReorderCells, SetFolder, UpdateCell, UpdateNotebook, UpdateNotebookSettings,
};
use crate::runtime::runtime;
use crate::store::NotebookStore;

const READ: &str = "notebook:read";
const WRITE: &str = "notebook:write";
const MANAGE: &str = "workspace:manage_members";

#[derive(Clone)]
pub struct NotebookState {
    pub store: NotebookStore,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    /// UserRepo for `ensure_username`.
    pub users: UserRepo,
    /// Base URL of the Iceberg REST catalog (e.g. `http://localhost:8181`).
    /// Returned verbatim in the runtime manifest so the kernel can connect.
    pub catalog_base_url: String,
    /// Mints short-lived per-user engine JWTs for the runtime manifest.
    pub engine_minter: std::sync::Arc<dyn crate::engine_token::EngineTokenMinter>,
}

impl FromRef<NotebookState> for SessionService {
    fn from_ref(s: &NotebookState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<NotebookState> for WorkspaceRepo {
    fn from_ref(s: &NotebookState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<NotebookState> for Authorizer {
    fn from_ref(s: &NotebookState) -> Self {
        s.authz.clone()
    }
}

pub fn notebook_router() -> Router<NotebookState> {
    Router::new()
        .route("/ws/{workspace}/notebooks", get(list).post(create))
        .route(
            "/ws/{workspace}/notebooks/{id}",
            get(get_one).patch(update).delete(remove),
        )
        .route("/ws/{workspace}/notebooks/{id}/trash", post(trash))
        .route("/ws/{workspace}/notebooks/{id}/restore", post(restore))
        .route("/ws/{workspace}/notebooks/{id}/folder", patch(set_folder))
        .route(
            "/ws/{workspace}/notebooks/{id}/bookmark",
            post(bookmark_add).delete(bookmark_remove),
        )
        .route("/ws/{workspace}/notebooks/{id}/cells", post(add_cell))
        .route("/ws/{workspace}/notebooks/{id}/cells/order", put(reorder))
        .route(
            "/ws/{workspace}/notebooks/{id}/cells/{cid}",
            patch(update_cell).delete(delete_cell),
        )
        .route(
            "/ws/{workspace}/notebook-settings",
            get(get_settings).put(update_settings),
        )
        .route("/ws/{workspace}/notebook-runtime", get(runtime))
}

/// `?trashed=true` selects the trash bin instead of the live list.
#[derive(Debug, serde::Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub trashed: bool,
}

/// Read scope: resolve a notebook and assert the caller may *view* it.
/// Cross-workspace → 404 (don't leak existence); not-own-not-public → 403.
async fn read_scoped(
    st: &NotebookState,
    ctx: &WorkspaceContext,
    id: Uuid,
) -> Result<NotebookSummary, NotebookApiError> {
    let (mut summary, ws) = st
        .store
        .get_summary(id)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    if ws != ctx.workspace_id.0 {
        return Err(Error::NotFound("notebook".into()).into());
    }
    let me = ctx.principal.user_id.0;
    if summary.owner_user_id != me && !summary.is_public {
        return Err(Error::Forbidden.into());
    }
    summary.is_owner = summary.owner_user_id == me;
    Ok(summary)
}

/// Write scope: owner ONLY. No `platform_admin` override by design.
/// Cross-workspace → 404; non-owner (incl. platform_admin) → 403.
async fn write_scoped(
    st: &NotebookState,
    ctx: &WorkspaceContext,
    id: Uuid,
) -> Result<(), NotebookApiError> {
    let (summary, ws) = st
        .store
        .get_summary(id)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    if ws != ctx.workspace_id.0 {
        return Err(Error::NotFound("notebook".into()).into());
    }
    if summary.owner_user_id != ctx.principal.user_id.0 {
        return Err(Error::Forbidden.into());
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebooks",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List the trash bin instead of live notebooks when true"),
    ),
    responses(
        (status = 200, description = "Notebooks visible to the caller (own + public)", body = Vec<NotebookSummary>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<NotebookSummary>>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    let nbs = st
        .store
        .list_visible(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await?;
    Ok(Json(nbs))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks",
    tag = "notebooks",
    params(WorkspacePath),
    request_body = CreateNotebook,
    responses(
        (status = 201, description = "Notebook created", body = NotebookDetail),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateNotebook>,
) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    let name = b.name.as_deref().unwrap_or("Untitled");
    let id = st
        .store
        .create(ctx.workspace_id.0, ctx.principal.user_id.0, name)
        .await?;
    let detail = st
        .store
        .get_with_cells(id, ctx.principal.user_id.0)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok((StatusCode::CREATED, Json(detail)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebooks/{id}",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 200, description = "Notebook detail (summary + cells)", body = NotebookDetail),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_one(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<NotebookDetail>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    // Enforce ws-scope + (own OR public) before reading.
    read_scoped(&st, &ctx, id).await?;
    let detail = st
        .store
        .get_with_cells(id, ctx.principal.user_id.0)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok(Json(detail))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/notebooks/{id}",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    request_body = UpdateNotebook,
    responses(
        (status = 200, description = "Notebook updated", body = NotebookDetail),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateNotebook>,
) -> Result<Json<NotebookDetail>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.update(id, &b).await?;
    let detail = st
        .store
        .get_with_cells(id, ctx.principal.user_id.0)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok(Json(detail))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/notebooks/{id}",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Notebook deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Soft-delete (move to trash). Owner only.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{id}/trash",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Notebook moved to trash"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn trash(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.trash(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Restore from trash. Owner only.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{id}/restore",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Notebook restored from trash"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restore(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.restore(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Move into a folder (or back to Uncategorized with `folder_id: null`).
/// Owner only; when a folder is given it must belong to this workspace and
/// carry `entity_type='notebook'`, else 404 (don't leak foreign folders).
#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/notebooks/{id}/folder",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 204, description = "Notebook moved to the folder (or back to Uncategorized)"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook, or target folder not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_folder(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<SetFolder>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    if let Some(fid) = b.folder_id {
        match st.store.folder_kind(fid).await? {
            Some((w, t)) if w == ctx.workspace_id.0 && t == "notebook" => {}
            _ => return Err(Error::NotFound("folder".into()).into()),
        }
    }
    st.store.set_folder(id, b.folder_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Bookmark a notebook for the calling user. Read scope (any readable notebook).
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{id}/bookmark",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Notebook bookmarked for the caller"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn bookmark_add(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    read_scoped(&st, &ctx, id).await?;
    st.store.add_bookmark(ctx.principal.user_id.0, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove the calling user's bookmark. Read scope.
#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/notebooks/{id}/bookmark",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Notebook bookmark removed for the caller"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn bookmark_remove(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    read_scoped(&st, &ctx, id).await?;
    st.store
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{id}/cells",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    request_body = CreateCell,
    responses(
        (status = 201, description = "Cell added; the full notebook is returned", body = NotebookDetail),
        (status = 400, description = "Invalid cell_type (must be code/markdown/sql)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<CreateCell>,
) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    CellType::parse(&b.cell_type).ok_or_else(|| Error::InvalidInput("invalid cell_type".into()))?;
    st.store
        .add_cell(
            id,
            &b.cell_type,
            b.source.as_deref().unwrap_or(""),
            b.cell_order.unwrap_or(0),
        )
        .await?;
    let detail = st
        .store
        .get_with_cells(id, ctx.principal.user_id.0)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok((StatusCode::CREATED, Json(detail)))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/notebooks/{id}/cells/{cid}",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
        ("cid" = Uuid, Path, description = "Cell id"),
    ),
    request_body = UpdateCell,
    responses(
        (status = 204, description = "Cell updated"),
        (status = 400, description = "Invalid cell_type (must be code/markdown/sql)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>,
    Json(b): Json<UpdateCell>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    if let Some(t) = &b.cell_type {
        CellType::parse(t).ok_or_else(|| Error::InvalidInput("invalid cell_type".into()))?;
    }
    st.store.update_cell(id, cid, &b).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/notebooks/{id}/cells/{cid}",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
        ("cid" = Uuid, Path, description = "Cell id"),
    ),
    responses(
        (status = 204, description = "Cell deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete_cell(id, cid).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/notebooks/{id}/cells/order",
    tag = "notebooks",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Notebook id"),
    ),
    request_body = ReorderCells,
    responses(
        (status = 204, description = "Cells reordered"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:write permission, or not the notebook owner", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn reorder(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<ReorderCells>,
) -> Result<StatusCode, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.reorder(id, &b.order).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebook-settings",
    tag = "notebooks",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Notebook engine settings for the workspace", body = NotebookSettings),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_settings(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookSettings>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    let settings = st.store.get_settings(ctx.workspace_id.0).await?;
    Ok(Json(settings))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/notebook-settings",
    tag = "notebooks",
    params(WorkspacePath),
    request_body = UpdateNotebookSettings,
    responses(
        (status = 200, description = "Notebook engine settings replaced", body = NotebookSettings),
        (status = 400, description = "An engine id does not belong to this workspace (whole PUT rejected)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace:manage_members permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_settings(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Json(body): Json<UpdateNotebookSettings>,
) -> Result<Json<NotebookSettings>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(NotebookApiError::from)?;
    // Atomic: the engine id set is validated up front, then replaced in one
    // transaction. A cross-tenant id rejects the whole PUT (400) with no
    // partial mutation.
    st.store
        .set_notebook_access(ctx.workspace_id.0, &body.engine_ids)
        .await?;
    let settings = st.store.get_settings(ctx.workspace_id.0).await?;
    Ok(Json(settings))
}

// ── OpenAPI fragment (E0·P4b-8) ──────────────────────────────────────────────
//
// The `#[derive(utoipa::OpenApi)]` doc struct for the notebook router, merged
// into `egret_notebook::openapi()`. The `runtime` handler lives in
// `crate::runtime`, so it's referenced by full path. Nested schemas
// (`Cell`, `EngineOption`, …) are collected automatically from the listed
// component + path types. `SetFolder` is published under the schema name
// `NotebookSetFolder` (its `#[schema(as = …)]`) to avoid a global collision with
// the BI seam's `SetFolder`.

/// OpenAPI fragment for the notebook router (`/api/ws/{workspace}/notebooks…`,
/// `/notebook-settings`, `/notebook-runtime`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list,
        create,
        get_one,
        update,
        remove,
        trash,
        restore,
        set_folder,
        bookmark_add,
        bookmark_remove,
        add_cell,
        reorder,
        update_cell,
        delete_cell,
        get_settings,
        update_settings,
        crate::runtime::runtime,
    ),
    components(schemas(
        NotebookSummary,
        NotebookDetail,
        crate::model::Cell,
        CreateNotebook,
        UpdateNotebook,
        CreateCell,
        UpdateCell,
        ReorderCells,
        SetFolder,
        NotebookSettings,
        UpdateNotebookSettings,
        crate::model::EngineOption,
        crate::runtime::NotebookRuntime,
        ErrorBody,
    )),
    tags((name = "notebooks", description = "Per-workspace notebooks: CRUD, cells, folders, trash, bookmarks, settings + kernel runtime manifest")),
)]
pub struct NotebookApi;
