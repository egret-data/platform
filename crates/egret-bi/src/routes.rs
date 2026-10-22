//! SQL Lab routes: saved queries + schema browse. Dataset CRUD routes.

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::http::ErrorBody;
use egret_core::Error;
use egret_core::{Ack, WorkspacePath};
use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query::DatabaseRepo;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use std::time::Duration;
use uuid::Uuid;

use crate::chartquery;
use crate::dataset::{NewColumn, SourceSpec};
use crate::error::BiApiError;
use crate::expr::{validate_expression, validate_metric_expression};
use crate::model::{
    AddColumnBody, CardInput, ChartDataBody, ChartDataResponse, ChartDto, ChartRunResponse,
    ConfirmQuery, CreateChart, CreateDashboard, CreateDataset, CreateFolder, CreateSource,
    DashboardCardDto, DashboardDetailDto, DashboardRevisionDto, DashboardSummaryDto,
    DashboardTabDto, DatasetColumnDto, DatasetDetailDto, DatasetRefreshDto, DatasetSummaryDto,
    DuplicateDataset, FolderDto, MetricBody, MetricDto, NewTabInput, RenameFolder, SaveQuery,
    SavedQueryDto, SchemaColumn, SetDashboardCertification, SetDatasetCertification, SetFolder,
    UpdateChart, UpdateColumnBody, UpdateDashboard, UpdateDataset,
};
use crate::saved::SavedQueryRepo;
use crate::schema::SchemaService;
use crate::typegen::generic_type;

#[derive(Clone)]
pub struct BiState {
    pub db: egret_metastore::Db,
    pub saved: SavedQueryRepo,
    pub schema: SchemaService,
    pub databases: DatabaseRepo,
    pub users: egret_auth::users::UserRepo,
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub datasets: crate::dataset::DatasetRepo,
    pub charts: crate::chart::ChartRepo,
    pub dashboards: crate::dashboard::DashboardRepo,
    pub folders: crate::folder::FolderRepo,
    /// Append-only audit sink (B4). Emits Mutation events at object-grant
    /// changes; best-effort, never fails the request.
    pub audit: egret_audit::AuditSink,
}
impl FromRef<BiState> for SessionService {
    fn from_ref(s: &BiState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<BiState> for WorkspaceRepo {
    fn from_ref(s: &BiState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<BiState> for Authorizer {
    fn from_ref(s: &BiState) -> Self {
        s.authz.clone()
    }
}

const PERM: &str = "query:run";
const SQ_READ: &str = "saved_query:read";
const SQ_MANAGE: &str = "saved_query:manage";
const DS_READ: &str = "dataset:read";
const DS_MANAGE: &str = "dataset:manage";
/// PLATFORM-scoped (0109), like `dashboard:certify`. Holding `dataset:manage` — or owning the
/// dataset — grants nothing here; that asymmetry is the feature.
const DS_CERTIFY: &str = "dataset:certify";
const CH_READ: &str = "chart:read";
const CH_MANAGE: &str = "chart:manage";
const DB_READ: &str = "dashboard:read";
const DB_MANAGE: &str = "dashboard:manage";
/// Platform-scoped: certifying is a stewardship act, not an editing one. See rbac.rs.
const DB_CERTIFY: &str = "dashboard:certify";

pub fn sqllab_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/sqllab/queries", post(save).get(list))
        .route(
            "/ws/{workspace}/sqllab/queries/{id}",
            get(load).put(update).delete(remove),
        )
        .route(
            "/ws/{workspace}/sqllab/queries/{id}/folder",
            patch(sq_set_folder),
        )
        .route(
            "/ws/{workspace}/sqllab/databases/{db}/catalogs",
            get(catalogs),
        )
        .route(
            "/ws/{workspace}/sqllab/databases/{db}/schemas",
            get(schemas),
        )
        .route("/ws/{workspace}/sqllab/databases/{db}/tables", get(tables))
        .route(
            "/ws/{workspace}/sqllab/databases/{db}/columns",
            get(columns),
        )
        .route("/ws/{workspace}/sqllab/databases/{db}/search", get(search))
        .route(
            "/ws/{workspace}/sqllab/snippets",
            get(snippets_list).post(snippets_create),
        )
        .route(
            "/ws/{workspace}/sqllab/snippets/{id}",
            put(snippets_update).delete(snippets_remove),
        )
}

fn snippet_dto(s: crate::snippet::Snippet) -> crate::model::SnippetDto {
    crate::model::SnippetDto {
        id: s.id.to_string(),
        name: s.name,
        body: s.body,
        description: s.description,
        updated_at: s.updated_at,
    }
}

fn dto(q: crate::saved::SavedQuery) -> SavedQueryDto {
    SavedQueryDto {
        id: q.id.to_string(),
        database_id: q.database_id.map(|d| d.to_string()),
        name: q.name,
        sql: q.sql,
        description: q.description,
        created_by: q.created_by.map(|u| u.to_string()),
        folder_id: q.folder_id.map(|f| f.to_string()),
        updated_at: q.updated_at,
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/sqllab/queries",
    tag = "sqllab",
    params(WorkspacePath),
    request_body = SaveQuery,
    responses(
        (status = 200, description = "Saved query created", body = SavedQueryDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn save(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<SaveQuery>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    let q = st
        .saved
        .create(
            ctx.workspace_id.0,
            b.database_id,
            &b.name,
            &b.sql,
            &b.description,
            Some(ctx.principal.user_id),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(dto(q)))
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/queries",
    tag = "sqllab",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Saved queries in the workspace", body = Vec<SavedQueryDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<SavedQueryDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_READ)
        .await
        .map_err(BiApiError::from)?;
    let qs = st
        .saved
        .list(ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(qs.into_iter().map(dto).collect()))
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/queries/{id}",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Saved query id"),
    ),
    responses(
        (status = 200, description = "Saved query detail", body = SavedQueryDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:read permission", body = ErrorBody),
        (status = 404, description = "No such saved query", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn load(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let q = st
        .saved
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("saved query".into()))?;
    Ok(Json(dto(q)))
}
#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/sqllab/queries/{id}",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Saved query id"),
    ),
    request_body = SaveQuery,
    responses(
        (status = 200, description = "Saved query updated", body = SavedQueryDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:manage permission", body = ErrorBody),
        (status = 404, description = "No such saved query", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<SaveQuery>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    let q = st
        .saved
        .update(id, &b.name, &b.sql, &b.description, b.database_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(dto(q)))
}
#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/sqllab/queries/{id}",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Saved query id"),
    ),
    responses(
        (status = 200, description = "Saved query deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:manage permission", body = ErrorBody),
        (status = 404, description = "No such saved query", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.saved.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/sqllab/queries/{id}/folder",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Saved query id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 200, description = "Saved query moved to folder", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:manage permission", body = ErrorBody),
        (status = 404, description = "No such saved query or target folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn sq_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "saved_query").await?;
    }
    st.saved
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn conn_spec(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    db: Uuid,
) -> Result<ConnSpec, BiApiError> {
    conn_spec_perm(st, ctx, db, PERM).await
}

async fn conn_spec_perm(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    db: Uuid,
    perm: &str,
) -> Result<ConnSpec, BiApiError> {
    egret_core::authz::require(&*st.pdp, ctx, perm)
        .await
        .map_err(BiApiError::from)?;
    conn_spec_granted(st, ctx, db).await
}

/// Build a `ConnSpec` for a workspace-granted database WITHOUT an authz gate —
/// the caller has already authorized (e.g. object-scope `require_object`). Still
/// enforces that the database is granted to the workspace.
///
/// Applies the same per-user delegation the BI data path uses
/// ([`resolve_delegation`]): a JWT-delegated engine requires the caller's engine
/// JWT (Trino rejects an unauthenticated request with 403 → surfaced as
/// `engine: forbidden`), and impersonation needs the resolved `session_user`.
/// Skipping this is exactly what made schema browsing (catalogs/schemas/tables/
/// columns) 502 on delegated engines while query execution worked.
pub async fn conn_spec_granted(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    db: Uuid,
) -> Result<ConnSpec, BiApiError> {
    let granted = st
        .databases
        .list_for_workspace(ctx.workspace_id.0)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    if !granted.iter().any(|d| d.id == db) {
        return Err(Error::NotFound("database not granted".into()).into());
    }
    let database = st
        .databases
        .get(db)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let secret = st
        .databases
        .open_secret(db)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    // Delegated engines require the caller's per-user identity/JWT on EVERY
    // connection — including these read-only meta-queries. Reuse the data
    // path's picker so browse behaves like execution.
    let (session_user, bearer) = resolve_delegation(st, ctx, db).await?;
    Ok(ConnSpec::from_database_as(
        &database,
        secret,
        session_user,
        bearer,
    ))
}

#[derive(Deserialize)]
struct CatQ {
    catalog: Option<String>,
}
#[derive(Deserialize)]
struct SchemaQ {
    catalog: Option<String>,
    schema: Option<String>,
}
#[derive(Deserialize)]
struct ColQ {
    catalog: Option<String>,
    schema: Option<String>,
    table: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/databases/{db}/catalogs",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("db" = Uuid, Path, description = "Granted database id"),
    ),
    responses(
        (status = 200, description = "Catalog names visible on the database", body = Vec<String>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "Database not granted to this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn catalogs(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, db)): Path<(String, Uuid)>,
) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(
        st.schema.catalogs(&spec).await.map_err(BiApiError::from)?,
    ))
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/databases/{db}/schemas",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("db" = Uuid, Path, description = "Granted database id"),
        ("catalog" = Option<String>, Query, description = "Catalog to list schemas in"),
    ),
    responses(
        (status = 200, description = "Schema names in the catalog", body = Vec<String>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "Database not granted to this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn schemas(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, db)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<CatQ>,
) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(
        st.schema
            .schemas(&spec, q.catalog.as_deref().unwrap_or(""))
            .await
            .map_err(BiApiError::from)?,
    ))
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/databases/{db}/tables",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("db" = Uuid, Path, description = "Granted database id"),
        ("catalog" = Option<String>, Query, description = "Catalog to scope to"),
        ("schema" = Option<String>, Query, description = "Schema to list tables in"),
    ),
    responses(
        (status = 200, description = "Table names in the schema", body = Vec<String>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "Database not granted to this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn tables(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, db)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<SchemaQ>,
) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(
        st.schema
            .tables(
                &spec,
                q.catalog.as_deref().unwrap_or(""),
                q.schema.as_deref().unwrap_or(""),
            )
            .await
            .map_err(BiApiError::from)?,
    ))
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/databases/{db}/columns",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("db" = Uuid, Path, description = "Granted database id"),
        ("catalog" = Option<String>, Query, description = "Catalog to scope to"),
        ("schema" = Option<String>, Query, description = "Schema to scope to"),
        ("table" = Option<String>, Query, description = "Table to describe columns for"),
    ),
    responses(
        (status = 200, description = "Columns (name + type) of the table", body = Vec<SchemaColumn>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "Database not granted to this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn columns(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, db)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<ColQ>,
) -> Result<Json<Vec<crate::model::SchemaColumn>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(
        st.schema
            .columns(
                &spec,
                q.catalog.as_deref().unwrap_or(""),
                q.schema.as_deref().unwrap_or(""),
                q.table.as_deref().unwrap_or(""),
            )
            .await
            .map_err(BiApiError::from)?,
    ))
}

#[derive(Deserialize)]
struct SearchQ {
    q: Option<String>,
}
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/sqllab/databases/{db}/search",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("db" = Uuid, Path, description = "Granted database id"),
        ("q" = Option<String>, Query, description = "Object-name search term"),
    ),
    responses(
        (status = 200, description = "Matching tables/views across the database", body = Vec<crate::model::SearchHit>),
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "Database not granted to this workspace", body = ErrorBody),
    ),
)]
async fn search(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, db)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<SearchQ>,
) -> Result<Json<Vec<crate::model::SearchHit>>, BiApiError> {
    let term = q.q.unwrap_or_default();
    if term.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(
        st.schema
            .search(&spec, term.trim())
            .await
            .map_err(BiApiError::from)?,
    ))
}

