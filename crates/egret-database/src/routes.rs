//! Admin CRUD/test/grant routers + the Explorer listing router.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::session::SessionService;
use egret_core::Error;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::DbApiError;
use crate::model::{DatabaseDetailDto, DatabaseInput, DatabaseListDto, TestInput, TestOutcome};
use crate::repo::DatabaseRepo;

#[derive(Clone)]
pub struct DatabaseState {
    pub repo: DatabaseRepo,
    pub authz: Authorizer,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
}

// The Auth extractor needs SessionService from the router state.
impl FromRef<DatabaseState> for SessionService {
    fn from_ref(s: &DatabaseState) -> Self {
        s.sessions.clone()
    }
}

const PERM_MANAGE: &str = "database:manage";

fn is_platform_admin(p: &egret_core::Principal) -> bool {
    p.platform_roles.iter().any(|r| r == "platform_admin")
}

// ── Admin router ──────────────────────────────────────────────────────────────

pub fn admin_database_router() -> Router<DatabaseState> {
    Router::new()
        .route("/databases", post(register).get(list))
        .route("/databases/test", post(test_unsaved))
        .route("/databases/{id}", get(detail).put(update).delete(remove))
        .route("/databases/{id}/test", post(test_saved))
        .route("/databases/{id}/grants", post(grant))
        .route("/databases/{id}/grants/{workspace_id}", delete(revoke))
}

async fn register(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let db = st.repo.create(&body, Some(p.user_id)).await?;
    Ok(Json(db.into()))
}

async fn list(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
) -> Result<Json<Vec<DatabaseDetailDto>>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let dbs = st.repo.list().await?;
    Ok(Json(dbs.into_iter().map(Into::into).collect()))
}

async fn detail(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    Ok(Json(st.repo.get(id).await?.into()))
}

async fn update(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    Ok(Json(
        st.repo.update(id, &body, Some(p.user_id)).await?.into(),
    ))
}

async fn remove(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo.delete(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_saved(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let db = st.repo.get(id).await?;
    let secret = st.repo.open_secret(id).await?;
    let outcome = crate::tester::test_connection(
        db.engine,
        &db.host,
        db.port,
        &db.options,
        secret.as_deref(),
    )
    .await;
    let _ = st
        .repo
        .set_status(id, if outcome.ok { "ok" } else { "error" })
        .await;
    Ok(Json(outcome))
}

async fn test_unsaved(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Json(body): Json<TestInput>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let outcome = crate::tester::test_connection(
        body.engine,
        &body.host,
        body.port,
        &body.options,
        body.secret.as_deref(),
    )
    .await;
    Ok(Json(outcome))
}

#[derive(Deserialize)]
struct GrantBody {
    workspace_id: Uuid,
}

async fn grant(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<GrantBody>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo
        .grant(body.workspace_id, id, Some(p.user_id))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn revoke(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path((id, workspace_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo.revoke(workspace_id, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Explorer (me) router ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WsQuery {
    workspace: String,
}

pub fn me_database_router() -> Router<DatabaseState> {
    Router::new().route("/databases", get(list_my_databases))
}

async fn list_my_databases(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Query(q): Query<WsQuery>,
) -> Result<Json<Vec<DatabaseListDto>>, DbApiError> {
    let ws = st
        .workspaces
        .get_by_slug(&q.workspace)
        .await?
        .filter(|w| w.status == "active")
        .ok_or(Error::NotFound(format!("workspace: {}", q.workspace)))?;
    if !is_platform_admin(&p) {
        let roles = st.authz.workspace_roles_for(p.user_id, ws.id).await?;
        if roles.is_empty() {
            return Err(Error::Forbidden.into());
        }
    }
    let dbs = st.repo.list_for_workspace(ws.id.0).await?;
    Ok(Json(
        dbs.into_iter()
            .map(|d| DatabaseListDto {
                id: d.id.to_string(),
                name: d.name,
                engine: d.engine,
            })
            .collect(),
    ))
}
