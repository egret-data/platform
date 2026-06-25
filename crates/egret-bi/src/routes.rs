//! SQL Lab routes: saved queries + schema browse. Dataset CRUD routes.

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::Error;
use egret_database::DatabaseRepo;
use egret_engine::connector::{build_connector, ConnSpec};
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
    AddColumnBody, ChartDataBody, ChartDataResponse, CreateChart, CreateDashboard, CreateDataset,
    CreateSource, DashboardDetailDto, DashboardSummaryDto, DatasetDetailDto, DatasetSummaryDto,
    MetricBody, SaveQuery, SavedQueryDto, UpdateChart, UpdateColumnBody, UpdateDashboard,
    UpdateDataset,
};
use crate::saved::SavedQueryRepo;
use crate::schema::SchemaService;
use crate::typegen::generic_type;

#[derive(Clone)]
pub struct BiState {
    pub saved: SavedQueryRepo,
    pub schema: SchemaService,
    pub databases: DatabaseRepo,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
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

async fn save(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<SaveQuery>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
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
async fn list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<SavedQueryDto>>, BiApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(BiApiError::from)?;
    let qs = st
        .saved
        .list(ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(qs.into_iter().map(dto).collect()))
}
async fn load(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
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
async fn update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<SaveQuery>,
) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
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
async fn remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
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
    st.authz
        .require_workspace(ctx, perm)
        .await
        .map_err(BiApiError::from)?;
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
        .route("/ws/{workspace}/datasets/{id}/folder", patch(ds_set_folder))
}

/// Guard: dataset exists and lives in this workspace. Mirrors the inline
/// `get` + workspace check the other dataset handlers use.
async fn require_dataset_in_ws(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    if ds.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("dataset".into()).into());
    }
    Ok(())
}

async fn ds_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_in_ws(&st, &ctx, id).await?;
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

async fn ds_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DatasetSummaryDto>>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_READ)
        .await
        .map_err(BiApiError::from)?;
    let rows = st
        .datasets
        .list(ctx.workspace_id.0)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|(ds, n)| crate::model::summary_dto(ds, n))
            .collect(),
    ))
}

async fn ds_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_READ)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    st.datasets
        .update(id, &b.name, &b.description, b.sql.as_deref())
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_refresh(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
    let source = source_of(&ds);
    let new_columns = introspect(&st, &spec, &source).await?;
    st.datasets
        .reconcile_physical_columns(id, &new_columns)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
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

async fn require_dataset_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.datasets.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
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

async fn ds_add_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
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

async fn ds_update_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
    Json(b): Json<MetricBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    require_metric_in_dataset(&st, id, metric_id).await?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
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

async fn ds_delete_metric(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, metric_id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    require_metric_in_dataset(&st, id, metric_id).await?;
    st.datasets
        .delete_metric(metric_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(load_detail(&st, id).await?))
}

async fn ds_add_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<AddColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
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

async fn ds_update_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>,
    Json(b): Json<UpdateColumnBody>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
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
                let spec = conn_spec_perm(&st, &ctx, ds.database_id, DS_MANAGE).await?;
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

async fn ds_delete_column(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, column_name)): Path<(String, Uuid, String)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DS_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
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

async fn ch_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "chart").await?;
    }
    st.charts
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    let copy = st
        .charts
        .duplicate(id, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(copy)))
}

async fn require_chart_in_ws(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    match st
        .charts
        .chart_workspace(id)
        .await
        .map_err(BiApiError::from)?
    {
        Some(w) if w == ctx.workspace_id.0 => Ok(()),
        _ => Err(Error::NotFound("chart".into()).into()),
    }
}

async fn require_chart_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.charts.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
}

async fn ch_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    AxQuery(q): AxQuery<ListQuery>,
) -> Result<Json<Vec<crate::model::ChartDto>>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_READ)
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

async fn ch_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
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

async fn ch_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    let c = st
        .charts
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(c)))
}

async fn ch_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateChart>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    require_chart_owner(&st, &ctx, id).await?;
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

async fn ch_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    require_chart_owner(&st, &ctx, id).await?;
    st.charts.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_data(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<ChartDataBody>,
) -> Result<Json<ChartDataResponse>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_READ)
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
    let database = st
        .databases
        .get(ds.database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let cols = st
        .datasets
        .columns(b.dataset_id)
        .await
        .map_err(BiApiError::from)?;
    let metrics = st
        .datasets
        .metrics(b.dataset_id)
        .await
        .map_err(BiApiError::from)?;
    let source = source_of(&ds);
    let sql = chartquery::build(database.engine, &source, &cols, &metrics, &b.query_spec)
        .map_err(BiApiError::from)?;
    Ok(Json(ChartDataResponse {
        sql,
        database_id: ds.database_id.to_string(),
    }))
}

async fn ch_trash(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    st.charts
        .set_trashed(id, true)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_restore(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    st.charts
        .set_trashed(id, false)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_bookmark_add(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    st.charts
        .add_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn ch_bookmark_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, CH_READ)
        .await
        .map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
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
}

async fn db_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "dashboard").await?;
    }
    st.dashboards
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn require_dashboard_in_ws(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    match st
        .dashboards
        .dashboard_workspace(id)
        .await
        .map_err(BiApiError::from)?
    {
        Some(w) if w == ctx.workspace_id.0 => Ok(()),
        _ => Err(Error::NotFound("dashboard".into()).into()),
    }
}

async fn require_dashboard_owner(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<(), BiApiError> {
    let owner = st.dashboards.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) {
        Ok(())
    } else {
        Err(Error::Forbidden.into())
    }
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

async fn db_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    AxQuery(q): AxQuery<ListQuery>,
) -> Result<Json<Vec<DashboardSummaryDto>>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_READ)
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

async fn db_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<CreateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
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

async fn db_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_READ)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    Ok(Json(
        load_dashboard(&st, id, ctx.principal.user_id.0).await?,
    ))
}

async fn db_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
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
    let tabs: Vec<crate::dashboard::NewTab> = b.tabs.into_iter().map(|t| t.into_new()).collect();
    let cards: Vec<crate::dashboard::NewCard> = b.cards.into_iter().map(|c| c.into_new()).collect();
    st.dashboards
        .replace_layout(id, &tabs, &cards)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        load_dashboard(&st, id, ctx.principal.user_id.0).await?,
    ))
}

async fn db_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    st.dashboards.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_trash(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    st.dashboards
        .set_trashed(id, true)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_restore(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_MANAGE)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    st.dashboards
        .set_trashed(id, false)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_READ)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
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

async fn db_bookmark_add(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_READ)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    st.dashboards
        .add_bookmark(ctx.principal.user_id.0, "dashboard", id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_bookmark_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz
        .require_workspace(&ctx, DB_READ)
        .await
        .map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
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

async fn folders_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
) -> Result<Json<Vec<crate::model::FolderDto>>, BiApiError> {
    let (etype, read, _manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz
        .require_workspace(&ctx, read)
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

async fn folders_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
    Json(b): Json<crate::model::CreateFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz
        .require_workspace(&ctx, manage)
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

async fn folders_rename(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
    Json(b): Json<crate::model::RenameFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz
        .require_workspace(&ctx, manage)
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

async fn folders_delete(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz
        .require_workspace(&ctx, manage)
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