// ── SQL snippets ──────────────────────────────────────────────────────────
fn snippet_repo(st: &BiState) -> crate::snippet::SnippetRepo {
    crate::snippet::SnippetRepo::new(st.db.clone())
}

#[utoipa::path(
    get, path = "/api/ws/{workspace}/sqllab/snippets", tag = "sqllab", params(WorkspacePath),
    responses((status = 200, description = "Workspace SQL snippets", body = Vec<crate::model::SnippetDto>)),
)]
async fn snippets_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<crate::model::SnippetDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_READ)
        .await
        .map_err(BiApiError::from)?;
    let items = snippet_repo(&st)
        .list(ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(items.into_iter().map(snippet_dto).collect()))
}

#[utoipa::path(
    post, path = "/api/ws/{workspace}/sqllab/snippets", tag = "sqllab", params(WorkspacePath),
    request_body = crate::model::SnippetBody,
    responses((status = 200, description = "Created snippet", body = crate::model::SnippetDto)),
)]
async fn snippets_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<crate::model::SnippetBody>,
) -> Result<Json<crate::model::SnippetDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    let s = snippet_repo(&st)
        .create(
            ctx.workspace_id.0,
            &b.name,
            &b.body,
            &b.description,
            Some(ctx.principal.user_id),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(snippet_dto(s)))
}

#[utoipa::path(
    put, path = "/api/ws/{workspace}/sqllab/snippets/{id}", tag = "sqllab",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Snippet id")),
    request_body = crate::model::SnippetBody,
    responses(
        (status = 200, description = "Updated snippet", body = crate::model::SnippetDto),
        (status = 404, description = "Snippet not found in this workspace", body = ErrorBody),
    ),
)]
async fn snippets_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SnippetBody>,
) -> Result<Json<crate::model::SnippetDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    let s = snippet_repo(&st)
        .update(ctx.workspace_id.0, id, &b.name, &b.body, &b.description)
        .await
        .map_err(BiApiError::from)?
        .ok_or_else(|| BiApiError::from(Error::NotFound("snippet".into())))?;
    Ok(Json(snippet_dto(s)))
}

#[utoipa::path(
    delete, path = "/api/ws/{workspace}/sqllab/snippets/{id}", tag = "sqllab",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Snippet id")),
    responses((status = 200, description = "Deleted", body = Ack)),
)]
async fn snippets_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SQ_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    snippet_repo(&st)
        .delete(ctx.workspace_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Dataset routes ──────────────────────────────────────────────────────────

pub fn dataset_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/datasets", post(ds_create).get(ds_list))
        .route(
            "/ws/{workspace}/datasets/{id}",
            get(ds_get).put(ds_update).delete(ds_remove),
        )
        .route("/ws/{workspace}/datasets/{id}/refresh", post(ds_refresh))
        .route(
            "/ws/{workspace}/datasets/{id}/duplicate",
            post(ds_duplicate),
        )
        // Certification is its own route for the same reason the dashboard's is: it is guarded by
        // a PLATFORM permission, and folding it into the object's update body is precisely how an
        // editor used to be able to certify their own work.
        .route(
            "/ws/{workspace}/datasets/{id}/certification",
            put(ds_set_certification),
        )
        .route("/ws/{workspace}/datasets/{id}/metrics", post(ds_add_metric))
        .route(
            "/ws/{workspace}/datasets/{id}/metrics/{metric_id}",
            put(ds_update_metric).delete(ds_delete_metric),
        )
        .route(
            "/ws/{workspace}/datasets/{id}/metrics/{metric_id}/certification",
            put(ds_set_metric_certification),
        )
        .route("/ws/{workspace}/datasets/{id}/columns", post(ds_add_column))
        .route(
            "/ws/{workspace}/datasets/{id}/columns/{column_name}",
            put(ds_update_column).delete(ds_delete_column),
        )
        .route("/ws/{workspace}/datasets/{id}/data", post(ds_data))
        .route("/ws/{workspace}/datasets/{id}/usage", get(ds_usage))
        .route("/ws/{workspace}/datasets/{id}/folder", patch(ds_set_folder))
        .route("/ws/{workspace}/datasets/{id}/trash", post(ds_trash))
        .route("/ws/{workspace}/datasets/{id}/restore", post(ds_restore))
        .route(
            "/ws/{workspace}/datasets/{id}/bookmark",
            post(ds_bookmark_add).delete(ds_bookmark_remove),
        )
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/datasets/{id}/folder",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 200, description = "Dataset moved to folder", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission", body = ErrorBody),
        (status = 404, description = "No such dataset or target folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "dataset").await?;
    }
    st.datasets
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Introspect a source's columns and classify them. Physical → schema service; virtual → describe().
/// A dataset's columns are keyed by name, so a source that returns one name
/// twice (`select 1 as x, 2 as x`) is refused here, as a 400 that names the
/// column, rather than by the store's unique key as a 500 (KI-095).
fn ensure_unique_column_names(cols: &[(String, String)]) -> Result<(), crate::BiError> {
    let mut seen = std::collections::HashSet::new();
    match cols.iter().find(|(name, _)| !seen.insert(name.as_str())) {
        Some((name, _)) => Err(crate::BiError::BadRequest(format!(
            "the query returns more than one column named `{name}`; give each column its own name with AS"
        ))),
        None => Ok(()),
    }
}

