//! Workspace-scoped database registry router (`/ws/{workspace}/databases`).
//!
//! ADR-19 per-workspace model: every route but the list is gated on the caller
//! holding `workspace:manage_members` in the path workspace (the same gate
//! Storage uses). The list also answers a member holding `query:run` or
//! `dataset:read`, redacted to id/name/engine (KI-159). Every repo call is scoped to `ctx.workspace_id`, so a database owned by
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

use egret_core::http::ErrorBody;
use egret_core::{Ack, WorkspacePath};

use crate::registry::error::{DatabaseError, DbApiError};
use crate::registry::model::{
    DatabaseDetailDto, DatabaseInput, DatabaseListDto, DatabaseListing, TestInput, TestOutcome,
    WsEngineCatalogDto, WsEngineDetailDto, WsEngineListDto,
};
use crate::registry::repo::DatabaseRepo;

#[derive(Clone)]
pub struct DatabaseState {
    pub repo: DatabaseRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
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
/// Either of these lists the workspace's connections, redacted (D9): the SQL
/// Lab and dataset pickers need the id and name, not how to reach the engine.
const PERMS_LIST_REDACTED: [&str; 2] = ["query:run", "dataset:read"];
/// The ws-member read-only engine view (B2) is gated on `catalog:read`, which
/// every workspace role (ws_admin/ws_editor/ws_viewer) holds — any member may
/// see the engines provisioned to their workspace, unlike the manage-only CRUD.
const PERM_READ: &str = "catalog:read";

/// Reject a mutation against a provisioned connection. A row with a non-null
/// `engine_id` was materialized by an engine→workspace assignment: it is
/// read-only + per-user-delegated and is managed through the assignment API,
/// never edited/deleted/tested directly in the workspace database router.
fn ensure_not_provisioned(db: &egret_query_api::Database) -> Result<(), DbApiError> {
    if db.engine_id.is_some() {
        return Err(DatabaseError::Conflict(
            "this connection is provisioned by an engine assignment and is read-only".into(),
        )
        .into());
    }
    Ok(())
}

pub fn workspace_database_router() -> Router<DatabaseState> {
    Router::new()
        .route("/ws/{workspace}/databases", post(create).get(list))
        .route("/ws/{workspace}/databases/test", post(test_unsaved))
        .route(
            "/ws/{workspace}/databases/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/ws/{workspace}/databases/{id}/test", post(test_saved))
        .route("/ws/{workspace}/engines", get(list_engines))
        .route("/ws/{workspace}/engines/{engine_id}", get(engine_detail))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/databases",
    tag = "databases",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Connections granted to the workspace: the full detail for a workspace admin, id/name/engine only for a member with query:run or dataset:read", body = DatabaseListing),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<DatabaseListing>, DbApiError> {
    let full = allows(&st, &ctx, PERM_MANAGE).await?;
    if !full {
        let mut redacted = false;
        for perm in PERMS_LIST_REDACTED {
            if allows(&st, &ctx, perm).await? {
                redacted = true;
                break;
            }
        }
        if !redacted {
            return Err(egret_core::Error::Forbidden.into());
        }
    }
    let dbs = st.repo.list_for_workspace(ctx.workspace_id.0).await?;
    Ok(Json(if full {
        DatabaseListing::Full(dbs.into_iter().map(Into::into).collect())
    } else {
        DatabaseListing::Summary(dbs.into_iter().map(Into::into).collect())
    }))
}

async fn allows(
    st: &DatabaseState,
    ctx: &egret_core::WorkspaceContext,
    action: &str,
) -> Result<bool, DbApiError> {
    use egret_core::authz::AuthTarget;
    Ok(st
        .pdp
        .authorize(&ctx.subject(), action, &ctx.resource())
        .await?
        .allow)
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
            let user = body.options.user.clone().unwrap_or_default();
            let id = if user.is_empty() {
                st.workspace_secrets
                    .create(Some(workspace_id), &n.name, None, &n.value)
                    .await
                    .map_err(DatabaseError::from)?
            } else {
                let mut data = std::collections::BTreeMap::new();
                data.insert("username".to_string(), user);
                data.insert("password".to_string(), n.value.clone());
                st.workspace_secrets
                    .create_typed(
                        Some(workspace_id),
                        &n.name,
                        None,
                        egret_secrets::secret_type::BASIC_AUTH,
                        &data,
                    )
                    .await
                    .map_err(DatabaseError::from)?
            };
            Ok(Some(id))
        }
        (None, Some(id)) => {
            // Validate the named secret belongs to this workspace (404 otherwise).
            st.workspace_secrets
                .get_meta(Some(workspace_id), id)
                .await
                .map_err(DatabaseError::from)?;
            Ok(Some(id))
        }
        (None, None) => Ok(existing),
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/databases",
    tag = "databases",
    params(WorkspacePath),
    request_body = DatabaseInput,
    responses(
        (status = 200, description = "Database registered", body = DatabaseDetailDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
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

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/databases/{id}",
    tag = "databases",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Database id"),
    ),
    responses(
        (status = 200, description = "Database detail", body = DatabaseDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such database in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn detail(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
    Ok(Json(
        st.repo
            .get_in_workspace(ctx.workspace_id.0, id)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/databases/{id}",
    tag = "databases",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Database id"),
    ),
    request_body = DatabaseInput,
    responses(
        (status = 200, description = "Database updated", body = DatabaseDetailDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such database in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
    // Provisioned (engine-assigned) rows are read-only (404 if not owned first).
    ensure_not_provisioned(&st.repo.get_in_workspace(ctx.workspace_id.0, id).await?)?;
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

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/databases/{id}",
    tag = "databases",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Database id"),
    ),
    responses(
        (status = 200, description = "Deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such database in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
    // Provisioned (engine-assigned) rows are read-only (404 if not owned first).
    ensure_not_provisioned(&st.repo.get_in_workspace(ctx.workspace_id.0, id).await?)?;
    st.repo.delete_in_workspace(ctx.workspace_id.0, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/databases/{id}/test",
    tag = "databases",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Database id"),
    ),
    responses(
        (status = 200, description = "Connection test outcome for the saved database", body = TestOutcome),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such database in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn test_saved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<TestOutcome>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
    // Ownership check first: a foreign or absent row is 404 before any secret read.
    let db = st.repo.get_in_workspace(ctx.workspace_id.0, id).await?;
    // Provisioned (engine-assigned) rows are read-only — no direct connection test.
    ensure_not_provisioned(&db)?;
    let secret = st.repo.open_secret(id).await?;
    let outcome = crate::registry::tester::test_connection(
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

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/databases/test",
    tag = "databases",
    params(WorkspacePath),
    request_body = TestInput,
    responses(
        (status = 200, description = "Connection test outcome for the unsaved connection", body = TestOutcome),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn test_unsaved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<TestInput>,
) -> Result<Json<TestOutcome>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_MANAGE).await?;
    let outcome = crate::registry::tester::test_connection(
        body.engine,
        &body.host,
        body.port,
        &body.options,
        body.secret.as_deref(),
    )
    .await;
    Ok(Json(outcome))
}

// ── B2: ws-member read-only view of assigned engines ──────────────────────────

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/engines",
    tag = "databases",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Engines provisioned (assigned) to the workspace", body = Vec<WsEngineListDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_engines(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WsEngineListDto>>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_READ).await?;
    let engines = st.repo.list_assigned_engines(ctx.workspace_id.0).await?;
    Ok(Json(
        engines
            .into_iter()
            .map(|e| WsEngineListDto {
                id: e.id.to_string(),
                name: e.name,
                engine_type: e.engine_type,
                status: e.status,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/engines/{engine_id}",
    tag = "databases",
    params(
        WorkspacePath,
        ("engine_id" = Uuid, Path, description = "Engine id"),
    ),
    responses(
        (status = 200, description = "Assigned engine detail with catalogs → storage/secret names", body = WsEngineDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing workspace permission", body = ErrorBody),
        (status = 404, description = "No such engine assigned to this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn engine_detail(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, engine_id)): Path<(String, Uuid)>,
) -> Result<Json<WsEngineDetailDto>, DbApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM_READ).await?;
    // 404 (not leaked as an engine that exists) unless it is assigned here.
    let engine = st
        .repo
        .get_assigned_engine(ctx.workspace_id.0, engine_id)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(engine_id.to_string()))?;
    // storage_name belongs to the CATALOG's own warehouse and is resolved per
    // row by the query (review F-8): the list holds other warehouses' catalogs
    // and jdbc/generic ones, which have no warehouse and no storage at all.
    let catalogs = st
        .repo
        .list_engine_catalogs(engine_id, ctx.workspace_id.0)
        .await?;
    Ok(Json(WsEngineDetailDto {
        name: engine.name,
        engine_type: engine.engine_type,
        status: engine.status,
        catalogs: catalogs
            .into_iter()
            .map(|c| WsEngineCatalogDto {
                name: c.name,
                kind: c.kind,
                storage_name: c.storage_name,
                secret_name: c.secret_name,
            })
            .collect(),
    }))
}

/// OpenAPI fragment for the workspace database-registry router. Merged into the
/// Query seam's `egret_query::openapi()`; nested request/response schemas
/// (`Engine`, `ConnOptions`, `NewSecret`, …) are collected automatically by
/// utoipa from the listed component + path types.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list,
        create,
        detail,
        update,
        remove,
        test_saved,
        test_unsaved,
        list_engines,
        engine_detail,
    ),
    components(schemas(
        DatabaseInput,
        DatabaseDetailDto,
        DatabaseListDto,
        DatabaseListing,
        TestInput,
        TestOutcome,
        WsEngineListDto,
        WsEngineDetailDto,
        WsEngineCatalogDto,
        // Exported contract vocabulary (egret-query-api). `Database` is the
        // domain row; not a direct path body but part of the seam's public
        // schema surface, so it's published explicitly.
        egret_query_api::Database,
        Ack,
        ErrorBody,
    )),
    tags((name = "databases", description = "Workspace database registry (engines + connection secrets)")),
)]
pub struct QueryRegistryApi;
