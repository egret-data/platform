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
use crate::expr::validate_expression;
use crate::model::{
    AddColumnBody, CardInput, ChartDataBody, ChartDataResponse, ChartDto, ChartRunResponse,
    CreateChart, CreateDashboard, CreateDataset, CreateFolder, CreateSource, DashboardCardDto,
    DashboardDetailDto, DashboardSummaryDto, DashboardTabDto, DatasetColumnDto, DatasetDetailDto,
    DatasetSummaryDto, FolderDto, MetricBody, MetricDto, NewTabInput, RenameFolder, SaveQuery,
    SavedQueryDto, SchemaColumn, SetFolder, UpdateChart, UpdateColumnBody, UpdateDashboard,
    UpdateDataset,
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
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub datasets: crate::dataset::DatasetRepo,
    pub charts: crate::chart::ChartRepo,
    pub dashboards: crate::dashboard::DashboardRepo,
    pub folders: crate::folder::FolderRepo,
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
const DS_READ: &str = "dataset:read";
const DS_MANAGE: &str = "dataset:manage";
const CH_READ: &str = "chart:read";
const CH_MANAGE: &str = "chart:manage";
const DB_READ: &str = "dashboard:read";
const DB_MANAGE: &str = "dashboard:manage";

pub fn sqllab_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/sqllab/queries", post(save).get(list))
        .route(
            "/ws/{workspace}/sqllab/queries/{id}",
            get(load).put(update).delete(remove),
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
}

fn dto(q: crate::saved::SavedQuery) -> SavedQueryDto {
    SavedQueryDto {
        id: q.id.to_string(),
        database_id: q.database_id.map(|d| d.to_string()),
        name: q.name,
        sql: q.sql,
        description: q.description,
        created_by: q.created_by.map(|u| u.to_string()),
        updated_at: q.updated_at,
    }
}

fn is_admin(ctx: &egret_core::WorkspaceContext) -> bool {
    ctx.principal
        .platform_roles
        .iter()
        .any(|r| r == "platform_admin")
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
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn save(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<SaveQuery>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM)
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
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<SavedQueryDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM)
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
        (status = 403, description = "Missing query:run permission", body = ErrorBody),
        (status = 404, description = "No such saved query", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn load(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM)
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
        (status = 403, description = "Missing query:run permission, or not the query owner", body = ErrorBody),
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
    egret_core::authz::require(&*st.pdp, &ctx, PERM)
        .await
        .map_err(BiApiError::from)?;
    require_owner(&st, &ctx, id).await?;
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
        (status = 403, description = "Missing query:run permission, or not the query owner", body = ErrorBody),
        (status = 404, description = "No such saved query", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, PERM)
        .await
        .map_err(BiApiError::from)?;
    require_owner(&st, &ctx, id).await?;
    st.saved.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
async fn require_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.saved.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
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
async fn conn_spec_granted(
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
    Ok(ConnSpec::from_database(&database, secret))
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

// ── Dataset routes ──────────────────────────────────────────────────────────

pub fn dataset_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/datasets", post(ds_create).get(ds_list))
        .route(
            "/ws/{workspace}/datasets/{id}",
            get(ds_get).put(ds_update).delete(ds_remove),
        )
        .route("/ws/{workspace}/datasets/{id}/refresh", post(ds_refresh))
        .route("/ws/{workspace}/datasets/{id}/metrics", post(ds_add_metric))
        .route(
            "/ws/{workspace}/datasets/{id}/metrics/{metric_id}",
            put(ds_update_metric).delete(ds_delete_metric),
        )
        .route("/ws/{workspace}/datasets/{id}/columns", post(ds_add_column))
        .route(
            "/ws/{workspace}/datasets/{id}/columns/{column_name}",
            put(ds_update_column).delete(ds_delete_column),
        )
        .route("/ws/{workspace}/datasets/{id}/data", post(ds_data))
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
        CreateSource::Virtual { sql } => SourceSpec::Virtual { sql },
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
        (status = 200, description = "Physical columns reconciled from the source", body = DatasetDetailDto),
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
    let source = source_of(&ds);
    let new_columns = introspect(&st, &spec, &source).await?;
    st.datasets
        .reconcile_physical_columns(id, &new_columns)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/datasets/{id}",
    tag = "datasets",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dataset id"),
    ),
    responses(
        (status = 200, description = "Dataset deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ds_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
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

    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        ds.database_id,
        &sql,
        Some(limit as usize),
        Some(ctx.principal.email.clone()),
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
        (status = 400, description = "Invalid metric expression", body = ErrorBody),
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
    let d = validate_expression(&spec, &source_of(&ds), &b.expression)
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
        (status = 200, description = "Metric updated (expression validated)", body = DatasetDetailDto),
        (status = 400, description = "Invalid metric expression", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dataset:manage permission, or not the dataset owner", body = ErrorBody),
        (status = 404, description = "No such dataset, or metric not in this dataset", body = ErrorBody),
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
        .update_metric(
            metric_id,
            &b.metric_name,
            &b.expression,
            &d.data_type,
            &d.type_generic,
            &b.verbose_name,
            &b.description,
        )
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
    ),
    responses(
        (status = 200, description = "Metric deleted", body = DatasetDetailDto),
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
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DS_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    require_metric_in_dataset(&st, id, metric_id).await?;
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
        (status = 400, description = "Invalid column expression", body = ErrorBody),
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
            eu,
        )
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
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
        .duplicate(id, Some(ctx.principal.user_id.0))
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
        .get(id)
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
        .get(id)
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
    let (sql, database_id) = compile_chart_sql(&st, &ctx, b.dataset_id, &b.query_spec).await?;
    Ok(Json(ChartDataResponse {
        sql,
        database_id: database_id.to_string(),
    }))
}

/// Compile a dataset's `QuerySpec` to `(sql, database_id)`. The dataset must live
/// in the caller's workspace (cross-tenant guard → `NotFound`). Shared by
/// `ch_data` (client-supplied spec) and `ch_run` (the chart's SAVED spec).
async fn compile_chart_sql(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    dataset_id: Uuid,
    spec: &chartquery::QuerySpec,
) -> Result<(String, Uuid), BiApiError> {
    let ds = st
        .datasets
        .get(dataset_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    let database = st
        .databases
        .get(ds.database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let cols = st
        .datasets
        .columns(dataset_id)
        .await
        .map_err(BiApiError::from)?;
    let metrics = st
        .datasets
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
        .get(id)
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

    let (sql, database_id) = compile_chart_sql(&st, &ctx, chart.dataset_id, &spec).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(spec.row_limit.max(0) as usize),
        Some(ctx.principal.email.clone()),
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
        .get(chart_id)
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

    let (sql, database_id) = compile_chart_sql(&st, &ctx, chart.dataset_id, &spec).await?;
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(spec.row_limit.max(0) as usize),
        Some(ctx.principal.email.clone()),
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
    Ok(Json(
        rows.into_iter()
            .map(|(d, n, bm)| crate::model::dashboard_summary_dto(d, n, bm))
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
    Ok(Json(
        load_dashboard(&st, d.id, ctx.principal.user_id.0).await?,
    ))
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
    Ok(Json(
        load_dashboard(&st, id, ctx.principal.user_id.0).await?,
    ))
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
    require_cards_in_ws(&st, &ctx, &b.cards).await?;
    crate::model::validate_layout(&b.tabs, &b.cards).map_err(BiApiError::from)?;
    if b.width != "fixed" && b.width != "full" {
        return Err(
            egret_core::Error::InvalidInput("width must be 'fixed' or 'full'".into()).into(),
        );
    }
    st.dashboards
        .update(id, &b.name, &b.description, &b.parameters, &b.width)
        .await
        .map_err(BiApiError::from)?;
    let tabs: Vec<crate::dashboard::NewTab> = b.tabs.into_iter().map(Into::into).collect();
    let cards: Vec<crate::dashboard::NewCard> = b.cards.into_iter().map(Into::into).collect();
    st.dashboards
        .replace_layout(id, &tabs, &cards)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        load_dashboard(&st, id, ctx.principal.user_id.0).await?,
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
        .duplicate(id, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dashboard".into()))?;
    Ok(Json(
        load_dashboard(&st, copy.id, ctx.principal.user_id.0).await?,
    ))
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

async fn load_dashboard(
    st: &BiState,
    id: Uuid,
    user: Uuid,
) -> Result<DashboardDetailDto, BiApiError> {
    let d = st
        .dashboards
        .get(id)
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
    Ok(crate::model::dashboard_detail_dto(
        d, tabs, cards, bookmarked,
    ))
}

// ── Folder routes ────────────────────────────────────────────────────────────

/// Map a URL {kind} segment to (entity_type, read_perm, manage_perm). None ⇒ 404.
fn kind_perms(kind: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        "charts" => Some(("chart", CH_READ, CH_MANAGE)),
        "dashboards" => Some(("dashboard", DB_READ, DB_MANAGE)),
        "datasets" => Some(("dataset", DS_READ, DS_MANAGE)),
        // Notebooks live in egret-notebook but share the generic folder router;
        // the perms already exist in the DB (migration 0037). Literal strings so
        // we don't take a cross-crate dependency.
        "notebooks" => Some(("notebook", "notebook:read", "notebook:write")),
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
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets | notebooks"),
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
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets | notebooks"),
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
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets | notebooks"),
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
        ("kind" = String, Path, description = "Entity kind: charts | dashboards | datasets | notebooks"),
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
        ds_data,
        ds_add_metric,
        ds_update_metric,
        ds_delete_metric,
        ds_add_column,
        ds_update_column,
        ds_delete_column,
        ds_set_folder,
        ds_trash,
        ds_restore,
        ds_bookmark_add,
        ds_bookmark_remove,
    ),
    components(schemas(
        CreateDataset,
        CreateSource,
        UpdateDataset,
        DatasetDetailDto,
        DatasetSummaryDto,
        DatasetColumnDto,
        MetricDto,
        MetricBody,
        AddColumnBody,
        UpdateColumnBody,
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
    tags((name = "folders", description = "Generic per-kind folders (charts / dashboards / datasets / notebooks)")),
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
        catalogs,
        schemas,
        tables,
        columns,
    ),
    components(schemas(
        SaveQuery,
        SavedQueryDto,
        SchemaColumn,
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
    ),
    components(schemas(
        CreateDashboard,
        UpdateDashboard,
        DashboardSummaryDto,
        DashboardDetailDto,
        DashboardTabDto,
        DashboardCardDto,
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
