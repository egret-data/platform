//! Workspace-scoped management routes for flow drafts and their published
//! versions (W1 Task 7, spec §10).
//!
//! ```text
//! GET    /api/ws/{workspace}/workflows/flows
//! POST   /api/ws/{workspace}/workflows/flows
//! GET    /api/ws/{workspace}/workflows/flows/{id}
//! PUT    /api/ws/{workspace}/workflows/flows/{id}
//! DELETE /api/ws/{workspace}/workflows/flows/{id}
//! POST   /api/ws/{workspace}/workflows/flows/{id}/validate
//! POST   /api/ws/{workspace}/workflows/flows/{id}/publish
//! GET    /api/ws/{workspace}/workflows/flows/{id}/versions
//! GET    /api/ws/{workspace}/workflows/flows/{id}/versions/{version}
//! ```
//!
//! Mounted under `/ws/{workspace}/workflows/...` here, **not**
//! `/api/ws/...` — `egret-server` nests this router under `/api`
//! (`crates/egret-server/src/lib.rs`, beside `egret_dataapi::management_router()`),
//! and `WsContext`'s feature-entitlement gate only recognises a path
//! beginning `/api/ws/` or `/ws/` (`crates/egret-dataapi/src/routes.rs:2307`'s
//! own comment states the same convention; `every_workspace_scoped_route_is_
//! mounted_under_api_ws` in `egret-server` holds it to CI). The `workflows`
//! feature key W0 registered claims the `workflows` prefix
//! (`egret-workspace/src/features.rs`) — these are the first routes ever to
//! land under it, so `the_workflows_entitlement_gates_these_routes` (this
//! crate's `tests/routes.rs`) is what first proves the rule actually bites
//! rather than sitting inert.
//!
//! **Two permissions, flat, following how `egret-dataapi`'s handlers do
//! it:** [`WF_READ`] for every route that only looks, [`WF_MANAGE`] for
//! every route that writes (including `/validate`, which runs no query
//! against anything but IS an authoring action — same reasoning
//! `data_api_endpoint:manage` gates `POST .../validate` in `egret-dataapi`).
//! egret's permissions never imply one another, so a role holding only
//! `:read` cannot create, edit, delete, validate, or publish a flow.
//!
//! **`POST .../validate` parses and validates the REQUEST BODY, not the
//! stored draft** — spec §10's whole reason for the route existing
//! separately from `PUT`: the editor calls it on every keystroke-pause, and
//! it must not require the in-progress edit to be saved first. `{id}` still
//! has to name a real flow in the caller's workspace (a 404 for a missing or
//! foreign one, like every other route here) — it scopes WHO may ask the
//! question, not WHAT gets validated.
//!
//! **`GET /flows/{id}` and `GET /flows` both recompute the validation report
//! fresh from the stored `source` on every read**, rather than storing one:
//! a draft's source can change between reads (`PUT`), and a stale report
//! would tell an author their flow still has an error they already fixed —
//! or hide one they just introduced.

use std::sync::Arc;

use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_core::{Ack, Error, Result as EgretResult, WorkspacePath};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::repo::{
    CreateFlowInput, FlowRepo, FlowRow, FlowVersionRow, ListFilter, UpdateFlowInput,
};
use crate::spec::{parse, validate, Finding, ValidationReport};
use egret_workflow_api::WorkflowRefPort;

/// View flow drafts, their validation report, and their published versions.
pub const WF_READ: &str = "workflow:read";
/// Create, edit, publish and delete flow drafts. Implies no `:read` — a
/// role holding only `:manage` could author flows it could never list.
pub const WF_MANAGE: &str = "workflow:manage";

/// Handler error type. No domain-specific failure mode of its own — every
/// arm below is already an [`egret_core::Error`], which carries the
/// canonical HTTP mapping. Same choice `egret-dataapi`'s `DataApiError`
/// makes, for the same reason.
pub type WorkflowError = Error;

