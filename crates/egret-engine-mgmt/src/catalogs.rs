//! Catalog registry routes (SP3 §4.4–4.7): platform catalogs, the engines they
//! are attached to, and their workspace assignment. Merged into the engines
//! router, so mounted under `/api/control-plane`.

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
    attach_to_engine, detach_from_engine, propagate, retry_apply, validate_references,
    ApplyOutcome, CatalogOpError,
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
        .route("/catalogs/{id}/test", post(test_registry_catalog))
        .route(
            "/catalogs/{id}/workspaces",
            get(list_catalog_workspaces).put(set_catalog_workspaces),
        )
        .route(
            "/catalogs/{id}/engines/{engine_id}",
            post(registry_attach_catalog).delete(detach_catalog),
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
    /// `applied` | `failed` | `pending`. `pending` means egret has recorded the
    /// attachment and has not yet been able to tell the engine — a StarRocks
    /// engine whose FE is still coming up (5-10 minutes after create), which
    /// the bootstrap or a Retry converges.
    apply_status: String,
    apply_error: Option<String>,
    applied_at: Option<DateTime<Utc>>,
    /// This catalog is that engine's PRIMARY: it leads the engine's render
    /// order and cannot be detached or edited. At most one of an engine's
    /// catalogs carries it (SP3c D2, migration 0150).
    is_primary: bool,
    /// When this attachment was recorded — `engine_catalog_attachments.created_at`.
    ///
    /// The clock a `pending` attachment has been waiting on. `pending` is
    /// written only by the INSERT that creates the row (engine create and
    /// attach; nothing flips a settled attachment back), so this is the moment
    /// the wait began. A UI deciding whether a `pending` has outlived the
    /// StarRocks bootstrap must measure against THIS and not against the
    /// catalog's `updatedAt`: attaching an existing catalog never touches the
    /// catalog row, so its `updatedAt` is already arbitrarily old.
    created_at: DateTime<Utc>,
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
            is_primary: r.is_primary,
            created_at: r.created_at,
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
    /// Why this row's stored `config` no longer parses, if it does not.
    /// A stored row the tightened validator now refuses stays visible and
    /// repairable instead of vanishing — and, critically, never fails the
    /// whole list (SP3c-1 review N1).
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
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
    /// `applied` | `failed` | `skipped`. `skipped`: the engine is external, so
    /// egret did not touch it (the attachment is only declared); `error` then
    /// says so.
    status: String,
    error: Option<String>,
}

