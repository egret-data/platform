//! Workspace-scoped notebook REST routes + ownership enforcement.
//!
//! Ownership rules (defense-in-depth on top of the store's SQL guards):
//!   - **read** (GET one): allowed iff the notebook is in the caller's workspace
//!     AND (the caller owns it OR it is public). Cross-workspace → 404,
//!     not-own-not-public → 403.
//!   - **write** (PATCH / DELETE / all cell mutations): owner ONLY — there is
//!     deliberately **no** `platform_admin` override (unlike egret-engine).
//!   - every cell route resolves its parent notebook and runs `write_scoped`
//!     before mutating, and the store additionally scopes by `notebook_id`.

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::{Error, WorkspaceContext};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::{
    CellType, CreateCell, CreateNotebook, NotebookDetail, NotebookSummary, ReorderCells, SetFolder,
    UpdateCell, UpdateNotebook,
};
use crate::store::NotebookStore;

const READ: &str = "notebook:read";
const WRITE: &str = "notebook:write";

#[derive(Clone)]
pub struct NotebookState {
    pub store: NotebookStore,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
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

async fn list(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<NotebookSummary>>, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    let nbs = st
        .store
        .list_visible(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await?;
    Ok(Json(nbs))
}

async fn create(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateNotebook>,
) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
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

async fn get_one(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<NotebookDetail>, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, READ)
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

async fn update(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateNotebook>,
) -> Result<Json<NotebookDetail>, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
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

async fn remove(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Soft-delete (move to trash). Owner only.
async fn trash(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.trash(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Restore from trash. Owner only.
async fn restore(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.restore(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Move into a folder (or back to Uncategorized with `folder_id: null`).
/// Owner only; when a folder is given it must belong to this workspace and
/// carry `entity_type='notebook'`, else 404 (don't leak foreign folders).
async fn set_folder(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<SetFolder>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
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
async fn bookmark_add(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    read_scoped(&st, &ctx, id).await?;
    st.store.add_bookmark(ctx.principal.user_id.0, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove the calling user's bookmark. Read scope.
async fn bookmark_remove(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, READ)
        .await
        .map_err(NotebookApiError::from)?;
    read_scoped(&st, &ctx, id).await?;
    st.store
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<CreateCell>,
) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
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

async fn update_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>,
    Json(b): Json<UpdateCell>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    if let Some(t) = &b.cell_type {
        CellType::parse(t).ok_or_else(|| Error::InvalidInput("invalid cell_type".into()))?;
    }
    st.store.update_cell(id, cid, &b).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_cell(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete_cell(id, cid).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reorder(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<ReorderCells>,
) -> Result<StatusCode, NotebookApiError> {
    st.authz
        .require_workspace(&ctx, WRITE)
        .await
        .map_err(NotebookApiError::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.reorder(id, &b.order).await?;
    Ok(StatusCode::NO_CONTENT)
}
