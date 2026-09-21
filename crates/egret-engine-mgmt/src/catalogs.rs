//! Catalog registry routes (SP3 §4.4–4.7): platform catalogs, the engines they
//! are attached to, their workspace assignment and the migration report. Merged
//! into the engines router, so mounted under `/api/control-plane`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_catalog_registry::{CatalogDef, CatalogEngineRow, CatalogFilter, CatalogRow};
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::catalog_apply::{
    attach_to_engine, propagate, retry_apply, validate_references, ApplyOutcome, CatalogOpError,
};
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

pub(crate) fn routes() -> Router<CpState> {
    Router::new()
        .route(
            "/catalogs",
            get(list_registry_catalogs).post(create_registry_catalog),
        )
        .route(
            "/catalogs/{id}",
            get(get_registry_catalog)
                .put(update_registry_catalog)
                .delete(delete_registry_catalog),
        )
        .route("/catalogs/{id}/impact", get(catalog_impact))
        .route("/catalogs/{id}/reapply", post(reapply_registry_catalog))
        .route("/catalogs/migration-report", get(get_migration_report))
        .route(
            "/catalogs/migration-report/acknowledge",
            post(acknowledge_migration_report),
        )
        .route(
            "/catalogs/{id}/workspaces",
            get(list_catalog_workspaces).put(set_catalog_workspaces),
        )
        .route(
            "/engines/{id}/catalogs/{cid}/apply",
            post(retry_catalog_apply),
        )
}

/// One engine a catalog is attached to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogAttachmentDto {
    engine_id: Uuid,
    engine_name: String,
    engine_type: String,
    /// `applied` | `failed` | `pending` (recorded by egret, not yet accepted
    /// by the engine — SP3b D1; only a StarRocks attachment is ever written
    /// that way, and only for as long as its convergence takes).
    apply_status: String,
    apply_error: Option<String>,
    applied_at: Option<DateTime<Utc>>,
}

impl From<CatalogEngineRow> for CatalogAttachmentDto {
    fn from(r: CatalogEngineRow) -> Self {
        CatalogAttachmentDto {
            engine_id: r.engine_id,
            engine_name: r.engine_name,
            engine_type: r.engine_type,
            apply_status: r.apply_status,
            apply_error: r.apply_error,
            applied_at: r.applied_at,
        }
    }
}

/// A registry catalog. `config` is the typed per-kind configuration (snake_case
/// fields, spec §4.2); it never holds a plaintext secret.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegistryCatalogDto {
    id: Uuid,
    name: String,
    kind: String,
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    description: Option<String>,
    /// The warehouse an egret catalog is.
    warehouse_id: Option<Uuid>,
    engines: Vec<CatalogAttachmentDto>,
    workspace_ids: Vec<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateRegistryCatalogBody {
    /// SQL name on every engine: `^[a-z][a-z0-9_]{0,62}$`, immutable.
    name: String,
    /// iceberg | hive | delta_lake | hudi | jdbc | elasticsearch | tpch | tpcds | generic
    kind: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

/// Replaces config/advanced/description; name and kind never change.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateRegistryCatalogBody {
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    advanced: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

/// One engine's result of applying a catalog.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyOutcomeDto {
    engine_id: Uuid,
    engine_name: String,
    /// `applied` | `failed`.
    status: String,
    error: Option<String>,
}

