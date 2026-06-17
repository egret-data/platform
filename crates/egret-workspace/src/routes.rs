//! HTTP state + routers for workspace administration and membership.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::{Error, Pagination, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::members::MemberRepo;
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

#[derive(Clone)]
pub struct WorkspaceState {
    pub workspaces: WorkspaceRepo,
    pub members: MemberRepo,
    pub authz: Authorizer,
    pub users: UserRepo,
    pub sessions: SessionService,
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

#[derive(Deserialize)]
pub struct CreateWorkspace {
    pub slug: String,
    pub name: String,
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