/// State for [`management_router`].
#[derive(Clone)]
pub struct WorkflowState {
    pub repo: FlowRepo,
    /// The seam `publish` resolves every reference through — see
    /// [`crate::refs::WorkflowRefs`] for the real implementation and which
    /// `RefKind`s it can and cannot answer for today.
    pub refs: Arc<dyn WorkflowRefPort>,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: Arc<dyn egret_core::authz::Pdp>,
    /// The append-only audit trail — the same sink every other management
    /// seam writes mutations to.
    pub audit: egret_audit::AuditSink,
}

impl FromRef<WorkflowState> for SessionService {
    fn from_ref(s: &WorkflowState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<WorkflowState> for WorkspaceRepo {
    fn from_ref(s: &WorkflowState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<WorkflowState> for Authorizer {
    fn from_ref(s: &WorkflowState) -> Self {
        s.authz.clone()
    }
}

// ---------- DTOs ----------

/// A validation finding, over the wire. Mirrors [`Finding`] field for field;
/// a separate type only because `Finding::code` is `&'static str` and this
/// crate's DTOs derive `utoipa::ToSchema`, which `Finding` itself does not
/// (it is a `models`/`spec` type and must stay HTTP-agnostic, per this
/// crate's own module boundary — see `lib.rs`'s doc comment).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FindingDto {
    pub code: String,
    pub message: String,
    pub path: String,
}

impl From<&Finding> for FindingDto {
    fn from(f: &Finding) -> Self {
        Self {
            code: f.code.to_string(),
            message: f.message.clone(),
            path: f.path.clone(),
        }
    }
}

/// A flow's validation report, over the wire. Errors block publish; warnings
/// do not — see [`ValidationReport`]'s own doc comment.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ValidationReportDto {
    pub errors: Vec<FindingDto>,
    pub warnings: Vec<FindingDto>,
}

impl From<&ValidationReport> for ValidationReportDto {
    fn from(r: &ValidationReport) -> Self {
        Self {
            errors: r.errors.iter().map(FindingDto::from).collect(),
            warnings: r.warnings.iter().map(FindingDto::from).collect(),
        }
    }
}

/// A flow draft, over the wire — everything `GET /flows/{id}` promises: the
/// source, the stored plan, and a validation report computed fresh from
/// `source` (see this module's doc comment on why it is never stored).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowDto {
    pub id: Uuid,
    pub folder_id: Option<Uuid>,
    pub namespace: String,
    pub flow_id: String,
    pub source: String,
    #[schema(value_type = Object)]
    pub plan: serde_json::Value,
    pub state: String,
    pub live_version_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub validation: ValidationReportDto,
}

/// A flow draft as `GET /flows` lists it — no `source`/`plan`/`validation`:
/// re-parsing and re-validating every draft in the workspace on every list
/// call is the wrong cost to pay for a list view, and the list route was
/// never asked to answer "is this flow currently valid" (`GET /flows/{id}`
/// is).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowListItemDto {
    pub id: Uuid,
    pub folder_id: Option<Uuid>,
    pub namespace: String,
    pub flow_id: String,
    pub state: String,
    pub live_version_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<FlowRow> for FlowListItemDto {
    fn from(r: FlowRow) -> Self {
        Self {
            id: r.id,
            folder_id: r.folder_id,
            namespace: r.namespace,
            flow_id: r.flow_id,
            state: r.state,
            live_version_id: r.live_version_id,
            created_by: r.created_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// A published version, over the wire.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowVersionDto {
    pub id: Uuid,
    pub flow_id: Uuid,
    pub version: i32,
    #[schema(value_type = Object)]
    pub snapshot: serde_json::Value,
    pub note: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl From<FlowVersionRow> for FlowVersionDto {
    fn from(r: FlowVersionRow) -> Self {
        Self {
            id: r.id,
            flow_id: r.flow_id,
            version: r.version,
            snapshot: r.snapshot,
            note: r.note,
            created_by: r.created_by,
            created_at: r.created_at,
        }
    }
}

/// `GET /flows`'s query parameters — folder, namespace, and state filters
/// (spec §10), each optional and each ANDed with the workspace predicate
/// [`FlowRepo::list`] itself carries. Never a way to reach another
/// workspace's flows: every filter narrows within `ctx.workspace_id`, and
/// there is no filter that widens past it.
#[derive(Debug, Deserialize)]
pub struct ListFlowsQuery {
    pub folder_id: Option<Uuid>,
    pub namespace: Option<String>,
    pub state: Option<String>,
}

/// `POST /flows` body.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateFlowRequest {
    /// The flow's YAML, byte for byte. `namespace`/`flow_id` are DERIVED from
    /// it (parsed, never accepted as separate body fields) so the stored
    /// identity can never disagree with what the YAML itself declares.
    pub source: String,
    pub folder_id: Option<Uuid>,
}

/// `PUT /flows/{id}` body — a full replace, following the route's verb: the
/// whole draft (source, and therefore its derived namespace/flow_id/plan) is
/// replaced by what is sent, and `folder_id` is set to exactly what is sent
/// (including `None`, which clears it).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateFlowRequest {
    pub source: String,
    pub folder_id: Option<Uuid>,
}

/// `POST /flows/{id}/validate` body — the UNSAVED source being checked. See
/// this module's doc comment for why this is a body field and not simply
/// "validate the stored draft".
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ValidateFlowRequest {
    pub source: String,
}

/// `POST /flows/{id}/publish` body. Optional in the request (a bare `POST`
/// with no body publishes with an empty note), following
/// `PublishEndpointInput`'s own optionality in `egret-dataapi`.
#[derive(Debug, Deserialize, utoipa::ToSchema, Default)]
pub struct PublishFlowRequest {
    #[serde(default)]
    pub note: String,
}

/// Parse `source`, run [`validate`], and build the wire DTO for a stored
/// draft row. A parse failure here is a bug, not a caller error — `source`
/// only reaches storage after successfully parsing in [`create_flow`]/
/// [`update_flow`] — so it is mapped to `Error::Internal` rather than
/// `InvalidInput`, which would misattribute a storage-layer inconsistency to
/// the current request.
fn draft_dto(row: FlowRow) -> EgretResult<FlowDto> {
    let parsed = parse(&row.source).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "stored draft {} no longer parses: {} (line {:?}, column {:?})",
            row.id,
            e.message,
            e.line,
            e.column
        ))
    })?;
    let report = validate(&parsed);
    Ok(FlowDto {
        id: row.id,
        folder_id: row.folder_id,
        namespace: row.namespace,
        flow_id: row.flow_id,
        source: row.source,
        plan: row.plan,
        state: row.state,
        live_version_id: row.live_version_id,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        validation: ValidationReportDto::from(&report),
    })
}