impl From<ApplyOutcome> for ApplyOutcomeDto {
    fn from(o: ApplyOutcome) -> Self {
        ApplyOutcomeDto {
            engine_id: o.engine_id,
            engine_name: o.engine_name,
            status: if o.skipped {
                "skipped"
            } else if o.applied {
                "applied"
            } else {
                "failed"
            }
            .into(),
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
    // Recorded, never propagated: one stored row the tightened validator
    // refuses must not fail this whole list with a 400 (SP3c-1 review N1).
    let config_error = row.def().err().map(|e| e.to_string());
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
        config_error,
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
        (status = 200, description = "Registry catalogs with their engines and workspaces. A catalog whose stored configuration no longer parses is listed with `configError` set rather than omitted, and never fails the whole list", body = Vec<RegistryCatalogDto>),
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
        (status = 200, description = "The catalog. If its stored configuration no longer parses, `configError` is set rather than the request failing", body = RegistryCatalogDto),
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
    registry
        .delete_catalog_with_compiler(id, st.catalog_assignment_compiler.as_deref())
        .await?;
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
        (status = 200, description = "The unchanged configuration re-applied to every attached engine (e.g. after rotating a secret). An external engine is listed with status `skipped`: egret does not apply catalogs to it", body = Vec<ApplyOutcomeDto>),
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

/// Which of the three checks produced a [`CatalogTestResultDto`]. An enum, not
/// a string, so Task 7's renderer gets a union it must switch on exhaustively:
/// the whole point of the discriminant is that the three cannot be blurred, and
/// a doc comment saying "read this first" is a comment doing a type's job (SP3c
/// review F4). The wire values are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogCheck {
    /// `SHOW SCHEMAS FROM <name>` ran on an attached Trino engine that has this
    /// catalog applied, and the catalog answered — credentials included.
    Query,
    /// A connect-only reachability check of the host and port in the STORED
    /// configuration. It proves the port answers. It proves NOTHING about
    /// credentials, TLS or permissions.
    Tcp,
    /// Nothing was measured: no Trino engine could run the query, and there was
    /// no host to dial either. Render it neutral, not red — a `tpch` catalog
    /// with nothing attached is not broken. `message` says which of the two
    /// reasons applies, because "this kind has no endpoint" and "this
    /// catalog's URI names no host" call for different fixes.
    Unsupported,
}

/// `POST /catalogs/{id}/test` payload.
///
/// Every field a UI author must read is a schema property, so its doc comment
/// reaches the generated client as JSDoc — a response description does not.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogTestResultDto {
    /// Whether the check that ran succeeded — and it means only as much as
    /// `check` says it does. Read `check` FIRST: `unsupported` is always
    /// `ok:false` because nothing was measured, and that is not a broken
    /// catalog, so it renders neutral rather than red.
    ok: bool,
    /// Which check ran, so a green tick can never be read as the other one.
    check: CatalogCheck,
    /// The engine the query ran on; null for `tcp` and `unsupported`.
    engine_name: Option<String>,
    /// For `tcp`, the `host:port` that was dialled, so the result names what it
    /// measured; null otherwise.
    target: Option<String>,
    /// One sentence an operator can act on. For `tcp` it always says, in the
    /// sentence itself, that reachability is all that was measured.
    message: String,
}

/// Why no query ran, in words that fit in front of a weaker check's sentence.
/// Empty when the catalog is attached to no Trino engine at all, which needs no
/// explaining; otherwise it names the engine and what is wrong with it, because
/// "no Trino engine is attached" would be false in those cases (SP3c review
/// F3, F7).
fn why_no_query(reason: Option<String>) -> String {
    reason.map(|r| format!("{r} ")).unwrap_or_default()
}

#[utoipa::path(
    post,
    path = "/api/control-plane/catalogs/{id}/test",
    tag = "control-plane-catalog-registry",
    params(("id" = Uuid, Path, description = "Registry catalog id")),
    responses(
        (status = 200, description = "The verdict, and `check` names which of the three checks produced it. The target of a `tcp` check comes from the STORED configuration and never from the request, which carries no body at all", body = CatalogTestResultDto),
        (status = 400, description = "The catalog's STORED configuration does not parse, so there was no target to decide and nothing to test. Reachable for a row written before a validation rule that has since tightened — the message names the field, and this is the one thing the operator can act on, so render it rather than the generic error", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 502, description = "`query` only: the Trino coordinator could not be reached at all. A coordinator that answers with a failure is a 200 with `ok:false`", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn test_registry_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<CatalogTestResultDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let row = load(&st, id).await?;
    // D4: a Trino attachment answers the better question, so prefer it. Today's
    // per-engine test route reaches only Trino (`monitor::coordinator_call`,
    // fallback service `{name}-trino`), so a StarRocks-only catalog falls back
    // to the probe until SP4 — which is a narrower answer, not a wrong one, and
    // `check` is what keeps it honest.
    let trino: Vec<_> = st
        .engine_mgmt
        .registry()
        .list_catalog_attachments(id)
        .await?
        .into_iter()
        .filter(|a| {
            a.engine_type == "trino"
                && a.apply_status != egret_catalog_registry::store::apply_status::DECLARED
        })
        .collect();
    // An APPLIED attachment, not merely the alphabetically first one
    // (`list_catalog_attachments` is `ORDER BY e.name`). Querying an engine the
    // catalog was never applied to answers "catalog not found" — a failure that
    // says nothing about the catalog and everything about the attachment, and
    // SP3c's own list page exists because `pending`/`failed` attachments are
    // expected (SP3c review F3).
    let applied = trino.iter().find(|a| a.apply_status == "applied");
    let mut no_query: Option<String> = None;
    if let Some(a) = applied {
        if let Some(engine) = st.engine_mgmt.get_engine(a.engine_id).await? {
            let (ok, message) =
                crate::engines::show_schemas_verdict(&st, &engine, &row.name).await?;
            return Ok(Json(CatalogTestResultDto {
                ok,
                check: CatalogCheck::Query,
                engine_name: Some(engine.name),
                target: None,
                message,
            }));
        }
        // The attachment row outlived its engine row. Fall through to the
        // weaker check rather than 500ing, and do not claim nothing is
        // attached.
        no_query = Some(format!(
            "The engine `{}` this catalog is applied to no longer exists, so egret could not run a query.",
            a.engine_name
        ));
    } else if let Some(a) = trino.first() {
        no_query = Some(format!(
            "No Trino engine has this catalog applied yet (`{}` is `{}`), so a query would have \
             tested the attachment rather than the catalog.",
            a.engine_name, a.apply_status
        ));
    }

    // Nothing to ask. The probe's target comes from the stored row and nowhere
    // else — see `probe.rs` and `CatalogDef::probe_target`.
    let def = CatalogDef::parse(&row.kind, &row.config, &row.advanced)
        .map_err(|e| CpError::BadRequest(e.to_string()))?;
    let why = why_no_query(no_query);
    let Some((host, port)) = def.probe_target() else {
        // Two different facts that call for different fixes: a kind with
        // nothing to dial is fine, a stored URI naming no host is broken.
        // `thrift_uri`/`http_uri` now require a host, so no VALID row reaches
        // the first arm any more — a row written before that rule fails
        // `CatalogDef::parse` above and is answered with the URI's own error,
        // which names the right problem. The arm stays as a standing guard: if
        // those validators are ever loosened, or a kind is added whose endpoint
        // can be unreadable, the false sentence must not come back silently
        // (SP3c review F2).
        let message = if def.names_an_endpoint() {
            format!(
                "{why}The endpoint stored for this catalog names no host egret can read, so there \
                 was nothing to dial. Fix the catalog's configuration."
            )
        } else {
            format!(
                "{why}A catalog of kind `{}` has no network endpoint egret can reach on its own, \
                 so there was nothing to check. Attach it to a Trino engine to test it.",
                row.kind
            )
        };
        return Ok(Json(CatalogTestResultDto {
            ok: false,
            check: CatalogCheck::Unsupported,
            engine_name: None,
            target: None,
            message,
        }));
    };
    let target = format!("{host}:{port}");
    let (ok, message) = match crate::probe::tcp_probe(&host, port).await {
        crate::probe::ProbeOutcome::Open => (
            true,
            format!(
                "{why}{target} accepted a TCP connection. This is a reachability check only — no \
                 protocol was spoken and no credentials were used, so it does not show that this \
                 catalog works."
            ),
        ),
        crate::probe::ProbeOutcome::Refused(e) => {
            (false, format!("{why}Could not reach {target}: {e}."))
        }
        crate::probe::ProbeOutcome::TimedOut => (
            false,
            format!(
                "{why}{target} did not answer within {}s.",
                crate::probe::PROBE_TIMEOUT.as_secs()
            ),
        ),
    };
    Ok(Json(CatalogTestResultDto {
        ok,
        check: CatalogCheck::Tcp,
        engine_name: None,
        target: Some(target),
        message,
    }))
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
        (status = 201, description = "Attached. Trino: the catalog was applied to the cluster first, so `applyStatus` is `applied`. StarRocks: the attachment is recorded first and the catalog is created on the FE immediately afterwards, so `applyStatus` is `applied` on success and `failed` (with `applyError`) when the FE could not be reached or refused — the attachment still exists and is converged by `POST …/catalogs/{cid}/apply` or by the engine's next bootstrap", body = CatalogAttachmentDto),
        (status = 400, description = "The engine cannot read this catalog, the name is one StarRocks reserves, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog", body = ErrorBody),
        (status = 409, description = "Already attached, or the engine is being deleted", body = ErrorBody),
        (status = 502, description = "Trino only: Kubernetes rejected the apply (`code` = `engine_cluster_apply_failed`); nothing was attached. A StarRocks FE that rejects the DDL is reported as a `failed` attachment in the 201 body, not as a 502", body = ErrorBody),
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
    path = "/api/control-plane/catalogs/{id}/engines/{engine_id}",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Registry catalog id"),
        ("engine_id" = Uuid, Path, description = "Engine id"),
    ),
    responses(
        (status = 201, description = "Attached. Trino: the catalog was applied to the cluster first, so `applyStatus` is `applied`. StarRocks: the attachment is recorded first and the catalog is created on the FE immediately afterwards, so `applyStatus` is `applied` on success and `failed` (with `applyError`) when the FE could not be reached or refused — the attachment still exists and is converged by `POST …/engines/{id}/catalogs/{cid}/apply` or by the engine's next bootstrap", body = CatalogAttachmentDto),
        (status = 400, description = "The engine cannot read this catalog, the name is one StarRocks reserves, or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog or engine", body = ErrorBody),
        (status = 409, description = "Already attached, or the engine is being deleted", body = ErrorBody),
        (status = 502, description = "Trino only: Kubernetes rejected the apply (`code` = `engine_cluster_apply_failed`); nothing was attached. A StarRocks FE that rejects the DDL is reported as a `failed` attachment in the 201 body, not as a 502", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn registry_attach_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, engine_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<CatalogAttachmentDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let engine = engine_or_404(&st, engine_id).await?;
    let catalog = load(&st, id).await?;
    attach_to_engine(&st, &engine, &catalog, None)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok((
        StatusCode::CREATED,
        Json(attachment_dto(&st, engine.id, id).await?),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/catalogs/{id}/engines/{engine_id}",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Registry catalog id"),
        ("engine_id" = Uuid, Path, description = "Engine id"),
    ),
    responses(
        (status = 204, description = "Removed from the engine, then detached. The registry catalog itself is kept, and the credential egret minted for this attachment is revoked"),
        (status = 400, description = "This is the engine's primary catalog (detach it by deleting the engine), or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog or engine, or the catalog is not attached to it", body = ErrorBody),
        (status = 409, description = "The engine is being deleted; retry", body = ErrorBody),
        (status = 502, description = "The engine rejected the removal; nothing was detached", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn detach_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, engine_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let engine = engine_or_404(&st, engine_id).await?;
    let catalog = load(&st, id).await?;
    detach_from_engine(&st, &engine, &catalog)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok(StatusCode::NO_CONTENT)
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
        (status = 200, description = "Re-applied; the attachment is `applied`. StarRocks: an absent catalog is created and an existing one is updated in place with `ALTER CATALOG … SET`", body = ApplyOutcomeDto),
        (status = 400, description = "Kubernetes is not configured (Trino), or the catalog cannot be rendered for this engine", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such engine, or the catalog is not attached to it", body = ErrorBody),
        (status = 409, description = "The engine is being deleted; The engine is external (`code` = `engine_is_external`): egret does not apply a catalog to it", body = ErrorBody),
        (status = 502, description = "The engine rejected the apply — Kubernetes for Trino, the FE for StarRocks (including a catalog of this name the FE already holds with different properties); the attachment stays `failed` with the new error", body = ErrorBody),
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
    // The assignment write and `perm`'s implicit-grant sync share ONE
    // transaction (SP4a Task 6, D3): an assignment that commits while its
    // grant sync fails is exactly the split-brain this exists to remove.
    // `sqlx::Error` from `.begin()`/`.commit()` is mapped explicitly here
    // (not through a blanket `From<sqlx::Error>`, Task 6 fix round 1 Minor)
    // so this is the one deliberate place this crate lets a raw sqlx error
    // become an HTTP one.
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    egret_catalog_registry::RegistryStore::set_catalog_workspaces_in_tx(&mut tx, id, &ids).await?;
    egret_perm::sync_assignment_grants_in_tx(&mut tx, id, &ids).await?;
    // SP4a Task 15 (D3 defect 1, live-gate measured): the grant row the sync
    // above just wrote/removed is not what the engine enforces until it is
    // compiled. Recompile every profile this catalog reaches in the SAME
    // transaction, so the assignment (or its removal) is live the instant
    // this commits — never a stale grant swept in only when some unrelated
    // write next recompiles the profile.
    if let Some(compiler) = &st.perm_compiler {
        compiler
            .recompile_catalog_assignment_in_tx(&mut tx, id)
            .await?;
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    Ok(Json(workspace_dtos(&st, id).await?))
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
        test_registry_catalog,
        attach_catalog,
        registry_attach_catalog,
        detach_catalog,
        retry_catalog_apply,
        list_catalog_workspaces,
        set_catalog_workspaces,
    ),
    components(schemas(
        RegistryCatalogDto,
        CatalogAttachmentDto,
        CreateRegistryCatalogBody,
        UpdateRegistryCatalogBody,
        ApplyOutcomeDto,
        CatalogTestResultDto,
        CatalogCheck,
        CatalogUpdateResultDto,
        CatalogImpactDto,
        CatalogImpactEngineDto,
        CatalogConflictBody,
        CatalogConflictEngine,
        CatalogConflictWorkspace,
        SetCatalogWorkspacesBody,
        CatalogWorkspaceDto,
        ErrorBody,
    )),
    tags((name = "control-plane-catalog-registry", description = "Catalog registry: platform catalogs, their engine attachments and workspace assignment")),
)]
pub struct CatalogRegistryApi;

#[cfg(test)]
mod openapi_tests {
    use super::*;
    use egret_catalog_registry::CatalogKind;

    /// `CreateRegistryCatalogBody.kind` is a plain `string` whose permitted
    /// values exist only in its description. That sentence is therefore the
    /// contract: `egret-server`'s `committed_openapi_json_is_current` copies it
    /// into the repo-root `openapi.json`, orval copies it from there into the
    /// generated model's JSDoc, and `CatalogCreate.test.tsx` pins the create
    /// form's kind list to that JSDoc. This is the one link in that chain that
    /// nothing else checks: the description against the kinds the backend
    /// actually accepts.
    ///
    /// Written as an equality against `CatalogKind::ALL` rather than a "each
    /// documented kind parses" loop, because the failure that matters is a kind
    /// the backend accepts and the description omits — the UI then cannot offer
    /// it, and a loop over the description would never notice.
    #[test]
    fn the_kind_description_lists_exactly_the_kinds_the_backend_accepts() {
        use utoipa::OpenApi as _;
        let doc = serde_json::to_value(CatalogRegistryApi::openapi()).unwrap();
        let described = doc["components"]["schemas"]["CreateRegistryCatalogBody"]["properties"]
            ["kind"]["description"]
            .as_str()
            .expect("CreateRegistryCatalogBody.kind has a description");
        let expected = CatalogKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert_eq!(
            described, expected,
            "the documented kinds have drifted from CatalogKind::ALL — the create \
             form's kind list is generated from this sentence"
        );
    }
}
