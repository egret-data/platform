//! Workspace-scoped notebook REST routes + object-scope authorization.
//!
//! Notebooks are ROLE-BASED, like BI content (dashboards/charts/datasets): a
//! workspace `ws_viewer` may read every notebook in the workspace and a
//! `ws_editor` may edit every notebook, and per-object shares additionally grant
//! outside users access to a single notebook. Each per-notebook `{id}` handler
//! resolves the notebook to an [`ObjectRef`] (`notebook_object_ref`) and gates on
//! [`require_object`] (READ or WRITE); a folder/workspace/object grant cascades
//! to it exactly as for the BI kinds. `owner_user_id`/`is_public` remain as
//! metadata (creator + a now-cosmetic flag) but no longer gate access.
//! Cross-workspace ids → 404 (don't leak existence); every cell route
//! additionally scopes its write by `notebook_id` (nested-IDOR defense).

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::authz::{require_object, ObjectRef};
use egret_core::http::ErrorBody;
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::{
    CellType, CreateCell, CreateNotebook, NotebookDetail, NotebookSettings, NotebookSummary,
    ReorderCells, SetFolder, SetNotebookWarehouse, UpdateCell, UpdateNotebook,
    UpdateNotebookSettings,
};
use crate::runtime::runtime;
use crate::store::NotebookStore;

const READ: &str = "notebook:read";
const WRITE: &str = "notebook:write";
const MANAGE: &str = "workspace:manage_members";

#[derive(Clone)]
pub struct NotebookState {
    pub store: NotebookStore,
    /// Embedded-catalog RBAC store — used to provision the notebook scratch
    /// role + namespace-scoped write grant when a workspace's notebook
    /// warehouse is set.
    pub catalog: egret_catalog_engine::CatalogStore,
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
        .route(
            "/ws/{workspace}/notebook-settings/warehouse",
            put(set_notebook_warehouse),
        )
        .route("/ws/{workspace}/notebook-runtime", get(runtime))
}

/// `?trashed=true` selects the trash bin instead of the live list.
#[derive(Debug, serde::Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub trashed: bool,
}

/// Resolve a notebook to the [`ObjectRef`] (folder + workspace ancestry) the PDP
/// needs, asserting it lives in the path workspace. `NotFound` if the notebook is
/// absent OR belongs to another workspace (cross-tenant → don't leak existence).
/// Callers pass the result to [`require_object`], which does the actual
/// role/share authorization — this function no longer inspects owner/public.
async fn notebook_object_ref(
    st: &NotebookState,
    ctx: &WorkspaceContext,
    id: Uuid,
) -> Result<ObjectRef, NotebookApiError> {
    let (summary, ws) = st
        .store
        .get_summary(id)
        .await?
        .ok_or_else(|| Error::NotFound("notebook".into()))?;
    if ws != ctx.workspace_id.0 {
        return Err(Error::NotFound("notebook".into()).into());
    }
    Ok(ObjectRef {
        kind: "notebook".into(),
        id,
        folder_id: summary.folder_id,
        workspace_id: ctx.workspace_id,
    })
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
        (status = 200, description = "All notebooks in the workspace (role-based)", body = Vec<NotebookSummary>),
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
        .list_in_workspace(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, READ, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, READ, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, READ, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
    CellType::parse(&b.cell_type).ok_or_else(|| Error::InvalidInput("invalid cell_type".into()))?;
    st.store
        .add_cell(
            id,
            &b.cell_type,
            b.source.as_deref().unwrap_or(""),
            b.cell_order.unwrap_or(0),
            b.sql_target.as_deref(),
            b.sql_into.as_deref(),
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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
    let obj = notebook_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, WRITE, obj)
        .await
        .map_err(NotebookApiError::from)?;
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

/// Map an embedded-catalog store error onto the shared HTTP error taxonomy.
fn cat_err(e: egret_catalog_engine::CatalogError) -> NotebookApiError {
    use egret_catalog_engine::CatalogError as C;
    NotebookApiError::from(match e {
        C::NotFound(m) => Error::NotFound(m),
        C::AlreadyExists(m) => Error::Conflict(m),
        C::InUse => Error::Conflict("in use".into()),
        C::BadRequest(m) => Error::InvalidInput(m),
        C::Db(db) => Error::Internal(anyhow::anyhow!(db)),
    })
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/notebook-settings/warehouse",
    tag = "notebooks",
    params(WorkspacePath),
    request_body = SetNotebookWarehouse,
    responses(
        (status = 200, description = "Notebook warehouse set; catalog role + write grant provisioned", body = NotebookSettings),
        (status = 400, description = "Warehouse not found or not owned by this workspace", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace:manage_members permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_notebook_warehouse(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Json(b): Json<SetNotebookWarehouse>,
) -> Result<Json<NotebookSettings>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(NotebookApiError::from)?;

    // The warehouse must exist AND belong to this workspace (cross-tenant → 400,
    // no partial mutation).
    let wh = st
        .catalog
        .get_warehouse(b.warehouse_id)
        .await
        .map_err(cat_err)?
        .ok_or_else(|| NotebookApiError::from(Error::InvalidInput("warehouse not found".into())))?;
    if wh.workspace_id != ctx.workspace_id.0 {
        return Err(NotebookApiError::from(Error::InvalidInput(
            "warehouse not in this workspace".into(),
        )));
    }

    let ns = b
        .namespace
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| vec!["notebook".to_string()]);

    st.store
        .set_notebook_warehouse(ctx.workspace_id.0, b.warehouse_id, &ns)
        .await?;

    // ── Provision (idempotent) the notebook scratch role + namespace write grant ──
    // `create_role` errors `AlreadyExists` on a re-PUT; fall back to the existing
    // role. `add_grant` dedupes identical grants (also `AlreadyExists`), which we
    // treat as success — so a re-PUT is a guaranteed no-op-or-success.
    let role_id = match st
        .catalog
        .create_role(
            b.warehouse_id,
            "notebook",
            "notebook",
            Some("notebook scratch"),
        )
        .await
    {
        Ok(role) => role.id,
        Err(egret_catalog_engine::CatalogError::AlreadyExists(_)) => {
            st.catalog
                .list_roles(b.warehouse_id)
                .await
                .map_err(cat_err)?
                .into_iter()
                .find(|r| r.name == "notebook")
                .ok_or_else(|| {
                    NotebookApiError::from(Error::Internal(anyhow::anyhow!(
                        "notebook role missing after AlreadyExists"
                    )))
                })?
                .id
        }
        Err(e) => return Err(cat_err(e)),
    };

    match st
        .catalog
        .add_grant(
            role_id,
            &egret_catalog_engine::GrantInput {
                securable_type: "namespace".into(),
                namespace: Some(ns.clone()),
                table_name: None,
                privilege: "write".into(),
            },
        )
        .await
    {
        Ok(_) => {}
        // An identical grant already exists — idempotent success.
        Err(egret_catalog_engine::CatalogError::AlreadyExists(_)) => {}
        Err(e) => return Err(cat_err(e)),
    }

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
        set_notebook_warehouse,
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
        SetNotebookWarehouse,
        crate::model::EngineOption,
        crate::runtime::NotebookRuntime,
        ErrorBody,
    )),
    tags((name = "notebooks", description = "Per-workspace notebooks: CRUD, cells, folders, trash, bookmarks, settings + kernel runtime manifest")),
)]
pub struct NotebookApi;
