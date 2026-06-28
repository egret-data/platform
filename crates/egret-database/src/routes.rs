//! Workspace-scoped database registry router (`/ws/{workspace}/databases`).
//!
//! ADR-19 per-workspace model: every route is gated on the caller holding
//! `workspace:manage_members` in the path workspace (the same gate Storage uses)
//! and every repo call is scoped to `ctx.workspace_id`, so a database owned by
//! another workspace is never readable, mutable, or testable through this
//! workspace's path (cross-tenant IDOR closed → 404).

use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_secrets::WorkspaceSecretStore;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

use crate::error::{DatabaseError, DbApiError};
use crate::model::{DatabaseDetailDto, DatabaseInput, TestInput, TestOutcome};
use crate::repo::DatabaseRepo;

#[derive(Clone)]
pub struct DatabaseState {
    pub repo: DatabaseRepo,
    pub authz: Authorizer,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    /// Named per-workspace secret layer (Epic B). The create/update routes
    /// resolve a `new_secret`/`secret_id` to a `workspace_secrets` id before
    /// handing a plain `Option<Uuid>` to the repo.
    pub workspace_secrets: WorkspaceSecretStore,
}

// The WsContext extractor needs SessionService, WorkspaceRepo and Authorizer
// from the router state.
impl FromRef<DatabaseState> for SessionService {
    fn from_ref(s: &DatabaseState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<DatabaseState> for WorkspaceRepo {
    fn from_ref(s: &DatabaseState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<DatabaseState> for Authorizer {
    fn from_ref(s: &DatabaseState) -> Self {
        s.authz.clone()
    }
}

const PERM_MANAGE: &str = "workspace:manage_members";

pub fn workspace_database_router() -> Router<DatabaseState> {
    Router::new()
        .route("/ws/{workspace}/databases", post(create).get(list))
        .route("/ws/{workspace}/databases/test", post(test_unsaved))
        .route(
            "/ws/{workspace}/databases/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/ws/{workspace}/databases/{id}/test", post(test_saved))
}

async fn list(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DatabaseDetailDto>>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let dbs = st.repo.list_for_workspace(ctx.workspace_id.0).await?;
    Ok(Json(dbs.into_iter().map(Into::into).collect()))
}

/// Resolve the connection's stored `secret_id` (a `workspace_secrets` id) by
/// Epic-B precedence:
///   (a) `new_secret` present → mint a new named secret and use its id,
///   (b) else `secret_id` present → validate ownership via `get_meta` and use it,
///   (c) else → `existing` (the keep-current id; `None` on create).
async fn resolve_secret_id(
    st: &DatabaseState,
    workspace_id: Uuid,
    body: &DatabaseInput,
    existing: Option<Uuid>,
) -> Result<Option<Uuid>, DbApiError> {
    match (&body.new_secret, body.secret_id) {
        (Some(n), _) => {
            let id = st
                .workspace_secrets
                .create(workspace_id, &n.name, None, &n.value)
                .await
                .map_err(DatabaseError::from)?;
            Ok(Some(id))
        }
        (None, Some(id)) => {
            // Validate the named secret belongs to this workspace (404 otherwise).
            st.workspace_secrets
                .get_meta(workspace_id, id)
                .await
                .map_err(DatabaseError::from)?;
            Ok(Some(id))
        }
        (None, None) => Ok(existing),
    }
}

async fn create(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let secret_id = resolve_secret_id(&st, ctx.workspace_id.0, &body, None).await?;
    let db = st
        .repo
        .create_in_workspace(
            ctx.workspace_id.0,
            &body,
            secret_id,
            Some(ctx.principal.user_id),
        )
        .await?;
    Ok(Json(db.into()))
}

async fn detail(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    Ok(Json(
        st.repo
            .get_in_workspace(ctx.workspace_id.0, id)
            .await?
            .into(),
    ))
}

async fn update(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    // Keep-current default: the row's existing secret id (404 if not owned).
    let existing = st.repo.current_secret_id(ctx.workspace_id.0, id).await?;
    let secret_id = resolve_secret_id(&st, ctx.workspace_id.0, &body, existing).await?;
    Ok(Json(
        st.repo
            .update_in_workspace(
                ctx.workspace_id.0,
                id,
                &body,
                secret_id,
                Some(ctx.principal.user_id),
            )
            .await?
            .into(),
    ))
}

async fn remove(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    st.repo.delete_in_workspace(ctx.workspace_id.0, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_saved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    // Ownership check first: a foreign or absent row is 404 before any secret read.
    let db = st.repo.get_in_workspace(ctx.workspace_id.0, id).await?;
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
        .set_status(
            ctx.workspace_id.0,
            id,
            if outcome.ok { "ok" } else { "error" },
        )
        .await;
    Ok(Json(outcome))
}

async fn test_unsaved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<TestInput>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
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