/// Parse `source` into `(Flow, plan JSON, namespace, flow_id)`, mapping a
/// syntax error to `Error::InvalidInput` with its line/column — the same
/// conversion `publish.rs` step 2 uses, reused here because `create`/
/// `update` need the identical shape of failure for the identical reason:
/// namespace/flow_id are `NOT NULL` columns, so a draft that does not even
/// parse cannot be stored at all.
fn parse_draft(source: &str) -> EgretResult<(String, String, serde_json::Value)> {
    let flow = parse(source).map_err(|e| {
        Error::InvalidInput(match (e.line, e.column) {
            (Some(l), Some(c)) => format!("{} (line {l}, column {c})", e.message),
            _ => e.message,
        })
    })?;
    let plan = serde_json::to_value(&flow)
        .map_err(|e| Error::Internal(anyhow::anyhow!("serializing parsed flow: {e}")))?;
    Ok((flow.namespace.clone(), flow.id.clone(), plan))
}

// ---------- Handlers ----------

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows",
    tag = "workflow",
    params(
        WorkspacePath,
        ("folder_id" = Option<Uuid>, Query, description = "List only flows in this folder"),
        ("namespace" = Option<String>, Query, description = "List only flows in this namespace"),
        ("state" = Option<String>, Query, description = "List only flows in this state (draft, live, disabled)"),
    ),
    responses(
        (status = 200, description = "Flow drafts in the workspace, newest first", body = Vec<FlowListItemDto>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_flows(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListFlowsQuery>,
) -> EgretResult<Json<Vec<FlowListItemDto>>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let filter = ListFilter {
        folder_id: q.folder_id,
        namespace: q.namespace,
        state: q.state,
        trashed: false,
    };
    let rows = st.repo.list(ctx.workspace_id.0, &filter).await?;
    Ok(Json(rows.into_iter().map(FlowListItemDto::from).collect()))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows",
    tag = "workflow",
    params(WorkspacePath),
    request_body = CreateFlowRequest,
    responses(
        (status = 201, description = "Draft created, with its validation report", body = FlowDto),
        (status = 400, description = "The YAML does not parse", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "A flow with this namespace and id already exists in the workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn create_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Json(body): Json<CreateFlowRequest>,
) -> EgretResult<(StatusCode, Json<FlowDto>)> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let (namespace, flow_id, plan) = parse_draft(&body.source)?;
    let created = st
        .repo
        .create(
            ctx.workspace_id.0,
            ctx.principal.user_id,
            &CreateFlowInput {
                namespace,
                flow_id,
                source: body.source,
                plan,
                folder_id: body.folder_id,
            },
        )
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:create",
        "workflow_flow",
        Some(created.id.to_string()),
        serde_json::json!({
            "namespace": created.namespace,
            "flow_id": created.flow_id,
        }),
    ));
    Ok((StatusCode::CREATED, Json(draft_dto(created)?)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows/{id}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "The draft, with its validation report", body = FlowDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<FlowDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let row = st.repo.get(ctx.workspace_id.0, id).await?;
    Ok(Json(draft_dto(row)?))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/flows/{id}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    request_body = UpdateFlowRequest,
    responses(
        (status = 200, description = "Draft updated, with its validation report", body = FlowDto),
        (status = 400, description = "The YAML does not parse", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "Another flow already claims this namespace and id in the workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn update_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<UpdateFlowRequest>,
) -> EgretResult<Json<FlowDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let (namespace, flow_id, plan) = parse_draft(&body.source)?;
    let updated = st
        .repo
        .update(
            ctx.workspace_id.0,
            id,
            ctx.principal.user_id,
            &UpdateFlowInput {
                namespace: Some(namespace),
                flow_id: Some(flow_id),
                source: Some(body.source),
                plan: Some(plan),
                folder_id: Some(body.folder_id),
            },
        )
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:update",
        "workflow_flow",
        Some(id.to_string()),
        serde_json::json!({
            "namespace": updated.namespace,
            "flow_id": updated.flow_id,
        }),
    ));
    Ok(Json(draft_dto(updated)?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/flows/{id}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "Draft soft-deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn delete_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<Ack>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    st.repo.soft_delete(ctx.workspace_id.0, id).await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:delete",
        "workflow_flow",
        Some(id.to_string()),
        serde_json::json!({}),
    ));
    Ok(Json(Ack::ok()))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/{id}/validate",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id — scopes the request to this workspace, but the YAML checked is the request body, not the stored draft"),
    ),
    request_body = ValidateFlowRequest,
    responses(
        (status = 200, description = "The validation report — never blocks on errors, since this route is called on unsaved edits", body = ValidationReportDto),
        (status = 400, description = "The YAML does not parse", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn validate_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<ValidateFlowRequest>,
) -> EgretResult<Json<ValidationReportDto>> {
    // `:manage`, not `:read` -- an authoring action, exactly like
    // `data_api_endpoint:manage` gating `POST .../validate` in `egret-dataapi`.
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    // Containment only: `id` must name a flow in this workspace, but nothing
    // about IT is read below -- the source checked is the body's, per this
    // module's doc comment.
    let _ = st.repo.get(ctx.workspace_id.0, id).await?;
    let flow = parse(&body.source).map_err(|e| {
        Error::InvalidInput(match (e.line, e.column) {
            (Some(l), Some(c)) => format!("{} (line {l}, column {c})", e.message),
            _ => e.message,
        })
    })?;
    let report = validate(&flow);
    Ok(Json(ValidationReportDto::from(&report)))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/{id}/publish",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    request_body = PublishFlowRequest,
    responses(
        (status = 200, description = "A new immutable version was frozen and is now live", body = FlowVersionDto),
        (status = 400, description = "The draft does not parse or validate, or a task names an unknown type", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace, or a task references an object that does not exist or belongs to another workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn publish_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    body: Option<Json<PublishFlowRequest>>,
) -> EgretResult<Json<FlowVersionDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let note = body.map(|Json(b)| b.note).unwrap_or_default();
    let version = crate::publish::publish(
        &st.repo,
        &*st.refs,
        ctx.workspace_id.0,
        ctx.principal.user_id,
        id,
        &note,
    )
    .await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:publish",
        "workflow_flow",
        Some(id.to_string()),
        serde_json::json!({ "version": version.version }),
    ));
    Ok(Json(FlowVersionDto::from(version)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows/{id}/versions",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "Published versions, newest first", body = Vec<FlowVersionDto>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_flow_versions(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<Vec<FlowVersionDto>>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    // Containment first, and DELIBERATELY a separate call from the list
    // below: `FlowRepo::list_versions` alone cannot tell "no such flow" from
    // "this flow has zero versions" (both answer an empty `Vec` — its own
    // doc comment says so), and this route's 404 contract must be able to.
    let _ = st.repo.get(ctx.workspace_id.0, id).await?;
    let rows = st.repo.list_versions(ctx.workspace_id.0, id).await?;
    Ok(Json(rows.into_iter().map(FlowVersionDto::from).collect()))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows/{id}/versions/{version}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("version" = i32, Path, description = "Version number"),
    ),
    responses(
        (status = 200, description = "One published version's frozen snapshot", body = FlowVersionDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow, or no such version of it, in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_flow_version(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, version)): Path<(String, Uuid, i32)>,
) -> EgretResult<Json<FlowVersionDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let row = st.repo.get_version(ctx.workspace_id.0, id, version).await?;
    Ok(Json(FlowVersionDto::from(row)))
}

/// The Workflow management router. Mount under `/api` so the live paths are
/// `/api/ws/{workspace}/workflows/...` — see this module's doc comment for
/// the feature-gate convention that depends on it.
pub fn management_router() -> Router<WorkflowState> {
    Router::new()
        .route(
            "/ws/{workspace}/workflows/flows",
            get(list_flows).post(create_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}",
            get(get_flow).put(update_flow).delete(delete_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/validate",
            post(validate_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/publish",
            post(publish_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/versions",
            get(list_flow_versions),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/versions/{version}",
            get(get_flow_version),
        )
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_flows,
        create_flow,
        get_flow,
        update_flow,
        delete_flow,
        validate_flow,
        publish_flow,
        list_flow_versions,
        get_flow_version,
    ),
    components(schemas(
        FindingDto,
        ValidationReportDto,
        FlowDto,
        FlowListItemDto,
        FlowVersionDto,
        CreateFlowRequest,
        UpdateFlowRequest,
        ValidateFlowRequest,
        PublishFlowRequest,
    )),
    tags((name = "workflow", description = "Flow drafts and published versions (W1)"))
)]
struct WorkflowApi;

/// This crate's OpenAPI document, merged into the aggregated one by
/// `egret-server`'s `openapi_doc()` beside every other seam's.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    WorkflowApi::openapi()
}