async fn introspect(
    st: &BiState,
    spec: &ConnSpec,
    source: &SourceSpec,
) -> Result<Vec<NewColumn>, BiApiError> {
    let raw: Vec<(String, String)> = match source {
        SourceSpec::Physical {
            catalog,
            schema,
            table,
        } => st
            .schema
            .columns(spec, catalog.as_deref().unwrap_or(""), schema, table)
            .await
            .map_err(BiApiError::from)?
            .into_iter()
            .map(|c| (c.name, c.ty))
            .collect(),
        SourceSpec::Virtual { sql } => build_connector(spec)
            .describe(sql, Duration::from_secs(15))
            .await
            .map_err(|e| BiApiError::from(crate::BiError::from(e)))?
            .into_iter()
            .map(|c| (c.name, c.ty))
            .collect(),
    };
    ensure_unique_column_names(&raw)?;
    Ok(raw
        .into_iter()
        .map(|(name, ty)| {
            let g = generic_type(spec.engine, &ty);
            NewColumn {
                column_name: name,
                data_type: ty,
                type_generic: g.as_str().to_string(),
                is_temporal: g.is_temporal(),
            }
        })
        .collect())
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets",
    tag = "datasets",
    params(WorkspacePath),
    request_body = CreateDataset,
    responses(
        (status = 200, description = "Dataset created (columns introspected from the source)", body = DatasetDetailDto),
        (status = 400, description = "Invalid source or introspection failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission (or the database is not granted)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    // conn_spec_perm enforces DS_MANAGE (and that the db is granted) — single auth checkpoint.
    let spec = conn_spec_perm(&st, &ctx, b.database_id, DS_MANAGE).await?;
    let source = match b.source {
        CreateSource::Physical {
            catalog,
            schema,
            table,
        } => SourceSpec::Physical {
            catalog,
            schema,
            table,
        },
        // Stored without a trailing `;`, as SQL Lab runs it: the stored SQL is
        // later placed inside a subquery, where a `;` is a syntax error (KI-095).
        CreateSource::Virtual { sql } => SourceSpec::Virtual {
            sql: egret_query::execution::strip_trailing_semicolons(&sql, spec.engine).to_string(),
        },
    };
    let columns = introspect(&st, &spec, &source).await?;
    let ds = st
        .datasets
        .create(
            ctx.workspace_id.0,
            b.database_id,
            &b.name,
            &b.description,
            &source,
            &columns,
            Some(ctx.principal.user_id),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, ds.id).await?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/datasets",
    tag = "datasets",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List only trashed datasets when true"),
    ),
    responses(
        (status = 200, description = "Datasets in the workspace", body = Vec<DatasetSummaryDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    AxQuery(q): AxQuery<ListQuery>,
) -> Result<Json<Vec<DatasetSummaryDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, DS_READ)
        .await
        .map_err(BiApiError::from)?;
    let rows = st
        .datasets
        .list_by_workspace_for(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|(ds, n, bm)| crate::model::summary_dto_bm(ds, n, bm))
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/trash",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset moved to trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_trash(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .set_trashed(id, true)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/restore",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset restored from trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_restore(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .set_trashed(id, false)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/bookmark",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset bookmarked for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:read permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_bookmark_add(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .add_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/datasets/{id}/bookmark",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset bookmark removed for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:read permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_bookmark_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/datasets/{id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset detail (columns + metrics)", body = DatasetDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:read permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/datasets/{id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = UpdateDataset,
    responses(
        (status = 200, description = "Dataset updated", body = DatasetDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .update(id, &b.name, &b.description, b.sql.as_deref())
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/refresh",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Physical columns reconciled from the source; names the columns added and removed. A column that kept its engine type keeps its Time switch.", body = DatasetRefreshDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_refresh(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetRefreshDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_granted(&st, &ctx, ds.database_id).await?;
    let source = source_of(&ds);
    let new_columns = introspect(&st, &spec, &source).await?;
    let changes = st
        .datasets
        .reconcile_physical_columns(id, &new_columns)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(DatasetRefreshDto {
        dataset: load_detail(&st, id).await?,
        added_columns: changes.added,
        removed_columns: changes.removed,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/duplicate",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = DuplicateDataset,
    responses(
        (status = 200, description = "The copy: same source, every column setting and metric (not the certification)", body = DatasetDetailDto),
        (status = 400, description = "Empty name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "No dataset:read on the source, or no dataset:manage (or the database is not granted)", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 409, description = "A dataset with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<DuplicateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    // Reading the source is what the copy discloses; creating a dataset is what it does. Both
    // gates, the second the same one `ds_create` uses (dataset:manage + the database granted).
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let name = b.name.trim();
    if name.is_empty() {
        return Err(crate::BiError::BadRequest("name is required".into()).into());
    }
    let copy = st
        .datasets
        .duplicate(id, name, Some(ctx.principal.user_id))
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, copy.id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/datasets/{id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("confirm" = Option<String>, Query, description = "The dataset's exact name. Required while charts are built on it: with it, those charts are deleted too."),
    ),
    responses(
        (status = 200, description = "Dataset deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 409, description = "Charts are built on this dataset (live or in the trash) and `confirm` is not its name. Body: `{error, code: \"in_use\", charts: [{id, name, viz_type, trashed}]}`", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<ConfirmQuery>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // D8: the charts go with the dataset (`ON DELETE CASCADE`), so a delete that would take any
    // with it — trashed ones too — is refused with their list, unless the caller typed the name.
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let charts = st.datasets.charts_on(id).await.map_err(BiApiError::from)?;
    if !charts.is_empty() && q.confirm.as_deref() != Some(ds.name.as_str()) {
        let n = charts.len();
        return Err(crate::BiError::InUse {
            message: format!(
                "{n} {} built on this dataset would be deleted with it; confirm with the dataset name to delete {}",
                if n == 1 { "chart" } else { "charts" },
                if n == 1 { "both" } else { "them all" },
            ),
            charts,
        }
        .into());
    }
    st.datasets.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn load_detail(st: &BiState, id: Uuid) -> Result<DatasetDetailDto, BiApiError> {
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    let metrics = st.datasets.metrics(id).await.map_err(BiApiError::from)?;
    Ok(crate::model::detail_dto(ds, cols, metrics))
}

fn source_of(ds: &crate::dataset::Dataset) -> SourceSpec {
    match &ds.sql {
        Some(sql) => SourceSpec::Virtual { sql: sql.clone() },
        None => SourceSpec::Physical {
            catalog: ds.catalog.clone(),
            schema: ds.schema_name.clone().unwrap_or_default(),
            table: ds.table_name.clone().unwrap_or_default(),
        },
    }
}

/// Preview row-cap bounds for the object-scoped dataset data endpoint.
const PREVIEW_DEFAULT_LIMIT: i64 = 200;
const PREVIEW_MIN_LIMIT: i64 = 100;
const PREVIEW_MAX_LIMIT: i64 = 1000;

/// Body of the object-scoped dataset data-preview: an OPTIONAL row cap only.
/// There is deliberately NO client SQL field — the endpoint always runs
/// `SELECT * FROM <the dataset's own source>`, so a shared reader can preview
/// only the dataset its grant covers. Unknown fields are ignored.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DatasetDataBody {
    /// Rows to preview. Clamped to [100, 1000]; defaults to 200 when absent.
    pub limit: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/data",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = DatasetDataBody,
    responses(
        (status = 200, description = "A bounded preview of the dataset's OWN source (SELECT * … LIMIT n): columns + rows", body = ChartRunResponse),
        (status = 400, description = "Malformed request body", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "No dataset:read grant on this dataset", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 502, description = "Query engine error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Object-scoped dataset data-preview: runs ONLY a bounded
/// `SELECT * FROM <the dataset's own source> LIMIT n` (n from the optional body,
/// clamped to [100, 1000]). Accepts NO client SQL, so an object guest with a
/// `dataset:read` grant can preview only the shared dataset — no `query:run`
/// workspace permission required.
async fn ds_data(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    // OPTIONAL body, read as raw bytes so an empty POST is "use the default limit"
    // rather than a parse error. A present-but-malformed body → 400.
    raw_body: axum::body::Bytes,
) -> Result<Json<ChartRunResponse>, BiApiError> {
    // Object-scoped read gate: a non-granted dataset is denied here (403) BEFORE
    // anything is loaded or run.
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;

    let body: DatasetDataBody = if raw_body.is_empty() {
        DatasetDataBody { limit: None }
    } else {
        serde_json::from_slice(&raw_body)
            .map_err(|_| Error::InvalidInput("invalid request body".into()))?
    };
    let limit = body
        .limit
        .unwrap_or(PREVIEW_DEFAULT_LIMIT)
        .clamp(PREVIEW_MIN_LIMIT, PREVIEW_MAX_LIMIT);

    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let database = st
        .databases
        .get(ds.database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;

    // Build the FROM with the SAME helper the chart-data path uses
    // (`chartquery::build` → `source_ref_for`): physical sources are engine-quoted;
    // a virtual dataset's saved SQL is wrapped as a subquery. NEVER hand-built.
    let from = crate::expr::source_ref_for(database.engine, &source_of(&ds));
    let sql = format!("SELECT *\nFROM {from}\nLIMIT {limit}");

    let (session_user, bearer) = resolve_delegation(&st, &ctx, ds.database_id).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        ds.database_id,
        &sql,
        Some(limit as usize),
        session_user,
        bearer,
        &chart_run_limits(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;

    Ok(Json(ChartRunResponse {
        columns: result.columns,
        rows: result.rows,
    }))
}

/// Guard against cross-dataset/cross-workspace metric access: the metric must belong to
/// the dataset named in the path (the auth check is scoped to that dataset).
async fn require_metric_in_dataset(
    st: &BiState,
    dataset_id: Uuid,
    metric_id: Uuid,
) -> Result<(), BiApiError> {
    match st
        .datasets
        .metric_dataset(metric_id)
        .await
        .map_err(BiApiError::from)?
    {
        Some(d) if d == dataset_id => Ok(()),
        _ => Err(Error::NotFound("metric".into()).into()),
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/metrics",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = MetricBody,
    responses(
        (status = 200, description = "Metric added (expression validated)", body = DatasetDetailDto),
        (status = 400, description = "Invalid metric expression, or not an aggregate", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_add_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_granted(&st, &ctx, ds.database_id).await?;
    let d = validate_metric_expression(&spec, &source_of(&ds), &b.expression)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .add_metric(
            id,
            &b.metric_name,
            &b.expression,
            &d.data_type,
            &d.type_generic,
            &b.verbose_name,
            &b.description,
            &b.format,
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/datasets/{id}/metrics/{metric_id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("metric_id" = Uuid, Path, description = "Metric id"),
    ),
    request_body = MetricBody,
    responses(
        (status = 200, description = "Metric updated (expression validated). A rename rewrites the charts that use the metric in the same transaction.", body = DatasetDetailDto),
        (status = 400, description = "Invalid metric expression, or not an aggregate", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset, or metric not in this dataset", body = ErrorBody),
        (status = 409, description = "Name taken, or a rename of a used metric whose old or new name is also a column name (the charts could not be rewritten unambiguously). Body of the latter: `{error, code: \"in_use\", charts}`", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_update_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
    Json(b): Json<MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    require_metric_in_dataset(&st, id, metric_id).await?;
    let old_name = st
        .datasets
        .metrics(id)
        .await
        .map_err(BiApiError::from)?
        .into_iter()
        .find(|m| m.id == metric_id)
        .map(|m| m.metric_name)
        .ok_or(Error::NotFound("metric".into()))?;
    // KI-099: a rename used to leave every chart that named the metric failing with
    // `unknown metric: <old>`. Those charts are now rewritten with the metric, in one transaction.
    // Decided BEFORE the engine probe below, so a rename that is going to be refused does not
    // spend an engine round trip to find out.
    let rewrite_charts = if old_name != b.metric_name {
        let charts = st
            .datasets
            .charts_using_metric(id, &old_name)
            .await
            .map_err(BiApiError::from)?;
        if !charts.is_empty() {
            // The rewrite matches settings by value, so it is only safe while no column shares
            // either name: `cartesian.xAxis: "revenue"` could otherwise be the column.
            let columns = st.datasets.columns(id).await.map_err(BiApiError::from)?;
            if let Some(clash) = [old_name.as_str(), b.metric_name.as_str()]
                .into_iter()
                .find(|n| columns.iter().any(|c| c.column_name == *n))
            {
                return Err(crate::BiError::InUse {
                    message: format!(
                        "cannot rename a metric that charts use while a column is also named {clash:?}: the charts could not be rewritten unambiguously"
                    ),
                    charts,
                }
                .into());
            }
        }
        !charts.is_empty()
    } else {
        false
    };
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_granted(&st, &ctx, ds.database_id).await?;
    let d = validate_metric_expression(&spec, &source_of(&ds), &b.expression)
        .await
        .map_err(BiApiError::from)?;
    if rewrite_charts {
        st.datasets
            .rename_metric(
                id,
                metric_id,
                &old_name,
                &b.metric_name,
                &b.expression,
                &d.data_type,
                &d.type_generic,
                &b.verbose_name,
                &b.description,
                &b.format,
            )
            .await
            .map_err(BiApiError::from)?;
        return Ok(Json(load_detail(&st, id).await?));
    }
    st.datasets
        .update_metric(
            metric_id,
            &b.metric_name,
            &b.expression,
            &d.data_type,
            &d.type_generic,
            &b.verbose_name,
            &b.description,
            &b.format,
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

/// Shared by both certification handlers below: resolve the dataset inside the path workspace,
/// then demand the PLATFORM permission.
///
/// SUBJECT IS THE PRINCIPAL, NOT THE WORKSPACE CONTEXT — the same trap that made #461 wrong.
/// `WorkspaceContext` presents `Resource::Workspace`, and a platform-scoped grant does not
/// authorize against a workspace resource, so this would 403 for every data_steward, i.e. for
/// exactly the people the permission is seeded to. `Principal` presents `Resource::Platform`.
///
/// The object is resolved FIRST so a caller without the permission cannot use the 403/404
/// difference to learn whether a dataset id exists in a workspace they cannot see.
async fn require_dataset_certify(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let _obj = crate::shares::resolve(st, ctx, "dataset", id).await?;
    egret_core::authz::require(&*st.pdp, &ctx.principal, DS_CERTIFY)
        .await
        .map_err(BiApiError::from)
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/datasets/{id}/certification",
    tag = "datasets",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Dataset id")),
    request_body = SetDatasetCertification,
    responses(
        (status = 200, description = "Certification set", body = DatasetDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:certify permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_set_certification(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<SetDatasetCertification>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_certify(&st, &ctx, id).await?;
    // The certifier's email, from the SESSION. The body cannot name anyone — that is the whole
    // point of 0109, and the difference from the update path this replaces.
    let who = ctx.principal.email.clone();
    st.datasets
        .set_certification(id, b.certified, &who, &b.details)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/datasets/{id}/metrics/{metric_id}/certification",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("metric_id" = Uuid, Path, description = "Metric id"),
    ),
    request_body = SetDatasetCertification,
    responses(
        (status = 200, description = "Certification set", body = DatasetDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:certify permission", body = ErrorBody),
        (status = 404, description = "No such dataset or metric", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_set_metric_certification(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
    Json(b): Json<SetDatasetCertification>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_certify(&st, &ctx, id).await?;
    // Without this the metric_id is unconstrained and a caller could certify a metric belonging
    // to a dataset in ANOTHER workspace, since the permission is platform-wide.
    require_metric_in_dataset(&st, id, metric_id).await?;
    let who = ctx.principal.email.clone();
    st.datasets
        .set_metric_certification(metric_id, b.certified, &who, &b.details)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/datasets/{id}/metrics/{metric_id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("metric_id" = Uuid, Path, description = "Metric id"),
        ("confirm" = Option<String>, Query, description = "The metric's exact name. Required while charts use it: with it, the metric is deleted and those charts stop running."),
    ),
    responses(
        (status = 200, description = "Metric deleted", body = DatasetDetailDto),
        (status = 409, description = "Charts use this metric and `confirm` is not its name. Body: `{error, code: \"in_use\", charts}`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset, or metric not in this dataset", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_delete_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
    AxQuery(q): AxQuery<ConfirmQuery>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    require_metric_in_dataset(&st, id, metric_id).await?;
    let name = st
        .datasets
        .metrics(id)
        .await
        .map_err(BiApiError::from)?
        .into_iter()
        .find(|m| m.id == metric_id)
        .map(|m| m.metric_name)
        .ok_or(Error::NotFound("metric".into()))?;
    let charts = st
        .datasets
        .charts_using_metric(id, &name)
        .await
        .map_err(BiApiError::from)?;
    if !charts.is_empty() && q.confirm.as_deref() != Some(name.as_str()) {
        let n = charts.len();
        return Err(crate::BiError::InUse {
            message: format!(
                "{n} {} this metric and would stop running; confirm with the metric name to delete it",
                if n == 1 { "chart uses" } else { "charts use" },
            ),
            charts,
        }
        .into());
    }
    st.datasets
        .delete_metric(metric_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/datasets/{id}/columns",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    request_body = AddColumnBody,
    responses(
        (status = 200, description = "Calculated column added (expression validated)", body = DatasetDetailDto),
        (status = 400, description = "Empty column name, or invalid column expression", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_add_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<AddColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // A column is addressed by its name in the path (`/columns/{column_name}`), so an empty name
    // makes a column no request can reach again: not editable, not deletable.
    if b.column_name.trim().is_empty() {
        return Err(crate::BiError::BadRequest("column name is required".into()).into());
    }
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_granted(&st, &ctx, ds.database_id).await?;
    let d = validate_expression(&spec, &source_of(&ds), &b.expression)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .add_calc_column(
            id,
            &b.column_name,
            &b.expression,
            &d.data_type,
            &d.type_generic,
            d.is_temporal,
            &b.verbose_name,
            &b.description,
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/datasets/{id}/columns/{column_name}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("column_name" = String, Path, description = "Column name"),
    ),
    request_body = UpdateColumnBody,
    responses(
        (status = 200, description = "Column updated", body = DatasetDetailDto),
        (status = 400, description = "Cannot set an expression on a physical column, or invalid expression", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset or column in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_update_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>,
    Json(b): Json<UpdateColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // THE ONLY HTTP WRITER OF `semantic_type`, which is why one check here is enough to close it.
    // Every other path into the column takes the DB default `''`: the three INSERTs in `dataset.rs`
    // (source sync, calculated column, dataset creation) never name the field, and the sync's
    // `ON CONFLICT … DO UPDATE` deliberately leaves it alone so a refresh preserves an author's
    // annotation. Until now nothing validated it anywhere — `PUT semantic_type: "banana"` persisted.
    //
    // AFTER AUTHZ, ON PURPOSE. A caller who may not manage the dataset should learn that first;
    // being told which semantic types are valid is harmless, but ordering the checks so that
    // authorisation always answers before the body is inspected is the property worth keeping
    // uniform across handlers.
    //
    // BEFORE the expression validation below rather than after, because that branch makes a round
    // trip to the engine to type-check an expression. Rejecting a bad `semantic_type` first means a
    // request that was going to fail anyway does not spend an engine query to find out.
    if let Some(stype) = b.semantic_type.as_deref() {
        egret_analytics_api::bi::validate_semantic_type(stype)
            .map_err(|m| BiApiError::from(crate::BiError::BadRequest(m)))?;
    }
    let expr_update = if let Some(expr) = b.expression.as_deref() {
        match st
            .datasets
            .column_is_calculated(id, &column_name)
            .await
            .map_err(BiApiError::from)?
        {
            Some(true) => {
                let ds = st
                    .datasets
                    .get(id)
                    .await
                    .map_err(BiApiError::from)?
                    .ok_or(Error::NotFound("dataset".into()))?;
                let spec = conn_spec_granted(&st, &ctx, ds.database_id).await?;
                let d = validate_expression(&spec, &source_of(&ds), expr)
                    .await
                    .map_err(BiApiError::from)?;
                Some((expr.to_string(), d.data_type, d.type_generic, d.is_temporal))
            }
            Some(false) => {
                return Err(crate::BiError::BadRequest(
                    "cannot set an expression on a physical column".into(),
                )
                .into())
            }
            None => return Err(Error::NotFound("column".into()).into()),
        }
    } else {
        None
    };
    let eu = expr_update
        .as_ref()
        .map(|(e, dt, tg, t)| (e.as_str(), dt.as_str(), tg.as_str(), *t));
    st.datasets
        .update_column(
            id,
            &column_name,
            b.verbose_name.as_deref(),
            b.description.as_deref(),
            b.groupby,
            b.filterable,
            b.is_temporal,
            b.format.as_deref(),
            b.semantic_type.as_deref(),
            eu,
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/datasets/{id}/usage",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("include_trashed" = Option<bool>, Query, description = "Also list charts in the trash: every chart a delete of the dataset would delete with it"),
    ),
    responses(
        (status = 200, description = "Charts that use this dataset, live ones first", body = Vec<crate::model::DatasetUsageDto>),
        (status = 403, description = "Missing dataset:read permission", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
    ),
)]
async fn ds_usage(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    AxQuery(q): AxQuery<UsageQuery>,
) -> Result<Json<Vec<crate::model::DatasetUsageDto>>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let items = if q.include_trashed {
        st.datasets.charts_on(id).await
    } else {
        st.datasets.usage(id).await
    }
    .map_err(BiApiError::from)?;
    Ok(Json(
        items
            .into_iter()
            .map(|u| crate::model::DatasetUsageDto {
                id: u.id.to_string(),
                name: u.name,
                viz_type: u.viz_type,
                trashed: u.trashed,
            })
            .collect(),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/datasets/{id}/columns/{column_name}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
        ("column_name" = String, Path, description = "Calculated column name"),
    ),
    responses(
        (status = 200, description = "Calculated column deleted", body = DatasetDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_delete_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.datasets
        .delete_calc_column(id, &column_name)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

// ── Chart routes ─────────────────────────────────────────────────────────────

pub fn chart_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/charts", post(ch_create).get(ch_list))
        .route("/ws/{workspace}/charts/data", post(ch_data))
        .route("/ws/{workspace}/charts/run", post(ch_run_spec))
        .route("/ws/{workspace}/charts/{id}/data", post(ch_run))
        .route(
            "/ws/{workspace}/charts/{id}",
            get(ch_get).put(ch_update).delete(ch_remove),
        )
        .route("/ws/{workspace}/charts/{id}/folder", patch(ch_set_folder))
        .route("/ws/{workspace}/charts/{id}/duplicate", post(ch_duplicate))
        .route("/ws/{workspace}/charts/{id}/trash", post(ch_trash))
        .route("/ws/{workspace}/charts/{id}/restore", post(ch_restore))
        .route(
            "/ws/{workspace}/charts/{id}/bookmark",
            post(ch_bookmark_add).delete(ch_bookmark_remove),
        )
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/charts/{id}/folder",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 200, description = "Chart moved to folder", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart or target folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "chart").await?;
    }
    st.charts
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/{id}/duplicate",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id to duplicate"),
    ),
    responses(
        (status = 200, description = "Chart duplicated", body = ChartDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    // Duplicating CREATES new workspace content, so it requires CH_MANAGE on the
    // source (matching ch_create) — a read-only principal must not create rows.
    // The source is still readable by any manager.
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    let copy = st
        .charts
        .duplicate(id, ctx.workspace_id.0, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(copy)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/charts",
    tag = "charts",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List only trashed charts when true"),
    ),
    responses(
        (status = 200, description = "Charts in the workspace", body = Vec<ChartDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    AxQuery(q): AxQuery<ListQuery>,
) -> Result<Json<Vec<crate::model::ChartDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;
    let rows = st
        .charts
        .list_by_workspace_for(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|(c, bm)| crate::model::chart_dto_bm(c, bm))
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts",
    tag = "charts",
    params(WorkspacePath),
    request_body = CreateChart,
    responses(
        (status = 200, description = "Chart created", body = ChartDto),
        (status = 400, description = "Malformed params (see the message)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "Referenced dataset not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(b.dataset_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    // Reject a params body the frontend could not open, rather than storing it and
    // failing later in whoever's browser opens the chart.
    crate::chartparams::validate(&b.viz_type, &b.params).map_err(BiApiError::from)?;
    let c = st
        .charts
        .create(
            ctx.workspace_id.0,
            b.dataset_id,
            &b.name,
            &b.description,
            &b.viz_type,
            &b.params,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(crate::model::chart_dto(c)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/charts/{id}",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart detail", body = ChartDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let c = st
        .charts
        .get(id, ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(c)))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/charts/{id}",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    request_body = UpdateChart,
    responses(
        (status = 200, description = "Chart updated", body = ChartDto),
        (status = 400, description = "Malformed params (see the message)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart, or referenced dataset not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // The (possibly changed) dataset must exist in this workspace — same cross-tenant guard as create.
    let ds = st
        .datasets
        .get(b.dataset_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    crate::chartparams::validate(&b.viz_type, &b.params).map_err(BiApiError::from)?;
    st.charts
        .update(
            id,
            b.dataset_id,
            &b.name,
            &b.description,
            &b.viz_type,
            &b.params,
        )
        .await
        .map_err(BiApiError::from)?;
    let c = st
        .charts
        .get(id, ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(c)))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/charts/{id}",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.charts.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/data",
    tag = "charts",
    params(WorkspacePath),
    request_body = ChartDataBody,
    responses(
        (status = 200, description = "Compiled SQL + target database id (the client submits it to the query seam)", body = ChartDataResponse),
        (status = 400, description = "Invalid query spec (unknown column/metric, bad filter, …)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission", body = ErrorBody),
        (status = 404, description = "Referenced dataset not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_data(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<ChartDataBody>,
) -> Result<Json<ChartDataResponse>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;
    let (sql, database_id) = compile_chart_sql(
        &st.datasets,
        &st.databases,
        ctx.workspace_id.0,
        b.dataset_id,
        &b.query_spec,
    )
    .await?;
    Ok(Json(ChartDataResponse {
        sql,
        database_id: database_id.to_string(),
    }))
}

/// Compile a dataset's `QuerySpec` to `(sql, database_id)`, scoped to
/// `workspace_id` (cross-tenant guard → `NotFound`). Shared by `ch_data`
/// (client-supplied spec), `ch_run`/`ch_run_spec` (the chart's SAVED spec), and
/// — the reason this takes repos rather than `&BiState` — `egret-automation`'s
/// render-payload endpoint, which compiles a chart with no `WorkspaceContext` at
/// all (a render token authenticates it, not a session) but does know the
/// object's workspace from the grant it redeemed. `pub` so that crate can call
/// it directly: two code paths that compile a chart differently is how a
/// report silently stops matching the dashboard it claims to show.
pub async fn compile_chart_sql(
    datasets: &crate::dataset::DatasetRepo,
    databases: &DatabaseRepo,
    workspace_id: Uuid,
    dataset_id: Uuid,
    spec: &chartquery::QuerySpec,
) -> Result<(String, Uuid), BiApiError> {
    let ds = datasets
        .get(dataset_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != workspace_id {
        return Err(Error::NotFound("dataset".into()).into());
    }
    let database = databases
        .get(ds.database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let cols = datasets
        .columns(dataset_id)
        .await
        .map_err(BiApiError::from)?;
    let metrics = datasets
        .metrics(dataset_id)
        .await
        .map_err(BiApiError::from)?;
    let source = source_of(&ds);
    let sql = chartquery::build(database.engine, &source, &cols, &metrics, spec)
        .map_err(BiApiError::from)?;
    Ok((sql, ds.database_id))
}

/// Execution guards for a chart-data run: cap at the chart row ceiling with a
/// modest timeout. (`Limits` has no `Default`; mirror `SchemaService::limits`.)
fn chart_run_limits() -> egret_query::execution::model::Limits {
    egret_query::execution::model::Limits {
        max_rows: crate::chartquery::MAX_CHART_ROWS as usize,
        timeout: Duration::from_secs(30),
    }
}

/// For a target database, resolve the `(session_user, bearer_token)` to run a BI
/// query with. Non-delegated connections keep the current behavior (caller email
/// as session_user, no bearer). Delegated connections mint the caller's per-user
/// engine JWT and use the caller's username (= the JWT `preferred_username`, which
/// the engine matches against the bearer principal); a non-OIDC caller is denied.
pub async fn resolve_delegation(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    database_id: Uuid,
) -> Result<(Option<String>, Option<String>), BiApiError> {
    let db = st
        .databases
        .get(database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    if !db.delegated {
        return Ok((Some(ctx.principal.email.clone()), None));
    }
    let token = st
        .engine_minter
        .mint(ctx.principal.user_id.0)
        .await
        .map_err(|e| match e {
            egret_analytics_api::engine_token::BrokerError::NoRefreshToken => {
                BiApiError::from(egret_core::Error::Forbidden)
            }
            other => BiApiError::from(egret_core::Error::Internal(anyhow::anyhow!(
                other.to_string()
            ))),
        })?;
    let username = st
        .users
        .ensure_username(ctx.principal.user_id)
        .await
        .map_err(BiApiError::from)?;
    Ok((Some(username), Some(token.access_token)))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/{id}/data",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "The chart's SAVED query, executed: columns + rows", body = ChartRunResponse),
        (status = 400, description = "Chart has no runnable query, or its saved spec is invalid", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "No chart:read grant on this chart", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace, or its dataset is gone", body = ErrorBody),
        (status = 502, description = "Query engine error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Object-scoped: runs ONLY the chart's persisted query (no client-supplied
/// spec — the handler takes only the path id), so an object guest with a
/// `chart:read` grant can view the shared chart's data without any query:run
/// workspace permission.
async fn ch_run(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<ChartRunResponse>, BiApiError> {
    // Object-scoped read gate: object guests reach execution; a non-granted
    // chart is denied here (403) BEFORE anything is loaded or run.
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_READ, obj)
        .await
        .map_err(BiApiError::from)?;

    let chart = st
        .charts
        .get(id, ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    // Extract ONLY the chart's saved query spec (params.spec). The endpoint
    // accepts no request body, so a guest can run nothing but this.
    let spec: chartquery::QuerySpec = serde_json::from_value(
        chart
            .params
            .get("spec")
            .cloned()
            .ok_or_else(|| Error::InvalidInput("chart has no runnable query".into()))?,
    )
    .map_err(|_| Error::InvalidInput("chart spec invalid".into()))?;

    let (sql, database_id) = compile_chart_sql(
        &st.datasets,
        &st.databases,
        ctx.workspace_id.0,
        chart.dataset_id,
        &spec,
    )
    .await?;
    let (session_user, bearer) = resolve_delegation(&st, &ctx, database_id).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(spec.row_limit.max(0) as usize),
        session_user,
        bearer,
        &chart_run_limits(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;

    Ok(Json(ChartRunResponse {
        columns: result.columns,
        rows: result.rows,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/run",
    tag = "charts",
    params(WorkspacePath),
    request_body = ChartDataBody,
    responses(
        (status = 200, description = "The client-supplied spec, compiled + executed: columns + rows", body = ChartRunResponse),
        (status = 400, description = "Invalid query spec (unknown column/metric, bad filter, …)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission, or the charts module is disabled", body = ErrorBody),
        (status = 404, description = "Referenced dataset not in this workspace", body = ErrorBody),
        (status = 502, description = "Query engine error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Run a client-supplied `QuerySpec` against a dataset and return the rows.
///
/// This is `ch_run` for a spec that is not (yet) a saved chart: the chart
/// builder's preview, dashboard edit-mode cards, and filter-value dropdowns. It
/// exists so those three do not have to go through the free-form SQL endpoint —
/// which lets `POST /queries` be gated on the SQL Lab entitlement without
/// breaking chart authoring, and stops raw SQL crossing the wire when a user is
/// only building a chart.
///
/// Authorization is workspace-scoped `CH_READ`, exactly like the `ch_data`
/// build-only endpoint it replaces. It is deliberately NOT object-scoped: there
/// is no chart id to resolve, so an object guest cannot reach it — guests keep
/// using `ch_run` (`/charts/{id}/data`), which is object-scoped.
async fn ch_run_spec(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<ChartDataBody>,
) -> Result<Json<ChartRunResponse>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;

    let (sql, database_id) = compile_chart_sql(
        &st.datasets,
        &st.databases,
        ctx.workspace_id.0,
        b.dataset_id,
        &b.query_spec,
    )
    .await?;
    let (session_user, bearer) = resolve_delegation(&st, &ctx, database_id).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(b.query_spec.row_limit.max(0) as usize),
        session_user,
        bearer,
        &chart_run_limits(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;

    Ok(Json(ChartRunResponse {
        columns: result.columns,
        rows: result.rows,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/{id}/trash",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart moved to trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_trash(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.charts
        .set_trashed(id, true)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/{id}/restore",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart restored from trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:manage permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_restore(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.charts
        .set_trashed(id, false)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/charts/{id}/bookmark",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart bookmarked for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_bookmark_add(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.charts
        .add_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/charts/{id}/bookmark",
    tag = "charts",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Chart id"),
    ),
    responses(
        (status = 200, description = "Chart bookmark removed for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing chart:read permission", body = ErrorBody),
        (status = 404, description = "No such chart in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ch_bookmark_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, CH_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.charts
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Dashboard routes ─────────────────────────────────────────────────────────

pub fn dashboard_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/dashboards", post(db_create).get(db_list))
        .route(
            "/ws/{workspace}/dashboards/{id}",
            get(db_get).put(db_update).delete(db_remove),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/duplicate",
            post(db_duplicate),
        )
        .route("/ws/{workspace}/dashboards/{id}/trash", post(db_trash))
        .route("/ws/{workspace}/dashboards/{id}/restore", post(db_restore))
        .route(
            "/ws/{workspace}/dashboards/{id}/bookmark",
            post(db_bookmark_add).delete(db_bookmark_remove),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/folder",
            patch(db_set_folder),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/cards/{card_id}/data",
            post(db_card_run),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/revisions",
            get(db_revisions),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/revisions/{revision_id}/revert",
            post(db_revision_revert),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/certification",
            put(db_set_certification),
        )
}

/// Optional body for the card-data endpoint: resolved dashboard-parameter filters
/// to merge into the card chart's persisted query. Reuses `chartquery::Filter`
/// (the same `{ col, op, value }` shape the chart spec uses), so the frontend
/// sends exactly what `applyParamsToChart` produces. Each filter's `col` is
/// validated against the dataset's real columns before it is applied (an unknown
/// column → 400), and the values/operators are rendered through the same
/// injection-safe `sql_literal`/`FilterOp` path as the persisted filters.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct CardRunBody {
    /// Resolved, dashboard-param-derived filters to AND onto the card's saved query.
    #[serde(default)]
    pub filters: Vec<chartquery::Filter>,
    /// Resolved `temporal_unit` dashboard-param overrides: dimension column name ->
    /// temporal unit token (e.g. "month"). Each token is validated against the
    /// `TemporalUnit` enum by `chartquery::build` (an unknown token → 400) and rendered
    /// as an enum-driven bucket expression — never spliced into SQL as a raw string.
    /// A client override replaces the saved unit for that already-gridded column.
    #[serde(default)]
    pub temporal_units: std::collections::HashMap<String, String>,
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/cards/{card_id}/data",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
        ("card_id" = Uuid, Path, description = "Card id (must belong to the dashboard)"),
    ),
    request_body = CardRunBody,
    responses(
        (status = 200, description = "The card's chart SAVED query (with dashboard filters), executed: columns + rows", body = ChartRunResponse),
        (status = 400, description = "Card has no chart, its saved spec is invalid, or a filter references an unknown column", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "No dashboard:read grant on this dashboard", body = ErrorBody),
        (status = 404, description = "No such dashboard, or the card is not on this dashboard", body = ErrorBody),
        (status = 502, description = "Query engine error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Object-scoped on the DASHBOARD: a `dashboard:read` object grant TRANSITIVELY
/// authorizes running the data of any card on that dashboard — NO per-chart
/// grant is needed. The card must belong to the path dashboard (else 404). Runs
/// the card chart's persisted query, optionally merged with resolved
/// dashboard-parameter filters from the request body, so a shared viewer can
/// view the card's (filtered) data without any `query:run` permission.
///
/// The body is OPTIONAL (a body-less call runs the saved query unfiltered).
/// Incoming filters are appended to the persisted `spec.filters` and validated by
/// `compile_chart_sql` → `chartquery::build`, which resolves every filter `col`
/// against the dataset's real columns (an unknown column → 400) and renders the
/// values/operators through the injection-safe `sql_literal`/`FilterOp` path. The
/// client cannot inject a predicate on an arbitrary column.
async fn db_card_run(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, card_id)): Path<(String, Uuid, Uuid)>,
    // OPTIONAL body, read as raw bytes so an empty POST (with or without a JSON
    // content-type) is treated as "no filters" rather than a parse error. A
    // present-but-malformed body → 400.
    raw_body: axum::body::Bytes,
) -> Result<Json<ChartRunResponse>, BiApiError> {
    let body: CardRunBody = if raw_body.is_empty() {
        CardRunBody::default()
    } else {
        serde_json::from_slice(&raw_body)
            .map_err(|_| Error::InvalidInput("invalid request body".into()))?
    };
    // Object-scoped read gate on the DASHBOARD. A non-granted dashboard is denied
    // here (403) BEFORE any card lookup or execution.
    let dobj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, dobj)
        .await
        .map_err(BiApiError::from)?;

    // The card must belong to THIS dashboard (a foreign / random card_id → 404).
    // The row's chart_id may be NULL for a virtual card (heading/text/link).
    let chart_id: Option<Uuid> = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT chart_id FROM analytics.dashboard_cards WHERE id = $1 AND dashboard_id = $2",
    )
    .bind(card_id)
    .bind(id)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::from(e)))?
    .ok_or(Error::NotFound("card".into()))?;
    let chart_id = chart_id.ok_or_else(|| Error::InvalidInput("card has no chart".into()))?;

    // Load the card's chart and run ONLY its saved query. NOTE: there is
    // deliberately NO per-chart `require_object` — the dashboard grant above is
    // the authority (transitive auth: dashboard grant → card data).
    let chart = st
        .charts
        .get(chart_id, ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    let mut spec: chartquery::QuerySpec = serde_json::from_value(
        chart
            .params
            .get("spec")
            .cloned()
            .ok_or_else(|| Error::InvalidInput("chart has no runnable query".into()))?,
    )
    .map_err(|_| Error::InvalidInput("chart spec invalid".into()))?;

    // Merge resolved dashboard-parameter filters onto the persisted query. Each
    // filter's `col` is validated against the dataset's real columns by
    // `compile_chart_sql` → `chartquery::build` (`find_column`); an unknown column
    // is rejected (BadRequest → 400) BEFORE any query runs, so a client cannot
    // inject a predicate on an arbitrary column. Values/operators are
    // injection-safe via `sql_literal`/`FilterOp`.
    spec.filters.extend(body.filters);

    // Merge resolved `temporal_unit` dashboard-param overrides onto the persisted
    // query: a client override REPLACES the saved unit for that column (re-grid). The
    // unit token is validated against the `TemporalUnit` enum by `chartquery::build`
    // (an unknown token → 400) and a unit on a non-temporal / non-selected column is
    // also rejected there, so a client cannot smuggle raw SQL through the unit value.
    spec.temporal_units.extend(body.temporal_units);

    let (sql, database_id) = compile_chart_sql(
        &st.datasets,
        &st.databases,
        ctx.workspace_id.0,
        chart.dataset_id,
        &spec,
    )
    .await?;
    let (session_user, bearer) = resolve_delegation(&st, &ctx, database_id).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(spec.row_limit.max(0) as usize),
        session_user,
        bearer,
        &chart_run_limits(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;

    Ok(Json(ChartRunResponse {
        columns: result.columns,
        rows: result.rows,
    }))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/dashboards/{id}/folder",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 200, description = "Dashboard moved to folder", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard or target folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "dashboard").await?;
    }
    st.dashboards
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Every chart referenced by a card must live in the same workspace as the dashboard,
/// else embedding it is a cross-workspace leak. Returns NotFound on any foreign chart.
async fn require_cards_in_ws(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    cards: &[crate::model::CardInput],
) -> Result<(), BiApiError> {
    for c in cards {
        if let Some(chart_id) = c.chart_id {
            match st
                .charts
                .chart_workspace(chart_id)
                .await
                .map_err(BiApiError::from)?
            {
                Some(w) if w == ctx.workspace_id.0 => {}
                _ => return Err(Error::NotFound("chart".into()).into()),
            }
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct UsageQuery {
    #[serde(default)]
    include_trashed: bool,
}

#[derive(serde::Deserialize)]
struct ListQuery {
    #[serde(default)]
    trashed: bool,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/dashboards",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List only trashed dashboards when true"),
    ),
    responses(
        (status = 200, description = "Dashboards in the workspace", body = Vec<DashboardSummaryDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    AxQuery(q): AxQuery<ListQuery>,
) -> Result<Json<Vec<DashboardSummaryDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, DB_READ)
        .await
        .map_err(BiApiError::from)?;
    let rows = st
        .dashboards
        .list_by_workspace_for(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await
        .map_err(BiApiError::from)?;
    // ONE decision for the whole list, not one per row — see the field docs on
    // `DashboardSummaryDto::can_manage` for why per-object here would cost 2 queries per row.
    // This is the SAME check `db_create` makes, so the row menu and the "+ Dashboard" button
    // agree with each other and with what the mutating routes will accept at workspace level.
    let can_manage = egret_core::authz::require(&*st.pdp, &ctx, DB_MANAGE)
        .await
        .is_ok();
    Ok(Json(
        rows.into_iter()
            .map(|(d, n, bm)| crate::model::dashboard_summary_dto(d, n, bm, can_manage))
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards",
    tag = "dashboards",
    params(WorkspacePath),
    request_body = CreateDashboard,
    responses(
        (status = 200, description = "Dashboard created (empty layout)", body = DashboardDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    let d = st
        .dashboards
        .create(
            ctx.workspace_id.0,
            &b.name,
            &b.description,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_dashboard(&st, d.id, &ctx).await?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/dashboards/{id}",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard detail (tabs + cards)", body = DashboardDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_dashboard(&st, id, &ctx).await?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/dashboards/{id}/certification",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    request_body = SetDashboardCertification,
    responses(
        (status = 200, description = "Updated dashboard", body = DashboardDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:certify permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_set_certification(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<SetDashboardCertification>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    // SUBJECT IS THE PRINCIPAL, NOT THE WORKSPACE CONTEXT. `WorkspaceContext` presents
    // `Resource::Workspace`, and a PLATFORM-scoped grant does not authorize against a workspace
    // resource — the request would 403 for everyone except platform_admin, i.e. for exactly the
    // stewards this perm is seeded to. `Principal` presents `Resource::Platform`, which is where
    // the grant lives. Pinned by dashboard_certification_routes.rs.
    // `require`, not `require_object`: `dashboard:certify` is PLATFORM-scoped, so holding
    // `dashboard:manage` on this dashboard — or owning it — grants nothing here. That asymmetry
    // is the feature: the badge means a steward vouched for it, not that its author did.
    //
    // The object is still resolved first, so a caller without the perm cannot use the 403/404
    // difference to learn whether a dashboard id exists in a workspace they cannot see.
    let _obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require(&*st.pdp, &ctx.principal, DB_CERTIFY)
        .await
        .map_err(BiApiError::from)?;

    // The certifier's email, not their uuid: this string is READ by whoever sees the badge, and
    // a uuid tells them nothing. No join needed, unlike created_by_name.
    let who = ctx.principal.email.clone();
    st.dashboards
        .set_certification(id, b.certified, &who, &b.details)
        .await
        .map_err(BiApiError::from)?;

    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        if b.certified {
            "dashboard:certify"
        } else {
            "dashboard:uncertify"
        },
        "dashboard",
        Some(id.to_string()),
        serde_json::json!({ "certified": b.certified, "details": b.details }),
    ));

    Ok(Json(load_dashboard(&st, id, &ctx).await?))
}

/// Best-effort revision write. A failure here must NOT fail the request: the dashboard has already
/// been updated, so a 500 would tell the client the save was lost when it was not. Same stance as
/// the audit sink — log and continue.
async fn capture_revision(
    st: &BiState,
    id: Uuid,
    actor: Option<Uuid>,
    kind: &str,
    snapshot: &serde_json::Value,
    at: Option<chrono::DateTime<chrono::Utc>>,
) {
    if let Err(e) = st
        .dashboards
        .insert_revision(id, actor, kind, snapshot, at)
        .await
    {
        tracing::warn!(dashboard = %id, kind, error = %e, "failed to record dashboard revision");
    }
}

/// The state a dashboard is about to leave, for dashboards that have no history yet.
///
/// READS ONLY. The value is held in memory and inserted by `apply_dashboard_update` after the
/// write lands: snapshotting afterwards would capture the NEW state, and inserting beforehand
/// would leave a restore point for an edit that then got rejected. Without this, the first edit
/// after this feature ships is the one edit that cannot be undone — which is the edit a user is
/// most likely to want back.
///
/// Attributed to whoever last touched the dashboard and stamped with that moment, not with now(),
/// because that is when the state being captured was actually written.
async fn pending_baseline(
    st: &BiState,
    id: Uuid,
    workspace_id: Uuid,
) -> Option<(
    serde_json::Value,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
)> {
    if st.dashboards.revision_count(id).await.ok()? != 0 {
        return None;
    }
    let d = st.dashboards.get(id, workspace_id).await.ok()??;
    let snap = st
        .dashboards
        .snapshot_current(id, workspace_id)
        .await
        .ok()??;
    Some((snap, d.updated_by.or(d.created_by), d.updated_at))
}

/// The one write path a dashboard layout takes. `db_update` and `db_revision_revert` both go
/// through it, so a revert can never take a different validation path than the save it undoes.
///
/// Authorization is the CALLER's job — the two handlers require different permissions on different
/// objects and both resolve the share before calling here.
async fn apply_dashboard_update(
    st: &BiState,
    ctx: &egret_core::context::WorkspaceContext,
    id: Uuid,
    b: UpdateDashboard,
    kind: &str,
) -> Result<DashboardDetailDto, BiApiError> {
    require_cards_in_ws(st, ctx, &b.cards).await?;
    crate::model::validate_layout(&b.tabs, &b.cards).map_err(BiApiError::from)?;
    if b.width != "fixed" && b.width != "full" {
        return Err(
            egret_core::Error::InvalidInput("width must be 'fixed' or 'full'".into()).into(),
        );
    }
    // Held, not written — see pending_baseline.
    let baseline = pending_baseline(st, id, ctx.workspace_id.0).await;
    // `UpdateDashboard` is a plain struct of owned values, so this cannot fail; on the impossible
    // branch we keep the save working and lose one history entry rather than 500-ing.
    let snapshot = serde_json::to_value(&b).ok();

    st.dashboards
        .update(
            id,
            &b.name,
            &b.description,
            &b.parameters,
            &b.width,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
    let tabs: Vec<crate::dashboard::NewTab> = b.tabs.into_iter().map(Into::into).collect();
    let cards: Vec<crate::dashboard::NewCard> = b.cards.into_iter().map(Into::into).collect();
    st.dashboards
        .replace_layout(id, &tabs, &cards)
        .await
        .map_err(BiApiError::from)?;

    // EVERYTHING THAT CAN REJECT THIS REQUEST HAS NOW RUN. Capturing any earlier would record an
    // edit that never happened — a 403 from the caller's authz, a 404 from require_cards_in_ws, a
    // 400 from validate_layout or the width check, or a failed replace_layout. From here the only
    // failure is a crash, which loses one entry: under-recording, the safe direction.
    if let Some((snap, actor, at)) = baseline {
        capture_revision(st, id, actor, "baseline", &snap, Some(at)).await;
    }
    if let Some(snap) = snapshot {
        capture_revision(st, id, Some(ctx.principal.user_id.0), kind, &snap, None).await;
    }
    load_dashboard(st, id, ctx).await
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/dashboards/{id}",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    request_body = UpdateDashboard,
    responses(
        (status = 200, description = "Dashboard metadata + layout replaced", body = DashboardDetailDto),
        (status = 400, description = "Invalid layout (tab/card mismatch) or width not 'fixed'/'full'", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard, or a referenced chart not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        apply_dashboard_update(&st, &ctx, id, b, "edit").await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/dashboards/{id}/revisions",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Revision history, newest first (at most 20 — see migration 0107)", body = Vec<DashboardRevisionDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Reading history needs only `dashboard:read` — it discloses who edited the dashboard and when,
/// which anyone who can read the dashboard can already see in the info sidebar. Reverting needs
/// `dashboard:manage`, because it writes.
async fn db_revisions(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<crate::model::DashboardRevisionDto>>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let revs = st
        .dashboards
        .list_revisions(id, 20)
        .await
        .map_err(BiApiError::from)?;
    // One users lookup for the whole page rather than one per row: at most twenty rows and
    // usually one or two distinct authors.
    let mut names: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
    for r in &revs {
        if let Some(u) = r.created_by {
            if let std::collections::hash_map::Entry::Vacant(e) = names.entry(u) {
                if let Ok(Some(user)) = st.users.find_by_id(egret_core::UserId(u)).await {
                    let label = if user.name.trim().is_empty() {
                        user.email.clone()
                    } else {
                        user.name.clone()
                    };
                    e.insert(label);
                }
            }
        }
    }
    Ok(Json(
        revs.into_iter()
            .enumerate()
            .map(|(i, r)| {
                let name = r.created_by.and_then(|u| names.get(&u).cloned());
                crate::model::dashboard_revision_dto(r, name, i == 0)
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/revisions/{revision_id}/revert",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
        ("revision_id" = Uuid, Path, description = "Revision to restore (must belong to this dashboard)"),
    ),
    responses(
        (status = 200, description = "Dashboard restored to the revision; the restored state is itself recorded as a new revision", body = DashboardDetailDto),
        (status = 400, description = "The stored snapshot is not a valid dashboard layout", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard or revision, or a chart the revision references no longer exists in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Restore an earlier revision by replaying it through the ordinary update path.
///
/// The snapshot CARRIES the card and tab ids it was taken with, so a revert keeps the id of every
/// card that still exists instead of renumbering the whole dashboard. Nothing is remapped here:
/// `replace_layout` decides what an id means, and it honours one only when this dashboard already
/// owns it — a card deleted since the revision comes back with a fresh id, and an id that never
/// belonged here cannot be adopted. (This is still NOT `DashboardRepo::duplicate`, which needs a
/// tab_map because it copies rows into a DIFFERENT dashboard.)
///
/// A revision recorded before ids were carried has none, and reverts exactly as it always did —
/// every card inserted fresh. Revalidation is
/// not optional: a chart a revision references may have been deleted since, and
/// `require_cards_in_ws` inside `apply_dashboard_update` turns that into a 404 rather than a
/// dangling card.
///
/// The revert is itself recorded, as kind 'revert', so undoing a revert is an ordinary revert.
async fn db_revision_revert(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, revision_id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // Scoped on the dashboard, so a revision of ANOTHER dashboard is a 404 and not a cross-object
    // restore, even for a caller who may manage both.
    let rev = st
        .dashboards
        .revision(id, revision_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("revision".into()))?;
    let body: UpdateDashboard = serde_json::from_value(rev.snapshot).map_err(|e| {
        BiApiError::from(egret_core::Error::InvalidInput(format!(
            "revision snapshot is not a valid dashboard layout: {e}"
        )))
    })?;
    Ok(Json(
        apply_dashboard_update(&st, &ctx, id, body, "revert").await?,
    ))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/dashboards/{id}",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.dashboards.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/trash",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard moved to trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_trash(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.dashboards
        .set_trashed(id, true)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/restore",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard restored from trash", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_restore(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    st.dashboards
        .set_trashed(id, false)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/duplicate",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id to duplicate"),
    ),
    responses(
        (status = 200, description = "Dashboard duplicated (layout copied)", body = DashboardDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    // Duplicating CREATES new workspace content, so it requires DB_MANAGE on the
    // source (matching db_create) — a read-only principal must not create rows.
    // The source is still readable by any manager.
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    let copy = st
        .dashboards
        .duplicate(id, ctx.workspace_id.0, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dashboard".into()))?;
    Ok(Json(load_dashboard(&st, copy.id, &ctx).await?))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/bookmark",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard bookmarked for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_bookmark_add(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.dashboards
        .add_bookmark(ctx.principal.user_id.0, "dashboard", id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/dashboards/{id}/bookmark",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Dashboard bookmark removed for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn db_bookmark_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    st.dashboards
        .remove_bookmark(ctx.principal.user_id.0, "dashboard", id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `ctx` rather than a bare user id, because the response now carries what the CALLER may do
/// with this dashboard and that is a PDP question, not a row lookup.
///
/// WHY THE SERVER ANSWERS IT. `dashboard:manage` is workspace-scoped, and the session's
/// `permissions` list is built from **platform** grants only (`session.rs:251-269`, `WHERE
/// ag.scope_type = 'platform'`) — so it can never contain `dashboard:manage`, and a frontend
/// gate written against it would hide Edit from everyone. The alternative, having the browser
/// re-derive workspace authorization from role names, is reimplementing the PDP in TypeScript.
async fn load_dashboard(
    st: &BiState,
    id: Uuid,
    ctx: &egret_core::WorkspaceContext,
) -> Result<DashboardDetailDto, BiApiError> {
    let user = ctx.principal.user_id.0;
    let d = st
        .dashboards
        .get(id, ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dashboard".into()))?;
    let tabs = st.dashboards.tabs(id).await.map_err(BiApiError::from)?;
    let cards = st.dashboards.cards(id).await.map_err(BiApiError::from)?;
    let bookmarked = st
        .dashboards
        .is_bookmarked(user, id)
        .await
        .map_err(BiApiError::from)?;
    let names = st
        .dashboards
        .attribution_names(id)
        .await
        .map_err(BiApiError::from)?;
    // Whether THIS caller may edit. The object resolves inside the same workspace guard the
    // write paths use, so a share granting manage on this dashboard alone counts exactly as it
    // does when the write is attempted — the flag cannot promise more than the PUT will honour.
    let can_manage = match crate::shares::resolve(st, ctx, "dashboard", id).await {
        Ok(obj) => egret_core::authz::require_object(&*st.pdp, ctx, DB_MANAGE, obj)
            .await
            .is_ok(),
        Err(_) => false,
    };
    Ok(crate::model::dashboard_detail_dto(
        d, tabs, cards, bookmarked, names, can_manage,
    ))
}

// ── Folder routes ────────────────────────────────────────────────────────────

/// Map a URL {kind} segment to (entity_type, read_perm, manage_perm). None ⇒ 404.
fn kind_perms(kind: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        "charts" => Some(("chart", CH_READ, CH_MANAGE)),
        "dashboards" => Some(("dashboard", DB_READ, DB_MANAGE)),
        "datasets" => Some(("dataset", DS_READ, DS_MANAGE)),
        "saved_queries" => Some(("saved_query", SQ_READ, SQ_MANAGE)),
        // Alerts live in egret-automation but share this router the same way
        // charts/dashboards/datasets do; `alert:read` was added by migration
        // 0113 precisely so this pair exists. Literal strings so we don't take
        // a cross-crate dependency.
        "alerts" => Some(("alert", "alert:read", "alert:manage")),
        // Data API endpoints live in egret-dataapi and share this router the way
        // alerts does; both permissions exist from migration 0122. Literal
        // strings, again to avoid a cross-crate dependency.
        "data_api_endpoints" => Some((
            "data_api_endpoint",
            "data_api_endpoint:read",
            "data_api_endpoint:manage",
        )),
        // Data apps live in egret-apps and share this router the same way;
        // `app:read`/`app:write` exist from migration 0162. Literal strings, again
        // to avoid a cross-crate dependency.
        "apps" => Some(("app", "app:read", "app:write")),
        _ => None,
    }
}

pub fn folder_router() -> Router<BiState> {
    Router::new()
        .route(
            "/ws/{workspace}/folders/{kind}",
            get(folders_list).post(folders_create),
        )
        .route(
            "/ws/{workspace}/folders/{kind}/{id}",
            patch(folders_rename).delete(folders_delete),
        )
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/folders/{kind}",
    tag = "folders",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets"),
    ),
    responses(
        (status = 200, description = "Folders for the entity kind in the workspace", body = Vec<FolderDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing the entity kind's read permission", body = ErrorBody),
        (status = 404, description = "Unknown entity kind", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn folders_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
) -> Result<Json<Vec<crate::model::FolderDto>>, BiApiError> {
    let (etype, read, _manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    egret_core::authz::require(&*st.pdp, &ctx, read)
        .await
        .map_err(BiApiError::from)?;
    let rows = st
        .folders
        .list(ctx.workspace_id.0, etype)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        rows.into_iter().map(crate::model::folder_dto).collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/folders/{kind}",
    tag = "folders",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets"),
    ),
    request_body = CreateFolder,
    responses(
        (status = 200, description = "Folder created", body = FolderDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing the entity kind's manage permission", body = ErrorBody),
        (status = 404, description = "Unknown entity kind", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn folders_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
    Json(b): Json<crate::model::CreateFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    egret_core::authz::require(&*st.pdp, &ctx, manage)
        .await
        .map_err(BiApiError::from)?;
    let f = st
        .folders
        .create(
            ctx.workspace_id.0,
            etype,
            &b.name,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(crate::model::folder_dto(f)))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/folders/{kind}/{id}",
    tag = "folders",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets"),
        ("id" = Uuid, Path, description = "Folder id"),
    ),
    request_body = RenameFolder,
    responses(
        (status = 200, description = "Folder renamed", body = FolderDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing the entity kind's manage permission", body = ErrorBody),
        (status = 404, description = "Unknown entity kind, or no such folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn folders_rename(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
    Json(b): Json<crate::model::RenameFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    egret_core::authz::require(&*st.pdp, &ctx, manage)
        .await
        .map_err(BiApiError::from)?;
    require_folder(&st, &ctx, id, etype).await?;
    let f = st
        .folders
        .rename(id, &b.name)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("folder".into()))?;
    Ok(Json(crate::model::folder_dto(f)))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/folders/{kind}/{id}",
    tag = "folders",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets"),
        ("id" = Uuid, Path, description = "Folder id"),
    ),
    responses(
        (status = 200, description = "Folder deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing the entity kind's manage permission", body = ErrorBody),
        (status = 404, description = "Unknown entity kind, or no such folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn folders_delete(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    egret_core::authz::require(&*st.pdp, &ctx, manage)
        .await
        .map_err(BiApiError::from)?;
    require_folder(&st, &ctx, id, etype).await?;
    st.folders.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Guard: folder exists, in this workspace, and matches the URL kind's entity_type.
async fn require_folder(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
    entity_type: &str,
) -> Result<(), BiApiError> {
    match st.folders.meta(id).await.map_err(BiApiError::from)? {
        Some((w, t)) if w == ctx.workspace_id.0 && t == entity_type => Ok(()),
        _ => Err(Error::NotFound("folder".into()).into()),
    }
}

// ── OpenAPI fragments (E0·P4b-6) ─────────────────────────────────────────────
//
// One `#[derive(utoipa::OpenApi)]` doc struct per router, co-located with its
// handlers and merged into `egret_bi::openapi()`. Nested request/response
// schemas (`CreateSource`, `MetricDto`, `QuerySpec` → `Filter`/`OrderBy`/
// `FilterOp`, …) are collected automatically from the listed paths + component
// types.

/// OpenAPI fragment for the dataset router (`/api/ws/{workspace}/datasets…`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        ds_create,
        ds_list,
        ds_get,
        ds_update,
        ds_remove,
        ds_refresh,
        ds_duplicate,
        ds_data,
        ds_set_certification,
        ds_add_metric,
        ds_update_metric,
        ds_delete_metric,
        ds_set_metric_certification,
        ds_add_column,
        ds_update_column,
        ds_delete_column,
        ds_set_folder,
        ds_trash,
        ds_restore,
        ds_bookmark_add,
        ds_bookmark_remove,
        ds_usage,
    ),
    components(schemas(
        CreateDataset,
        CreateSource,
        UpdateDataset,
        SetDatasetCertification,
        DatasetDetailDto,
        DatasetRefreshDto,
        DuplicateDataset,
        DatasetSummaryDto,
        DatasetColumnDto,
        MetricDto,
        MetricBody,
        AddColumnBody,
        UpdateColumnBody,
        crate::model::DatasetUsageDto,
        DatasetDataBody,
        ChartRunResponse,
        egret_core::ColumnMeta,
        SetFolder,
        Ack,
        ErrorBody,
    )),
    tags((name = "datasets", description = "BI datasets: sources, columns, metrics, folders, trash + bookmarks")),
)]
pub struct BiDatasetApi;

/// OpenAPI fragment for the chart router (`/api/ws/{workspace}/charts…`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        ch_create,
        ch_list,
        ch_get,
        ch_update,
        ch_remove,
        ch_data,
        ch_run,
        ch_run_spec,
        ch_set_folder,
        ch_duplicate,
        ch_trash,
        ch_restore,
        ch_bookmark_add,
        ch_bookmark_remove,
    ),
    components(schemas(
        CreateChart,
        UpdateChart,
        ChartDto,
        ChartDataBody,
        ChartDataResponse,
        ChartRunResponse,
        egret_core::ColumnMeta,
        crate::chartquery::QuerySpec,
        crate::chartquery::Filter,
        crate::chartquery::OrderBy,
        crate::chartquery::FilterOp,
        SetFolder,
        Ack,
        ErrorBody,
    )),
    tags((name = "charts", description = "BI charts: CRUD, compile-to-SQL data requests, folders, trash + bookmarks")),
)]
pub struct BiChartApi;

/// OpenAPI fragment for the folder router (`/api/ws/{workspace}/folders/{kind}…`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(folders_list, folders_create, folders_rename, folders_delete),
    components(schemas(FolderDto, CreateFolder, RenameFolder, Ack, ErrorBody)),
    tags((name = "folders", description = "Generic per-kind folders (charts / dashboards / datasets)")),
)]
pub struct BiFolderApi;

/// OpenAPI fragment for the SQL Lab router (`/api/ws/{workspace}/sqllab…`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        save,
        list,
        load,
        update,
        remove,
        sq_set_folder,
        catalogs,
        schemas,
        tables,
        columns,
        search,
        snippets_list,
        snippets_create,
        snippets_update,
        snippets_remove,
    ),
    components(schemas(
        SaveQuery,
        SavedQueryDto,
        SchemaColumn,
        crate::model::SearchHit,
        crate::model::SnippetDto,
        crate::model::SnippetBody,
        SetFolder,
        Ack,
        ErrorBody,
    )),
    tags((name = "sqllab", description = "SQL Lab: saved queries + schema browse (catalogs / schemas / tables / columns)")),
)]
pub struct BiSqlLabApi;

/// OpenAPI fragment for the dashboard router (`/api/ws/{workspace}/dashboards…`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        db_create,
        db_list,
        db_get,
        db_update,
        db_remove,
        db_duplicate,
        db_trash,
        db_restore,
        db_bookmark_add,
        db_bookmark_remove,
        db_set_folder,
        db_card_run,
        db_revisions,
        db_revision_revert,
        db_set_certification,
    ),
    components(schemas(
        CreateDashboard,
        UpdateDashboard,
        SetDashboardCertification,
        DashboardSummaryDto,
        DashboardDetailDto,
        DashboardTabDto,
        DashboardCardDto,
        DashboardRevisionDto,
        CardInput,
        NewTabInput,
        SetFolder,
        ChartRunResponse,
        CardRunBody,
        crate::chartquery::Filter,
        crate::chartquery::FilterOp,
        Ack,
        ErrorBody,
    )),
    tags((name = "dashboards", description = "BI dashboards: CRUD, layout (tabs + cards), folders, trash + bookmarks")),
)]
pub struct BiDashboardApi;

#[cfg(test)]
mod tests {
    use super::ensure_unique_column_names;
    use axum::response::IntoResponse;

    fn cols(names: &[&str]) -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (n.to_string(), "integer".to_string()))
            .collect()
    }

    #[test]
    fn a_repeated_column_name_is_a_400_that_names_the_column() {
        let err = ensure_unique_column_names(&cols(&["x", "y", "x"])).unwrap_err();
        assert!(err.to_string().contains("`x`"), "{err}");
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn distinct_names_pass_and_case_is_significant() {
        assert!(ensure_unique_column_names(&cols(&["x", "X", "y"])).is_ok());
    }
}
