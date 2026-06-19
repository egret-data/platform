//! SQL Lab routes: saved queries + schema browse. Dataset CRUD routes.

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, post};
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

use crate::dataset::{NewColumn, SourceSpec};
use crate::error::BiApiError;
use crate::model::{
    CreateDataset, CreateSource, DatasetDetailDto, DatasetSummaryDto, SaveQuery, SavedQueryDto,
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
}

/// Introspect a source's columns and classify them. Physical → schema service; virtual → describe().
async fn introspect(
    st: &BiState,
    spec: &ConnSpec,
    source: &SourceSpec,
) -> Result<Vec<NewColumn>, BiApiError> {
    let raw: Vec<(String, String)> = match source {
        SourceSpec::Physical { catalog, schema, table } => st
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
        CreateSource::Physical { catalog, schema, table } => {
            SourceSpec::Physical { catalog, schema, table }
        }
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
    let cols = st.datasets.columns(ds.id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DatasetSummaryDto>>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_READ).await.map_err(BiApiError::from)?;
    let rows = st.datasets.list(ctx.workspace_id.0).await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(|(ds, n)| crate::model::summary_dto(ds, n)).collect()))
}

async fn ds_get(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_READ).await.map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDataset>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    let ds = st
        .datasets
        .update(id, &b.name, &b.description, b.sql.as_deref())
        .await
        .map_err(BiApiError::from)?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_refresh(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DatasetDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
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
    st.datasets.replace_columns(id, &new_columns).await.map_err(BiApiError::from)?;
    let ds = st
        .datasets
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dataset".into()))?;
    let cols = st.datasets.columns(id).await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::detail_dto(ds, cols)))
}

async fn ds_remove(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DS_MANAGE).await.map_err(BiApiError::from)?;
    require_dataset_owner(&st, &ctx, id).await?;
    st.datasets.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