impl From<ApplyOutcome> for ApplyOutcomeDto {
    fn from(o: ApplyOutcome) -> Self {
        ApplyOutcomeDto {
            engine_id: o.engine_id,
            engine_name: o.engine_name,
            status: if o.applied { "applied" } else { "failed" }.into(),
            error: o.error,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogUpdateResultDto {
    catalog: RegistryCatalogDto,
    outcomes: Vec<ApplyOutcomeDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogImpactEngineDto {
    engine_id: Uuid,
    engine_name: String,
    engine_type: String,
    /// `restart` (Trino rolls its pods) | `in_place` (StarRocks rebuilds the connector).
    action: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogImpactDto {
    engines: Vec<CatalogImpactEngineDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictEngine {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictWorkspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

/// 409 body of a refused catalog delete. Two shapes share this struct (same
/// pattern as `EngineConflictBody`): `code = "catalog_in_use"` carries the
/// non-empty `engines`/`workspaces` that still use the catalog; `code =
/// "warehouse_catalog"` (an egret warehouse catalog, refused unconditionally —
/// delete the warehouse instead) carries neither, so they're omitted.
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogConflictBody {
    pub error: String,
    pub code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub engines: Vec<CatalogConflictEngine>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<CatalogConflictWorkspace>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListCatalogsQuery {
    /// Only catalogs assigned to this workspace.
    workspace_id: Option<Uuid>,
    /// Only catalogs referencing this secret (config or engine credential).
    secret_id: Option<Uuid>,
}

async fn to_dto(st: &CpState, row: CatalogRow) -> Result<RegistryCatalogDto, CpError> {
    let registry = st.engine_mgmt.registry();
    let engines = registry
        .list_catalog_attachments(row.id)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    let workspace_ids = registry
        .list_catalog_workspaces(row.id)
        .await?
        .into_iter()
        .map(|w| w.workspace_id)
        .collect();
    Ok(RegistryCatalogDto {
        id: row.id,
        name: row.name,
        kind: row.kind,
        config: row.config,
        advanced: row.advanced,
        description: row.description,
        warehouse_id: row.warehouse_id,
        engines,
        workspace_ids,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn load(st: &CpState, id: Uuid) -> Result<CatalogRow, CpError> {
    st.engine_mgmt
        .registry()
        .get_catalog(id)
        .await?
        .ok_or(CpError::NotFound)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs",
    tag = "control-plane-catalog-registry",
    params(ListCatalogsQuery),
    responses(
        (status = 200, description = "Registry catalogs with their engines and workspaces", body = Vec<RegistryCatalogDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_registry_catalogs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<ListCatalogsQuery>,
) -> Result<Json<Vec<RegistryCatalogDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let rows = st
        .engine_mgmt
        .registry()
        .list_catalogs(&CatalogFilter {
            workspace_id: q.workspace_id,
            secret_id: q.secret_id,
        })
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(to_dto(&st, row).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs",
    tag = "control-plane-catalog-registry",
    request_body = CreateRegistryCatalogBody,
    responses(
        (status = 201, description = "Catalog created (not attached to any engine)", body = RegistryCatalogDto),
        (status = 400, description = "Invalid name, kind or configuration, an unknown secret or storage connection, or an egret warehouse catalog (those are created with their warehouse)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 409, description = "A catalog with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateRegistryCatalogBody>,
) -> Result<(StatusCode, Json<RegistryCatalogDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    let def = CatalogDef::parse(&b.kind, &b.config, &b.advanced)
        .map_err(|e| CpError::BadRequest(e.to_string()))?;
    if def.warehouse_id().is_some() {
        return Err(CpError::BadRequest(
            "an egret warehouse catalog is created with its warehouse".into(),
        ));
    }
    validate_references(&st, &def).await?;
    let row = st
        .engine_mgmt
        .registry()
        .create_catalog(&b.name, &def, b.description.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(to_dto(&st, row).await?)))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "The catalog", body = RegistryCatalogDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<RegistryCatalogDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let row = load(&st, id).await?;
    Ok(Json(to_dto(&st, row).await?))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    request_body = UpdateRegistryCatalogBody,
    responses(
        (status = 200, description = "Stored, then applied to every attached engine; `outcomes` lists each engine (a failed engine is flagged, nothing is reverted)", body = CatalogUpdateResultDto),
        (status = 400, description = "Invalid configuration, an unknown secret or storage connection, or a changed warehouse", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateRegistryCatalogBody>,
) -> Result<Json<CatalogUpdateResultDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let row = load(&st, id).await?;
    let def = CatalogDef::parse(&row.kind, &b.config, &b.advanced)
        .map_err(|e| CpError::BadRequest(e.to_string()))?;
    if def.warehouse_id() != row.warehouse_id {
        return Err(CpError::BadRequest(
            "the warehouse of an egret catalog cannot change".into(),
        ));
    }
    validate_references(&st, &def).await?;
    let updated = st
        .engine_mgmt
        .registry()
        .update_catalog(id, &def, b.description.as_deref())
        .await?
        .ok_or(CpError::NotFound)?;
    let outcomes = propagate(&st, &updated).await?;
    Ok(Json(CatalogUpdateResultDto {
        catalog: to_dto(&st, updated).await?,
        outcomes: outcomes.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/catalogs/{id}",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 204, description = "Catalog deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 409, description = "Nothing deleted: an egret warehouse catalog (`code` = `warehouse_catalog`; it goes with its warehouse — this route refuses it unconditionally, delete the warehouse instead), or still attached or assigned (`code` = `catalog_in_use`, with `engines` and `workspaces`)", body = CatalogConflictBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    let row = load(&st, id).await?;
    // An egret warehouse catalog is refused unconditionally, before even
    // looking at attachments/assignments: it goes with its warehouse (FK
    // cascade) and is never deletable through this route, so telling the
    // caller to detach/unassign it first (the catalog_in_use message) would be
    // actionable-sounding advice that leads nowhere.
    if row.warehouse_id.is_some() {
        return Err(CpError::WarehouseCatalog(
            "this catalog is an egret warehouse; delete the warehouse instead".into(),
        ));
    }
    let registry = st.engine_mgmt.registry();
    let engines = registry.list_catalog_attachments(id).await?;
    let workspaces = registry.list_catalog_workspaces(id).await?;
    if !engines.is_empty() || !workspaces.is_empty() {
        return Err(CpError::CatalogInUse {
            engines: engines
                .into_iter()
                .map(|e| CatalogConflictEngine {
                    id: e.engine_id,
                    name: e.engine_name,
                })
                .collect(),
            workspaces: workspaces
                .into_iter()
                .map(|w| CatalogConflictWorkspace {
                    id: w.workspace_id,
                    name: w.name,
                    slug: w.slug,
                })
                .collect(),
        });
    }
    registry.delete_catalog(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}/impact",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "What saving this catalog does to each attached engine", body = CatalogImpactDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn catalog_impact(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<CatalogImpactDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    load(&st, id).await?;
    let engines = st
        .engine_mgmt
        .registry()
        .list_catalog_attachments(id)
        .await?
        .into_iter()
        .map(|e| CatalogImpactEngineDto {
            action: if e.engine_type == "starrocks" {
                "in_place"
            } else {
                "restart"
            }
            .into(),
            engine_id: e.engine_id,
            engine_name: e.engine_name,
            engine_type: e.engine_type,
        })
        .collect();
    Ok(Json(CatalogImpactDto { engines }))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs/{id}/reapply",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "The unchanged configuration re-applied to every attached engine (e.g. after rotating a secret)", body = Vec<ApplyOutcomeDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn reapply_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ApplyOutcomeDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let row = load(&st, id).await?;
    Ok(Json(
        propagate(&st, &row)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetCatalogWorkspacesBody {
    workspace_ids: Vec<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogWorkspaceDto {
    workspace_id: Uuid,
    name: String,
    slug: String,
    assigned_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationReportRowDto {
    id: i64,
    run_id: Uuid,
    created_at: DateTime<Utc>,
    /// `info` | `warning`.
    severity: String,
    code: String,
    catalog_id: Option<Uuid>,
    engine_id: Option<Uuid>,
    warehouse_id: Option<Uuid>,
    subject: String,
    message: String,
    #[schema(value_type = Object)]
    detail: serde_json::Value,
    acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct MigrationReportQuery {
    /// Include rows already acknowledged.
    #[serde(default)]
    all: bool,
}

async fn engine_or_404(st: &CpState, id: Uuid) -> Result<crate::Engine, CpError> {
    st.engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)
}

async fn attachment_dto(
    st: &CpState,
    engine_id: Uuid,
    catalog_id: Uuid,
) -> Result<CatalogAttachmentDto, CpError> {
    st.engine_mgmt
        .registry()
        .list_catalog_attachments(catalog_id)
        .await?
        .into_iter()
        .find(|r| r.engine_id == engine_id)
        .map(Into::into)
        .ok_or(CpError::NotFound)
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Registry catalog id"),
    ),
    responses(
        (status = 201, description = "Attached (an egret warehouse catalog mints a Catalog Client for the engine). For Trino the apply happened first; for StarRocks the attachment is recorded first and converged after, so a 201 may carry `applyStatus: failed` with the engine's error — the Retry route converges it (SP3b D1)", body = CatalogAttachmentDto),
        (status = 400, description = "The engine cannot read this catalog, the engine reserves this catalog name, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog", body = ErrorBody),
        (status = 409, description = "Already attached, or the engine is being deleted", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply (`code` = `engine_cluster_apply_failed`); nothing was attached", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(crate) async fn attach_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<CatalogAttachmentDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = engine_or_404(&st, id).await?;
    let catalog = load(&st, cid).await?;
    attach_to_engine(&st, &engine, &catalog, None)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok((
        StatusCode::CREATED,
        Json(attachment_dto(&st, engine.id, cid).await?),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}/apply",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Registry catalog id"),
    ),
    responses(
        (status = 200, description = "Re-applied; the attachment is `applied`", body = ApplyOutcomeDto),
        (status = 400, description = "The change could not be assembled, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such engine, or the catalog is not attached to it", body = ErrorBody),
        (status = 409, description = "The engine is being deleted", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply; the attachment stays `failed` with the new error", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn retry_catalog_apply(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApplyOutcomeDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let engine = engine_or_404(&st, id).await?;
    st.engine_mgmt
        .registry()
        .get_attachment(engine.id, cid)
        .await?
        .ok_or(CpError::NotFound)?;
    retry_apply(&st, &engine, cid)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok(Json(ApplyOutcomeDto {
        engine_id: engine.id,
        engine_name: engine.name,
        status: "applied".into(),
        error: None,
    }))
}

async fn workspace_dtos(st: &CpState, id: Uuid) -> Result<Vec<CatalogWorkspaceDto>, CpError> {
    Ok(st
        .engine_mgmt
        .registry()
        .list_catalog_workspaces(id)
        .await?
        .into_iter()
        .map(|w| CatalogWorkspaceDto {
            workspace_id: w.workspace_id,
            name: w.name,
            slug: w.slug,
            assigned_at: w.assigned_at,
        })
        .collect())
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/{id}/workspaces",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "Workspaces the catalog is assigned to", body = Vec<CatalogWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_catalog_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<CatalogWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    load(&st, id).await?;
    Ok(Json(workspace_dtos(&st, id).await?))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/catalogs/{id}/workspaces",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    request_body = SetCatalogWorkspacesBody,
    responses(
        (status = 200, description = "The full assignment set was replaced. Assignment decides which catalogs a workspace sees in its engine view (GET /api/ws/{workspace}/engines/{engine_id}), alongside the fallback that shows an unassigned warehouse catalog to the workspace that owns its warehouse; it does not restrict what an engine serves. Notebook warehouse selection is deliberately owner-only and ignores assignment — an assigned workspace gets its own notebook role and namespace in SP4", body = Vec<CatalogWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog or workspace (nothing changed)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_catalog_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<SetCatalogWorkspacesBody>,
) -> Result<Json<Vec<CatalogWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    load(&st, id).await?;
    let mut ids = b.workspace_ids;
    ids.sort();
    ids.dedup();
    if !st.engine_mgmt.missing_workspace_ids(&ids).await?.is_empty() {
        return Err(CpError::NotFound);
    }
    st.engine_mgmt
        .registry()
        .set_catalog_workspaces(id, &ids)
        .await?;
    Ok(Json(workspace_dtos(&st, id).await?))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/catalogs/migration-report",
    tag = "control-plane-catalog-registry",
    params(MigrationReportQuery),
    responses(
        (status = 200, description = "What the registry migration did, oldest first (unacknowledged rows unless `all`)", body = Vec<MigrationReportRowDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_migration_report(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<MigrationReportQuery>,
) -> Result<Json<Vec<MigrationReportRowDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let rows = st.engine_mgmt.registry().list_report(q.all).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MigrationReportRowDto {
                id: r.id,
                run_id: r.run_id,
                created_at: r.created_at,
                severity: r.severity,
                code: r.code,
                catalog_id: r.catalog_id,
                engine_id: r.engine_id,
                warehouse_id: r.warehouse_id,
                subject: r.subject,
                message: r.message,
                detail: r.detail,
                acknowledged_at: r.acknowledged_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs/migration-report/acknowledge",
    tag = "control-plane-catalog-registry",
    responses(
        (status = 204, description = "Every open report row acknowledged"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn acknowledge_migration_report(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    st.engine_mgmt.registry().acknowledge_report().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_registry_catalogs,
        create_registry_catalog,
        get_registry_catalog,
        update_registry_catalog,
        delete_registry_catalog,
        catalog_impact,
        reapply_registry_catalog,
        attach_catalog,
        retry_catalog_apply,
        list_catalog_workspaces,
        set_catalog_workspaces,
        get_migration_report,
        acknowledge_migration_report,
    ),
    components(schemas(
        RegistryCatalogDto,
        CatalogAttachmentDto,
        CreateRegistryCatalogBody,
        UpdateRegistryCatalogBody,
        ApplyOutcomeDto,
        CatalogUpdateResultDto,
        CatalogImpactDto,
        CatalogImpactEngineDto,
        CatalogConflictBody,
        CatalogConflictEngine,
        CatalogConflictWorkspace,
        SetCatalogWorkspacesBody,
        CatalogWorkspaceDto,
        MigrationReportRowDto,
        ErrorBody,
    )),
    tags((name = "control-plane-catalog-registry", description = "Catalog registry: platform catalogs, their engine attachments and workspace assignment")),
)]
pub struct CatalogRegistryApi;
