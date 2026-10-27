//! Workspace-scoped management routes for flow drafts and their published
//! versions (W1 Task 7, spec §10).
//!
//! ```text
//! GET    /api/ws/{workspace}/workflows/schema
//! GET    /api/ws/{workspace}/workflows/flows
//! POST   /api/ws/{workspace}/workflows/flows
//! POST   /api/ws/{workspace}/workflows/flows/validate
//! GET    /api/ws/{workspace}/workflows/flows/{id}
//! PUT    /api/ws/{workspace}/workflows/flows/{id}
//! DELETE /api/ws/{workspace}/workflows/flows/{id}
//! POST   /api/ws/{workspace}/workflows/flows/{id}/validate
//! POST   /api/ws/{workspace}/workflows/flows/{id}/publish
//! GET    /api/ws/{workspace}/workflows/flows/{id}/versions
//! GET    /api/ws/{workspace}/workflows/flows/{id}/versions/{version}
//! GET    /api/ws/{workspace}/workflows/flows/{id}/triggers              (triggers::routes)
//! GET    /api/ws/{workspace}/workflows/triggers
//! PUT    /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled
//! PUT    /api/ws/{workspace}/workflows/triggers/disabled
//! POST   /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill   (triggers::backfill_routes)
//! PUT    /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill/paused
//! DELETE /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill
//! PUT    /api/ws/{workspace}/workflows/triggers/backfill/paused
//! POST   /api/ws/{workspace}/workflows/triggers/backfill/delete
//! POST   /api/ws/{workspace}/workflows/flows/graph
//! GET    /api/ws/{workspace}/workflows/flows/{id}/graph
//! POST   /api/ws/{workspace}/workflows/flows/{id}/executions
//! GET    /api/ws/{workspace}/workflows/executions
//! GET    /api/ws/{workspace}/workflows/executions/{id}
//! GET    /api/ws/{workspace}/workflows/executions/{id}/logs
//! GET    /api/ws/{workspace}/workflows/executions/{id}/file          (storage::routes)
//! GET    /api/ws/{workspace}/workflows/executions/{id}/file/metas
//! GET    /api/ws/{workspace}/workflows/executions/{id}/file/preview
//! POST   /api/ws/{workspace}/workflows/executions/{id}/resume
//! POST   /api/ws/{workspace}/workflows/executions/{id}/replay
//! POST   /api/ws/{workspace}/workflows/executions/{id}/kill
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
//! **A third, [`WF_USE_SECRETS`] (W5b fix round 1)**, on top of `:manage`
//! for the three routes that write a flow's source -- create, update,
//! publish -- when that source can call `secret()`. See
//! [`require_secret_use`].
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
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_core::{Ack, Error, Result as EgretResult, WorkspacePath};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::executor::RunState;
use crate::logs::{LogLevel, LogQuery, LogRepo, StoredLine};
use crate::repo::{
    CreateFlowInput, ExecutionDetail, ExecutionFilter, ExecutionRepo, ExecutionRow, FlowRepo,
    FlowRow, FlowVersionRow, ListFilter, Page, TaskRunRow, UpdateFlowInput, DEFAULT_PAGE_LIMIT,
    MAX_PAGE_LIMIT,
};
use crate::spec::{parse, validate_source, Finding, ValidationReport};
use crate::storage::multipart::{
    is_length_limit, is_multipart, multipart_body_limit, multipart_refusal, HeaderBudget,
    MultipartGate,
};
/// Moved to [`crate::storage::multipart`] (W9b Task 8); re-exported where
/// W9a's tests and callers name them.
pub use crate::storage::multipart::{
    EXPORT_TOTAL_TIMEOUT, MAX_MULTIPART_HEADER_BYTES, MULTIPART_OVERHEAD_BYTES,
    UPLOAD_IDLE_TIMEOUT, UPLOAD_TOTAL_TIMEOUT,
};
use egret_workflow_api::WorkflowRefPort;

/// View flow drafts, their validation report, and their published versions.
pub const WF_READ: &str = "workflow:read";
/// Create, edit, publish and delete flow drafts. Implies no `:read` — a
/// role holding only `:manage` could author flows it could never list.
pub const WF_MANAGE: &str = "workflow:manage";

/// Save or publish a flow whose content can call `secret()` (W5b fix round 1,
/// I3). Checked on EVERY write of a flow's source -- `create_flow`,
/// `update_flow` -- and at `publish_flow`, by [`require_secret_use`]: every
/// execution path runs the stored draft (`workflow.flows.source`), not a
/// published version, so a gate at publish alone would be walked round by
/// saving and starting. It is in addition to [`WF_MANAGE`], never instead.
pub const WF_USE_SECRETS: &str = "workflow:use_secrets";

/// The refusal [`require_secret_use`] answers, naming the permission.
pub const USE_SECRETS_REFUSAL: &str = "forbidden: this flow calls secret(), which reads the \
     workspace's secrets; saving or publishing it requires the permission \
     'workflow:use_secrets' (granted to workspace admins)";

/// The error of the three routes that write a flow's source: every
/// [`egret_core::Error`], plus a 403 that says which permission is missing
/// (`egret_core::Error::Forbidden`'s body is the bare word "forbidden").
#[derive(Debug)]
pub enum WriteError {
    Core(Error),
    /// The source calls `secret()` and the caller lacks [`WF_USE_SECRETS`].
    SecretsNotAllowed,
    /// `if_updated_at` no longer matches the draft (W7a spec §7.3): a 409
    /// with [`DRAFT_CHANGED_CODE`], so a client tells it from the other 409
    /// (a namespace and id another flow holds) without reading the prose.
    DraftChanged,
}

/// `ErrorBody.code` of [`WriteError::DraftChanged`].
pub const DRAFT_CHANGED_CODE: &str = "draft_changed";

impl From<Error> for WriteError {
    fn from(e: Error) -> Self {
        WriteError::Core(e)
    }
}

impl axum::response::IntoResponse for WriteError {
    fn into_response(self) -> axum::response::Response {
        match self {
            WriteError::Core(e) => e.into_response(),
            WriteError::SecretsNotAllowed => (
                StatusCode::FORBIDDEN,
                Json(egret_core::http::ErrorBody {
                    error: USE_SECRETS_REFUSAL.to_string(),
                    code: None,
                }),
            )
                .into_response(),
            WriteError::DraftChanged => (
                StatusCode::CONFLICT,
                Json(egret_core::http::ErrorBody {
                    error: Error::Conflict(crate::repo::DRAFT_CHANGED.into()).to_string(),
                    code: Some(DRAFT_CHANGED_CODE.to_string()),
                }),
            )
                .into_response(),
        }
    }
}

/// [`WF_USE_SECRETS`] when `source` can call `secret()`
/// ([`crate::runners::secrets::source_uses_secret`]), through the one PDP
/// every other gate here uses. A source that cannot is not asked about.
///
/// **Any** save of such a source needs it, not only one that adds the call:
/// an edit elsewhere in a flow that already reads a secret (its `url`, its
/// `sql`) can redirect the value just as well.
async fn require_secret_use(
    st: &WorkflowState,
    ctx: &impl egret_core::authz::AuthTarget,
    source: &str,
) -> std::result::Result<(), WriteError> {
    if !crate::runners::secrets::source_uses_secret(source) {
        return Ok(());
    }
    match egret_core::authz::require(&*st.pdp, ctx, WF_USE_SECRETS).await {
        Ok(()) => Ok(()),
        Err(Error::Forbidden) => Err(WriteError::SecretsNotAllowed),
        Err(e) => Err(WriteError::Core(e)),
    }
}

/// Handler error type. No domain-specific failure mode of its own — every
/// arm below is already an [`egret_core::Error`], which carries the
/// canonical HTTP mapping. Same choice `egret-dataapi`'s `DataApiError`
/// makes, for the same reason.
pub type WorkflowError = Error;

/// Which of the workflow loops this deployment runs, from `config.workflow`
/// (`scheduler_enabled`, `worker_enabled`, `webhooks_enabled`). The trigger
/// routes echo them so the UI can say a trigger cannot fire here (W7e spec
/// §3.5). The deployment sets them once for every replica.
#[derive(Debug, Clone, Copy, Default)]
pub struct TriggerFlags {
    pub scheduler_enabled: bool,
    pub worker_enabled: bool,
    pub webhooks_enabled: bool,
}

/// State for [`management_router`].
#[derive(Clone)]
pub struct WorkflowState {
    /// Whole-branch review, critical finding 1: `create_flow`/`update_flow`
    /// need to run [`scoped_folder`]'s own query against `analytics.folders`
    /// directly, the same reason `DataApiState` carries a `db` field beside
    /// `EndpointStore` -- `folder_id` names a table this crate otherwise
    /// never touches, so there is no repository method to route the check
    /// through instead.
    pub db: egret_metastore::Db,
    pub repo: FlowRepo,
    /// The READ layer over `workflow.executions` / `workflow.task_runs`
    /// (W10 Task 3). A field beside `repo` rather than an `ExecutionRepo::new`
    /// per request, for the reason `repo` is one: the handler must not have to
    /// reach into `db` to build it, and `db` is documented above as being here
    /// for `analytics.folders` and nothing else. It writes nothing -- the
    /// writes stay in `worker::claim`, which holds the `FOR NO KEY UPDATE` this type
    /// deliberately does not.
    pub executions: ExecutionRepo,
    /// The READ side of `workflow.task_logs` (W11): `GET …/executions/{id}/logs`
    /// pages it. A field for the reason `executions` is one. The writes are
    /// the log writer's (`logs::spawn_writer`), never a route's.
    pub logs: LogRepo,
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
    /// W5a: the platform half of the script-task gate publish runs
    /// (`workflow.plugin_configurations`, the default runner). The workspace
    /// half, `workflow_containers`, is read from `workspaces` per publish.
    pub scripts: crate::scripts::ScriptSettings,
    /// W9b Task 8: how long a namespace file upload's body may go without a
    /// byte arriving before the upload is refused with 408 --
    /// [`UPLOAD_IDLE_TIMEOUT`] in production; a field so a test can drive the
    /// refusal in milliseconds (fix round 1, I-2).
    pub upload_idle_timeout: std::time::Duration,
    /// W9b Task 8 re-review m-2: how long a namespace file upload's body may
    /// take in all before the upload is refused with 408 --
    /// [`UPLOAD_TOTAL_TIMEOUT`] (12 hours) in production; a field so a test
    /// can drive the refusal in milliseconds.
    pub upload_total_timeout: std::time::Duration,
    /// W9b Task 9 fix round 1, M-7: how long a namespace export's response
    /// body may take in all before it is aborted -- [`EXPORT_TOTAL_TIMEOUT`]
    /// (12 hours) in production; a field so a test can drive it in
    /// milliseconds.
    pub export_total_timeout: std::time::Duration,
    /// W7e: the loops this deployment runs, echoed by the trigger routes.
    pub trigger_flags: TriggerFlags,
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
    /// The creator's display name: the profile name, else the email. Null
    /// when `created_by` is null or the user no longer exists (W7a spec §7.4).
    pub created_by_name: Option<String>,
    /// The flow's highest version number; null when it has none. Versions
    /// are snapshots to compare and restore: executions run the saved draft
    /// (W7a spec §4, §5.3).
    pub latest_version: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub validation: ValidationReportDto,
    /// Whether the caller may save, publish, run and delete this flow: they
    /// hold `workflow:manage` in this workspace. An editor opens read-only
    /// when this is false; every mutating route still checks for itself.
    // W7a spec §5.2. The PDP's answer to the check the mutating routes make.
    // `workflow:manage` is workspace-scoped, so `/auth/me` cannot say it.
    pub can_manage: bool,
    /// Whether this deployment runs the workflow worker
    /// (`workflow.worker_enabled`, `EGRET_WORKFLOW__WORKER_ENABLED`; off by
    /// default). **False: an execution is created and stays `RUNNING`, and
    /// nothing runs it until an administrator turns the worker on** (KI-013).
    /// The same flag the trigger routes echo ([`TriggerFlags`]).
    pub worker_enabled: bool,
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
    /// The creator's display name: the profile name, else the email. Null
    /// when `created_by` is null or the user no longer exists (W7a spec §7.4).
    pub created_by_name: Option<String>,
    /// The flow's highest version number; null when it has none. Versions
    /// are snapshots to compare and restore: executions run the saved draft
    /// (W7a spec §4, §5.3).
    pub latest_version: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When the flow was moved to the trash; null for a flow that is not in
    /// it (KI-119: `GET /flows?trashed=true` lists only these).
    pub deleted_at: Option<DateTime<Utc>>,
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
            created_by_name: r.created_by_name,
            latest_version: r.latest_version,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
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
    /// The publisher's display name: the profile name, else the email. Null
    /// when `created_by` is null or the user no longer exists (W7a spec §7.4).
    pub created_by_name: Option<String>,
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
            created_by_name: r.created_by_name,
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
    /// `true` lists the trash instead (KI-119): only the flows moved there,
    /// never mixed with the others.
    #[serde(default)]
    pub trashed: bool,
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
    /// The draft's `updated_at` as the editor last read it (RFC 3339, the
    /// exact string `FlowDto.updated_at` carried -- microseconds included).
    /// When present and no longer the row's, the save is refused with 409
    /// and nothing is written; absent, the save is unconditional. Publishing
    /// also moves `updated_at`.
    #[serde(default)]
    pub if_updated_at: Option<DateTime<Utc>>,
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

/// Parse `source`, run [`validate_source`] and [`crate::spec::unrunnable`] against
/// this deployment's served set, and build the wire DTO for a stored draft
/// row. A parse failure here is a bug, not a caller error — `source`
/// only reaches storage after successfully parsing in [`create_flow`]/
/// [`update_flow`] — so it is mapped to `Error::Internal` rather than
/// `InvalidInput`, which would misattribute a storage-layer inconsistency to
/// the current request.
fn draft_dto(row: FlowRow, can_manage: bool, worker_enabled: bool) -> EgretResult<FlowDto> {
    let parsed = parse(&row.source).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "stored draft {} no longer parses: {} (line {:?}, column {:?})",
            row.id,
            e.message,
            e.line,
            e.column
        ))
    })?;
    let mut report = validate_source(&row.source, &parsed);
    report
        .errors
        .extend(crate::spec::unrunnable(&parsed, crate::runtime::served()));
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
        created_by_name: row.created_by_name,
        latest_version: row.latest_version,
        created_at: row.created_at,
        updated_at: row.updated_at,
        validation: ValidationReportDto::from(&report),
        can_manage,
        worker_enabled,
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

/// The `entity_type` a workflow flow's folder must carry. `analytics.folders`
/// is polymorphic by convention (0021/0113), free-form `text`, no `CHECK` --
/// so this is the ONE place the string is spelled, matching how
/// `egret-dataapi`'s `FOLDER_ENTITY_TYPE` and `egret-bi`'s per-type entries
/// (`'chart'`/`'dashboard'`/`'dataset'`) each pick their own resource noun.
pub const FOLDER_ENTITY_TYPE: &str = "flow";

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// `folder_id` arrives in the request BODY (`CreateFlowRequest`/
/// `UpdateFlowRequest`), naming an object this crate does not otherwise
/// touch at all -- so nothing else on the path to `INSERT`/`UPDATE`
/// constrains it. Ported verbatim from `egret-dataapi`'s `scoped_folder`
/// (`crates/egret-dataapi/src/routes.rs`), which states the same hazard in
/// almost the same words:
///
/// Whole-branch review, CRITICAL finding 1, measured live before this fix:
/// `POST /flows` with a folder_id belonging to ANOTHER workspace answered
/// `201 Created` and stored it; the same request with a folder_id that
/// exists nowhere answered `500`. The only thing behind
/// `workflow.flows.folder_id` is a plain `REFERENCES analytics.folders(id)`
/// (migration 0132) -- migration 0128's exact shape, the one spec §8 names
/// as this programme's recorded failure, one column over. 201-versus-500 is
/// a free existence oracle for folder ids in every other workspace; an
/// unchecked workspace match lets a `workflow:manage` holder file a flow
/// into a folder that belongs to someone else's workspace outright.
///
/// `entity_type` is checked too, not only the workspace, for the identical
/// reason `scoped_folder`'s Data API original does: a workspace-local CHART
/// folder is a perfectly valid uuid here, and a flow filed into one
/// disappears from `GET /flows?folder_id=…` without ever erroring. The
/// wrong TYPE reads as `NotFound`, same as a foreign or nonexistent one: to
/// a flow author, a folder that cannot hold flows is a folder that does not
/// exist -- a distinguishable answer would turn this check into the same
/// kind of oracle it exists to close.
async fn scoped_folder(st: &WorkflowState, ws: Uuid, folder: Uuid) -> EgretResult<()> {
    let meta: Option<(Uuid, String)> =
        sqlx::query_as("SELECT workspace_id, entity_type FROM analytics.folders WHERE id = $1")
            .bind(folder)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(db_err)?;
    match meta {
        Some((owner, etype)) if owner == ws && etype == FOLDER_ENTITY_TYPE => Ok(()),
        _ => Err(Error::NotFound("folder".into())),
    }
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
        ("trashed" = Option<bool>, Query, description = "`true` lists only the flows in the trash; absent or `false`, only the others"),
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
        trashed: q.trashed,
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
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace, or the flow calls secret() and the caller lacks workflow:use_secrets", body = egret_core::http::ErrorBody),
        (status = 409, description = "A flow with this namespace and id already exists in the workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn create_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Json(body): Json<CreateFlowRequest>,
) -> std::result::Result<(StatusCode, Json<FlowDto>), WriteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let (namespace, flow_id, plan) = parse_draft(&body.source)?;
    require_secret_use(&st, &ctx, &body.source).await?;
    // Before the insert, so a refused reference never reaches its FK — and
    // so the answer is a 404 about the folder rather than a 500 about a
    // constraint (whole-branch review, critical finding 1).
    if let Some(folder_id) = body.folder_id {
        scoped_folder(&st, ctx.workspace_id.0, folder_id).await?;
    }
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
    // The caller just passed `workflow:manage` to get here.
    Ok((
        StatusCode::CREATED,
        Json(draft_dto(created, true, st.trigger_flags.worker_enabled)?),
    ))
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
    // The check the mutating routes make, so the editor and the routes
    // cannot disagree about who may write.
    let can_manage = caller_may_manage(&st, &ctx).await?;
    Ok(Json(draft_dto(
        row,
        can_manage,
        st.trigger_flags.worker_enabled,
    )?))
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
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace, or the flow calls secret() and the caller lacks workflow:use_secrets", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "Another flow already claims this namespace and id in the workspace, or `if_updated_at` was sent and the draft was changed since (`code` is `draft_changed`; the other conflict carries no code); nothing was written", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
/// Replace a flow's draft.
///
/// With `if_updated_at`, the save happens only if the draft is still the one the editor opened; otherwise it is refused with
/// 409 and nothing is written. Saving and publishing both move `updated_at`,
/// so after a publish the editor reads the draft again before saving.
//
// # `if_updated_at` (W7a spec §7.3) -- internal notes, not API text
//
// The comparison is inside the `UPDATE`'s own `WHERE` (`FlowRepo::update`),
// so two concurrent saves of the same draft cannot both pass: the second
// waits on the first's row lock, re-reads the row, no longer matches, and is
// refused with 409.
//
// The precondition means "nobody saved or published this draft since you
// read it" only because these are **every** writer of
// `workflow.flows.updated_at` (`grep -rn updated_at` over `crates/` and
// `migrations/`, 2026-10-10):
//
// * `FlowRepo::create` -- the `INSERT`, through the column's `DEFAULT now()`
//   (migration 0132);
// * `FlowRepo::update` -- this route, `updated_at = now()`;
// * `FlowRepo::set_live` -- publish (`POST /flows/{id}/publish`),
//   `updated_at = now()`.
//
// Writes to the row that do **not** move it: `FlowRepo::soft_delete`
// (`deleted_at` only; a trashed draft is not updatable anyway), and the
// `ON DELETE SET NULL` foreign keys on `folder_id` and `live_version_id`
// (migration 0132). No database trigger writes it.
async fn update_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<UpdateFlowRequest>,
) -> std::result::Result<Json<FlowDto>, WriteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let (namespace, flow_id, plan) = parse_draft(&body.source)?;
    require_secret_use(&st, &ctx, &body.source).await?;
    // Same containment as `create_flow`, and for the identical reason: a PUT
    // may repoint the flow at a different folder, so the reference has to
    // be checked before the write, not after.
    if let Some(folder_id) = body.folder_id {
        scoped_folder(&st, ctx.workspace_id.0, folder_id).await?;
    }
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
                if_updated_at: body.if_updated_at,
            },
        )
        .await
        // The repo names a failed precondition with this crate's own constant
        // (`FlowRepo::update`, its only writer); it becomes the coded 409 here.
        .map_err(|e| match e {
            Error::Conflict(m) if m == crate::repo::DRAFT_CHANGED => WriteError::DraftChanged,
            e => WriteError::Core(e),
        })?;
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
    // The caller just passed `workflow:manage` to get here.
    Ok(Json(draft_dto(
        updated,
        true,
        st.trigger_flags.worker_enabled,
    )?))
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

// ---------- Trash (KI-119) ----------
//
// `DELETE /flows/{id}` above moves a flow to the trash. These two are the ways
// out of it: back to the flow list, or gone for good, which frees the
// `(namespace, flow_id)` the trashed row still holds. Both gate on
// `workflow:manage`, as delete does, and both are audited, as delete is.

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/{id}/restore",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "Flow restored from the trash, with its source, state and versions; its schedules start at their next tick", body = FlowDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "The flow is not in the trash", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn restore_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<FlowDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let restored = st.repo.restore(ctx.workspace_id.0, id).await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:restore",
        "workflow_flow",
        Some(id.to_string()),
        serde_json::json!({
            "namespace": restored.namespace,
            "flow_id": restored.flow_id,
        }),
    ));
    // The caller just passed `workflow:manage` to get here.
    Ok(Json(draft_dto(
        restored,
        true,
        st.trigger_flags.worker_enabled,
    )?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/flows/{id}/permanent",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "A flow in the trash deleted permanently, with its versions and trigger state; its namespace and id are free again. Its executions are kept", body = Ack),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "The flow is not in the trash; move it there first", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn purge_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<Ack>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let gone = st.repo.purge(ctx.workspace_id.0, id).await?;
    // The names go in the detail: the row is gone, and the trail is the only
    // record left of which flow this id was.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_flow:purge",
        "workflow_flow",
        Some(id.to_string()),
        serde_json::json!({
            "namespace": gone.namespace,
            "flow_id": gone.flow_id,
            "deleted_at": gone.deleted_at,
        }),
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
    Ok(Json(validate_flow_source(&body.source)?))
}

/// The report both validate routes answer for `source`: parse (a YAML that
/// does not parse is a 400 whose message carries the line and column), then
/// [`validate_source`] (which carries the key census: an unknown property is an
/// error, a key Kestra accepts and egret ignores a `not_honoured` warning),
/// plus [`crate::spec::unrunnable`] against this deployment's served set.
/// Writes nothing.
fn validate_flow_source(source: &str) -> EgretResult<ValidationReportDto> {
    let flow = parse_unsaved(source)?;
    let mut report = validate_source(source, &flow);
    report
        .errors
        .extend(crate::spec::unrunnable(&flow, crate::runtime::served()));
    Ok(ValidationReportDto::from(&report))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/validate",
    tag = "workflow",
    params(WorkspacePath),
    request_body = ValidateFlowRequest,
    responses(
        (status = 200, description = "The validation report of a source that belongs to no saved flow yet (the create page). The same report `POST /flows/{id}/validate` gives for the same source; nothing is written", body = ValidationReportDto),
        (status = 400, description = "The YAML does not parse; the message carries the line and column", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn validate_unsaved_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Json(body): Json<ValidateFlowRequest>,
) -> EgretResult<Json<ValidationReportDto>> {
    // `:manage`, as the by-id route: validating is an authoring action.
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    Ok(Json(validate_flow_source(&body.source)?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/schema",
    tag = "workflow",
    params(WorkspacePath),
    responses(
        (status = 200, description = "The JSON Schema (draft 2020-12) of one flow document as this workspace may publish it: Kestra's definitions filtered to the types and properties egret honours, egret's own types, and the script and plugin types only when this workspace's features allow them. Carries a strong `ETag` and `Cache-Control: private, no-cache`", body = Object),
        (status = 304, description = "`If-None-Match` names the current `ETag`"),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_flow_schema(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    headers: axum::http::HeaderMap,
) -> EgretResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse as _;
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    // The same effective feature set `publish_flow` gates with.
    let disabled = st.workspaces.disabled_features(ctx.workspace_id).await?;
    let features = crate::spec::schema::SchemaFeatures {
        kestra_plugins: !disabled.contains(crate::plugins::FEATURE_KEY),
        containers: !disabled.contains(crate::scripts::FEATURE_KEY),
    };
    let served = crate::spec::schema::served(&features, &st.scripts);
    let cache = [
        (header::ETAG, served.etag.clone()),
        (header::CACHE_CONTROL, "private, no-cache".to_string()),
    ];
    if if_none_match(&headers, &served.etag) {
        return Ok((StatusCode::NOT_MODIFIED, cache).into_response());
    }
    Ok((
        StatusCode::OK,
        cache,
        [(header::CONTENT_TYPE, "application/json".to_string())],
        served.bytes.clone(),
    )
        .into_response())
}

/// Whether `If-None-Match` names `etag` (or is `*`). RFC 9110 §13.1.2 uses
/// the weak comparison here, so a `W/` prefix on a listed tag is ignored.
fn if_none_match(headers: &axum::http::HeaderMap, etag: &str) -> bool {
    headers
        .get_all(axum::http::header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
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
        (status = 400, description = "The draft does not parse or validate, a task names an unknown type or a type this deployment does not serve (`task_type_not_served`), or a script task is refused by the container gate (workflow_containers off, no or empty allowed-images for its runner, image not allowed)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace, or the flow calls secret() and the caller lacks workflow:use_secrets", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace, or a task references an object that does not exist or belongs to another workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn publish_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    body: Option<Json<PublishFlowRequest>>,
) -> std::result::Result<Json<FlowVersionDto>, WriteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    // I3: the draft `publish_gated` is about to freeze. Read here as well as
    // there: a draft stored before this gate existed (or by a writer that is
    // not these routes) must not be published by someone without the
    // permission. Between this read and `publish_gated`'s, only
    // `create_flow`/`update_flow` can change the draft, and both run the same
    // check, so a source that reaches the freeze without having been checked
    // against SOME holder of the permission cannot exist.
    let draft = st.repo.get(ctx.workspace_id.0, id).await?;
    require_secret_use(&st, &ctx, &draft.source).await?;
    let note = body.map(|Json(b)| b.note).unwrap_or_default();
    // W5a: the workspace half of the script-task gate. `disabled_features` is
    // the effective set, so an absent `workflow_containers` row reads as OFF
    // (its `default_enabled`), exactly as the `WsContext` gate would read it.
    // W5b: the plugin gate's feature is read from the same effective set.
    let disabled = st.workspaces.disabled_features(ctx.workspace_id).await?;
    let feature_enabled = !disabled.contains(crate::scripts::FEATURE_KEY);
    let kestra_plugins_enabled = !disabled.contains(crate::plugins::FEATURE_KEY);
    let version = crate::publish::publish_gated(
        &st.repo,
        &*st.refs,
        ctx.workspace_id.0,
        ctx.principal.user_id,
        id,
        &note,
        &crate::publish::ScriptGate {
            feature_enabled,
            plugin_configurations: &st.scripts.plugin_configurations,
            default_runner: st.scripts.default_runner,
            kestra_plugins_enabled,
        },
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

// ---------- The flow graph (W7c Task 2, spec §3.4, §3.5, §5) ----------
//
// ```text
// GET  /api/ws/{workspace}/workflows/flows/{id}/graph?version=&subflows=   workflow:read
// POST /api/ws/{workspace}/workflows/flows/graph?subflows=                 workflow:manage
// ```
//
// Both build [`crate::graph::expanded_flow_graph`] with the request's
// workspace as the only place a `Subflow` child is loaded from
// ([`FlowRepo`]'s [`crate::graph::SubflowSource`]). `GET` reads the stored
// draft, or with `version=` that version's frozen `source`; a foreign flow or
// version is the 404 every other read here gives, since both reads are scoped
// by the workspace. `POST` is the editor's: it draws unsaved YAML, so it is
// `:manage` as the validate route is, a parse error is that route's 400 with
// line and column, and YAML that parses but fails validation still gets its
// graph (the refused tasks carry `refusal`).

/// `POST /flows/graph` body: the unsaved YAML to draw.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct FlowGraphRequest {
    pub source: String,
}

/// `GET /flows/{id}/graph`'s typed query parameter. `subflows` is read from
/// the raw query instead ([`subflows_param`]), because it repeats.
#[derive(Debug, Deserialize)]
pub struct FlowGraphQuery {
    pub version: Option<i32>,
}

/// `ErrorBody.code` of a graph over [`crate::graph::MAX_NODES`] (422).
pub const GRAPH_TOO_LARGE_CODE: &str = "graph_too_large";

/// The error of the graph routes: every [`egret_core::Error`], plus the 422
/// `egret_core::Error` has no variant for.
#[derive(Debug)]
pub enum GraphRouteError {
    Core(Error),
    Graph(crate::graph::GraphError),
}

impl From<Error> for GraphRouteError {
    fn from(e: Error) -> Self {
        GraphRouteError::Core(e)
    }
}

impl From<crate::graph::GraphError> for GraphRouteError {
    fn from(e: crate::graph::GraphError) -> Self {
        GraphRouteError::Graph(e)
    }
}

impl axum::response::IntoResponse for GraphRouteError {
    fn into_response(self) -> axum::response::Response {
        use crate::graph::GraphError;
        match self {
            GraphRouteError::Core(e) => e.into_response(),
            GraphRouteError::Graph(e @ GraphError::TooLarge { .. }) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(egret_core::http::ErrorBody {
                    error: e.to_string(),
                    code: Some(GRAPH_TOO_LARGE_CODE.to_string()),
                }),
            )
                .into_response(),
            GraphRouteError::Graph(e @ (GraphError::Source(_) | GraphError::State(_))) => {
                Error::Internal(anyhow::anyhow!("{e}")).into_response()
            }
        }
    }
}

/// Every `subflows` value of the query string, in order (`?subflows=a&subflows=b`,
/// each percent-decoded). Unknown keys are ignored.
fn subflows_param(raw: Option<&str>) -> std::collections::BTreeSet<String> {
    crate::webhook_routes::parse_query(raw)
        .into_iter()
        .filter(|(k, _)| k == "subflows")
        .map(|(_, v)| v)
        .collect()
}

/// Parse unsaved YAML for the editor's routes (validate, graph): a YAML that
/// does not parse is a 400 whose message carries the line and column.
fn parse_unsaved(source: &str) -> EgretResult<crate::models::Flow> {
    parse(source).map_err(|e| {
        Error::InvalidInput(match (e.line, e.column) {
            (Some(l), Some(c)) => format!("{} (line {l}, column {c})", e.message),
            _ => e.message,
        })
    })
}

async fn graph_for(
    st: &WorkflowState,
    ws: Uuid,
    flow: &crate::models::Flow,
    raw_query: Option<&str>,
) -> std::result::Result<Json<crate::graph::FlowGraph>, GraphRouteError> {
    let opts = crate::graph::GraphOptions {
        subflows: subflows_param(raw_query),
        depth: 0,
    };
    Ok(Json(
        crate::graph::expanded_flow_graph(flow, &opts, ws, &st.repo, crate::runtime::served())
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows/{id}/graph",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("version" = Option<i32>, Query, description = "Draw this published version's frozen source instead of the draft"),
        ("subflows" = Option<Vec<String>>, Query, explode, description = "Uids of `Subflow` nodes to expand in place, repeated (`subflows=a&subflows=b`). A uid inside an expansion is `{subflow_uid}/{child_uid}`; a uid that names no `Subflow` node is ignored"),
    ),
    responses(
        (status = 200, description = "The topology of the draft, or of the version", body = crate::graph::FlowGraph),
        (status = 400, description = "`version` is not an integer", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow, or no such version of it, in this workspace", body = egret_core::http::ErrorBody),
        (status = 422, description = "The graph, counted after expansion, has more than 5,000 nodes; the message names the count", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_flow_graph(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<FlowGraphQuery>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> std::result::Result<Json<crate::graph::FlowGraph>, GraphRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let ws = ctx.workspace_id.0;
    let source = match q.version {
        // Scoped by the workspace through the flow: a foreign flow's version
        // is `NotFound`, as `get_flow_version` answers.
        Some(v) => {
            let row = st.repo.get_version(ws, id, v).await?;
            row.snapshot
                .get("source")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    Error::Internal(anyhow::anyhow!(
                        "flow version {} has no source in its snapshot",
                        row.id
                    ))
                })?
        }
        None => st.repo.get(ws, id).await?.source,
    };
    // A stored source parsed when it was saved, so a failure here is ours.
    let flow = parse(&source).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "stored source of flow {id} no longer parses: {} (line {:?}, column {:?})",
            e.message,
            e.line,
            e.column
        ))
    })?;
    graph_for(&st, ws, &flow, raw.as_deref()).await
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/graph",
    tag = "workflow",
    params(
        WorkspacePath,
        ("subflows" = Option<Vec<String>>, Query, explode, description = "Uids of `Subflow` nodes to expand in place, repeated (`subflows=a&subflows=b`). A uid inside an expansion is `{subflow_uid}/{child_uid}`; a uid that names no `Subflow` node is ignored"),
    ),
    request_body = FlowGraphRequest,
    responses(
        (status = 200, description = "The topology of the unsaved YAML. A flow that parses but fails validation still gets its graph; its refused tasks carry `refusal`. Nothing is written", body = crate::graph::FlowGraph),
        (status = 400, description = "The YAML does not parse; the message carries the line and column", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 422, description = "The graph, counted after expansion, has more than 5,000 nodes; the message names the count", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn graph_unsaved_flow(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    Json(body): Json<FlowGraphRequest>,
) -> std::result::Result<Json<crate::graph::FlowGraph>, GraphRouteError> {
    // `:manage`, as the validate routes: the editor draws what is being
    // authored.
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let flow = parse_unsaved(&body.source)?;
    graph_for(&st, ctx.workspace_id.0, &flow, raw.as_deref()).await
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}/graph",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id"),
        ("subflows" = Option<Vec<String>>, Query, explode, description = "Uids of `Subflow` nodes to expand in place, repeated (`subflows=a&subflows=b`). A `Subflow` whose run started a child execution expands from that execution's frozen source, with its states; otherwise from the child flow's current draft, without states. A uid inside an expansion is `{subflow_uid}/{child_uid}`; a uid that names no `Subflow` node is ignored"),
    ),
    responses(
        (status = 200, description = "The graph of the execution's frozen source, and the state of every node that has a run. A task with no run has no entry; a task in a `Loop` body is folded over every iteration by Kestra's priority, with `iterations` and no `run_id`", body = crate::graph::ExecutionGraph),
        (status = 400, description = "`{id}` is not a UUID", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace; one of another workspace answers exactly this", body = egret_core::http::ErrorBody),
        (status = 422, description = "The graph, counted after expansion, has more than 5,000 nodes; the message names the count", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_execution_graph(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> std::result::Result<Json<crate::graph::ExecutionGraph>, GraphRouteError> {
    use crate::graph::ExecutionSource as _;
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let ws = ctx.workspace_id.0;
    // Scoped by the workspace in the read itself: a foreign id is `None`.
    let frozen = st
        .executions
        .frozen(ws, id)
        .await?
        .ok_or_else(|| Error::NotFound("execution".into()))?;
    // The source it ran, frozen at start; it parsed then, so a failure is ours.
    let flow = parse(&frozen.source).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "frozen source of execution {id} no longer parses: {} (line {:?}, column {:?})",
            e.message,
            e.line,
            e.column
        ))
    })?;
    let opts = crate::graph::GraphOptions {
        subflows: subflows_param(raw.as_deref()),
        depth: 0,
    };
    Ok(Json(
        crate::graph::execution_graph(
            &flow,
            &frozen.runs,
            &opts,
            ws,
            &st.repo,
            &st.executions,
            crate::runtime::served(),
        )
        .await?,
    ))
}

// ---------- Executions (W10 Task 4) ----------
//
// The READ surface over `workflow.executions` / `workflow.task_runs`, and the
// first time an execution is visible outside the database. Two routes, both
// [`WF_READ`]:
//
// ```text
// GET /api/ws/{workspace}/workflows/executions
// GET /api/ws/{workspace}/workflows/executions/{id}
// ```
//
// **Start, resume and replay landed in W10 Task 5** and are below;
// [`ExecutionRepo`] still has no method that writes, and none of them reaches
// for one — every write goes through `worker::claim`, which holds the
// `FOR NO KEY UPDATE` that repository deliberately does not. **Kill is a finding**;
// see the write section's own comment.
//
// **A foreign execution is a 404 and never a 403.** [`ExecutionRepo::get`]
// answers `NotFound` to a foreign id as well as an unknown one, and
// `egret_core::http` maps that to 404. A 403 would confirm the id is real,
// which is the same disclosure as serving the row — the `chart_id` class
// migration 0128 shipped and spec §8 records.
//
// **No join to `workflow.flows`, and that is a decision rather than an
// omission.** A list row carries `namespace` and `flow_key` — the flow's human
// identity, FROZEN into the execution at start — so nothing here needs the
// `flows` table to name what ran. Joining it would cost three things and buy
// one: `executions_flow_id_fkey` is SINGLE-column with no workspace component
// (unlike the composite parent/trigger FKs), so the join would need its own
// `f.workspace_id = $n` or it would reach across workspaces; an INNER join
// would drop every execution whose `flow_id` is `NULL`, making `rows` disagree
// with the `total` counted by the second, unjoined statement, and a pager
// cannot draw a last page it can never reach; and `flows.flow_id` is TODAY's
// name for a draft that may have been renamed since, so a joined label would
// misreport what an old execution ran. `namespace`/`flow_key` cannot: they are
// what this execution actually ran, and they survive the draft being trashed.

/// One execution as `GET /executions` lists it.
///
/// **No `source`.** It is the whole frozen flow YAML — one document per row —
/// and a fifty-row page would carry fifty of them to show a column that does
/// not exist. The field is not merely omitted here: [`ExecutionRow`] does not
/// have it at all, so no future edit to this DTO can put it back by accident.
/// `GET /executions/{id}` carries it, because a detail page draws the topology
/// from it.
///
/// `workspace_id` is dropped for the reason every other DTO in this module
/// drops it: the `{workspace}` path segment already named it, and a body field
/// that merely echoes the URL is a second place for it to be wrong.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionListItemDto {
    pub id: Uuid,
    /// The draft this execution came from, and `NULL` once that flow was
    /// deleted permanently from the trash (`FlowRepo::purge`, KI-119) —
    /// **trashing a flow does not clear it**, because `FlowRepo`'s delete is
    /// soft. A trashed flow's executions are still listed, and still findable
    /// by `flow_id`.
    pub flow_id: Option<Uuid>,
    pub namespace: String,
    /// The YAML's own `id`; `flow_id` is taken by the draft's uuid above.
    pub flow_key: String,
    /// Kestra's wire spelling — `RUNNING`, `SUCCESS`, `FAILED`, …
    pub state: String,
    /// The state timeline, as stored: an array of state NAMES, e.g.
    /// `["CREATED","RUNNING","SUCCESS"]`. It carries no timestamps — see
    /// [`TaskRunDto::history`], which is written by the same function.
    #[schema(value_type = Vec<String>)]
    pub history: serde_json::Value,
    #[schema(value_type = Object)]
    pub inputs: serde_json::Value,
    pub seq: i64,
    /// The trigger record, `NULL` for an API start.
    #[schema(value_type = Option<Object>)]
    pub execution_trigger: Option<serde_json::Value>,
    pub trigger_execution_id: Option<Uuid>,
    pub trigger_run_ix: Option<i32>,
    pub parent_execution_id: Option<Uuid>,
    pub parent_run_ix: Option<i32>,
    pub loop_index: Option<i32>,
    pub loop_value: Option<String>,
    pub attempt_number: i32,
    pub origin_offset_ms: i64,
    /// The execution this one is a manual replay of (`POST …/replay`), `NULL`
    /// for every other start. A `CREATE_NEW_EXECUTION` retry is NOT linked
    /// here; it carries `parent_execution_id` instead.
    pub replayed_from_execution_id: Option<Uuid>,
    /// The first time the execution entered `RUNNING`. `NULL` before it runs,
    /// for one that never ran (an SLA that cancelled it at creation), and for
    /// an execution recorded before these times existed — a UI shows that last
    /// case as estimated, never as "not started".
    pub started_at: Option<DateTime<Utc>>,
    /// When the execution entered a terminal state; `NULL` while it has not
    /// (a paused execution has none), and for an execution recorded before
    /// these times existed. Written once: later writes to a finished
    /// execution do not move it.
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The execution's own label column; `null` for none. At start it holds
    /// the labels beyond the flow's own `labels:` (a backfill's, a replay's
    /// copy); once a `Labels` task has run it holds the full merged list, the
    /// flow's labels included. The detail's `effective_labels` is the list to
    /// display.
    pub labels: Option<Vec<LabelDto>>,
    /// The backfill that fired this execution (W7f), `null` otherwise.
    pub backfill_id: Option<Uuid>,
}

/// One execution label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LabelDto {
    pub key: String,
    pub value: String,
}

impl From<crate::models::Label> for LabelDto {
    fn from(l: crate::models::Label) -> Self {
        Self {
            key: l.key,
            value: l.value,
        }
    }
}

impl From<ExecutionRow> for ExecutionListItemDto {
    fn from(r: ExecutionRow) -> Self {
        let own: Vec<LabelDto> = crate::labels::own_labels(r.labels.as_ref())
            .into_iter()
            .map(LabelDto::from)
            .collect();
        Self {
            id: r.id,
            flow_id: r.flow_id,
            namespace: r.namespace,
            flow_key: r.flow_key,
            state: r.state,
            history: r.history,
            inputs: r.inputs,
            seq: r.seq,
            execution_trigger: r.execution_trigger,
            trigger_execution_id: r.trigger_execution_id,
            trigger_run_ix: r.trigger_run_ix,
            parent_execution_id: r.parent_execution_id,
            parent_run_ix: r.parent_run_ix,
            loop_index: r.loop_index,
            loop_value: r.loop_value,
            attempt_number: r.attempt_number,
            origin_offset_ms: r.origin_offset_ms,
            replayed_from_execution_id: r.replayed_from_execution_id,
            started_at: r.started_at,
            ended_at: r.ended_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            labels: (!own.is_empty()).then_some(own),
            backfill_id: r.backfill_id,
        }
    }
}

/// One element of [`TaskRunDto::attempts`], as `worker::claim`'s
/// `attempts_json` writes it.
///
/// **The schema of a field that is passed through as stored.** `attempts` is
/// served as the stored `jsonb` (`serde_json::Value`), and this type is what
/// the document says it holds; `tests/executions_routes.rs`
/// (`a_gantt_row_is_computable_from_the_execution_dto_alone`) deserialises a
/// real run's `attempts` into it, so the two cannot drift silently.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AttemptDto {
    /// The attempt's state names, in order, e.g. `["RUNNING","FAILED"]`.
    pub history: Vec<String>,
    /// Milliseconds after the execution's `created_at`.
    pub started_at: i64,
    /// Milliseconds after the execution's `created_at`. Absent while the
    /// attempt is open, and on an attempt recorded before migration 0183.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// One task run, whole — all twenty-four columns.
///
/// Unlike the execution's own row there is nothing dropped here: a detail page
/// reads ONE execution's runs, so there is no page-sized multiplier the way
/// `executions.source` has one.
///
/// **These are the fields a gantt is drawn from**, and
/// `tests/executions_routes.rs::a_gantt_row_is_computable_from_the_execution_
/// dto_alone` computes one from this DTO and nothing else, so W7 cannot
/// discover a missing field after this package closes. What it uses:
/// [`Self::task_id`] and [`Self::parent_ix`] for the label and the nesting,
/// [`Self::state`] for the bar, [`Self::created_seq`]/[`Self::ended_seq`] for
/// LOGICAL time, and [`Self::started_at`]/[`Self::ended_at`] plus
/// `attempts[].started_at`/`attempts[].ended_at` for WALL-CLOCK time.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TaskRunDto {
    pub id: Uuid,
    pub execution_id: Uuid,
    /// The `RunIx` — the executor's creation order, and the order the detail
    /// route returns these in.
    pub run_ix: i32,
    /// The `TaskIx` — the index into the plan's task list.
    pub task_ix: i32,
    pub task_id: String,
    /// The task's `type:`, and `NULL` exactly when the planned kind is not
    /// runnable — i.e. a flowable the executor drives itself, which is why
    /// such a run has no `attempts` for a gantt to read a start from.
    pub type_id: Option<String>,
    /// The parent run's `run_ix` — a gantt's nesting, `NULL` at the top level.
    pub parent_ix: Option<i32>,
    pub value: Option<String>,
    /// The run's rendered expression (an `If`'s condition, a `Switch`'s
    /// value, a `Loop`'s `values:`, ...), `NULL` until it is rendered. For a
    /// `Loop` whose `values:` named a file it is `"<kestra:/// URI>\n<count>"`
    /// -- the file and how many records it holds -- and **never the records**
    /// (W9a Task 11 fix round 1): they are the file's, read through the
    /// execution file API, and stay out of every task-run response.
    pub rendered: Option<String>,
    pub rendered_run_if: Option<String>,
    pub state: String,
    /// **An array of state NAMES and nothing else** — `["CREATED","RUNNING",
    /// "SUCCESS"]` — written by `worker::claim`'s `states_json`, which maps
    /// each `RunState` to its wire spelling. It is **not** Kestra's
    /// `State.History`, and it carries **no dates**: a timeline drawn from
    /// this alone has an order but no clock. The wall-clock instants are
    /// [`Self::created_at`], [`Self::updated_at`], and `attempts[].started_at`
    /// (milliseconds after the EXECUTION's `created_at`, which is what a
    /// `Tick` is in this engine).
    #[schema(value_type = Vec<String>)]
    pub history: serde_json::Value,
    /// One entry per attempt: `{"history": [...], "started_at": <ms>,
    /// "ended_at": <ms>}`, both in milliseconds after the execution's
    /// `created_at`. `ended_at` is absent while the attempt is open and on an
    /// attempt recorded before it existed; an attempt that a retry followed
    /// ends no later than the next one starts.
    #[schema(value_type = Vec<AttemptDto>)]
    pub attempts: serde_json::Value,
    pub not_before_ms: Option<i64>,
    /// The lease. `NULL` means unclaimed.
    pub claimed_at: Option<DateTime<Utc>>,
    /// The run's outputs, `NULL` for a run that has none: a handler that
    /// returned nothing (a `Log`, a `Sleep`) and any run not yet terminal.
    #[schema(value_type = Option<Object>)]
    pub outputs: Option<serde_json::Value>,
    pub message: Option<String>,
    /// The executor round this run was created in — LOGICAL time. Two bars
    /// whose wall-clock spans are microseconds apart are still ordered by
    /// this, which is why a gantt wants it beside the timestamps.
    pub created_seq: Option<i64>,
    /// The executor round this run terminated in; `NULL` while it has not.
    pub ended_seq: Option<i64>,
    pub resume_with: Option<String>,
    /// The first time the run entered `RUNNING`: for a worker task, when the
    /// executor handed it to the workers (equal, to the millisecond, to
    /// `attempts[0].started_at`); for a flowable, when the executor first ran
    /// it. Not moved by a resumed pause or a retry. `NULL` for a run that
    /// never ran — `SKIPPED` by `runIf`, or not yet started — and for a run
    /// recorded before these times existed.
    pub started_at: Option<DateTime<Utc>>,
    /// When the run entered a terminal state; `NULL` while it has not, and
    /// cleared if a retry reopens it. Written once per terminal: a later
    /// write to a finished run does not move it.
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TaskRunRow> for TaskRunDto {
    fn from(r: TaskRunRow) -> Self {
        Self {
            id: r.id,
            execution_id: r.execution_id,
            run_ix: r.run_ix,
            task_ix: r.task_ix,
            task_id: r.task_id,
            type_id: r.type_id,
            parent_ix: r.parent_ix,
            value: r.value,
            rendered: r.rendered,
            rendered_run_if: r.rendered_run_if,
            state: r.state,
            history: r.history,
            attempts: r.attempts,
            not_before_ms: r.not_before_ms,
            claimed_at: r.claimed_at,
            outputs: r.outputs,
            message: r.message,
            created_seq: r.created_seq,
            ended_seq: r.ended_seq,
            resume_with: r.resume_with,
            started_at: r.started_at,
            ended_at: r.ended_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// One execution in full: every list field, plus the two things only a detail
/// page needs — the frozen `source` and every task run, ordered by `run_ix`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionDto {
    #[serde(flatten)]
    pub execution: ExecutionListItemDto,
    /// The flow source this execution runs, frozen at start. The plan is pure
    /// over it, which is why a detail page can draw the topology from this
    /// string alone.
    pub source: String,
    /// Every task run, in `run_ix` order — the executor's creation order.
    pub runs: Vec<TaskRunDto>,
    /// Whether the caller may resume and replay this execution: they hold
    /// `workflow:manage` in this workspace. A detail page hides both actions
    /// when this is false; the routes still check for themselves.
    // W7b spec §5.2. The same PDP decision `FlowDto::can_manage` carries, on
    // the execution itself because its flow may be trashed (a 404 to read)
    // while the execution is still readable.
    pub can_manage: bool,
    /// What the run sees as `labels`: the flow's, then `labels`, the later key
    /// winning. Empty when the frozen source does not parse.
    pub effective_labels: Vec<LabelDto>,
    /// Whether this deployment runs the workflow worker
    /// (`workflow.worker_enabled`, `EGRET_WORKFLOW__WORKER_ENABLED`; off by
    /// default). **False: an execution is created and stays `RUNNING`, and
    /// nothing runs it until an administrator turns the worker on** (KI-013).
    /// The same flag the trigger routes echo ([`TriggerFlags`]).
    pub worker_enabled: bool,
}

impl ExecutionDto {
    fn new(d: ExecutionDetail, can_manage: bool, worker_enabled: bool) -> Self {
        let own = crate::labels::own_labels(d.execution.labels.as_ref());
        let flow_labels = crate::spec::parse(&d.source)
            .ok()
            .map(|f| f.labels)
            .unwrap_or_default();
        let effective_labels = crate::labels::execution_labels(&flow_labels, &own)
            .into_iter()
            .map(LabelDto::from)
            .collect();
        Self {
            effective_labels,
            execution: ExecutionListItemDto::from(d.execution),
            source: d.source,
            runs: d.runs.into_iter().map(TaskRunDto::from).collect(),
            can_manage,
            worker_enabled,
        }
    }
}

/// The PDP's answer to the check every mutating workflow route makes. A PDP
/// that failed is an error, not a "no": only a refusal reads as false.
pub(crate) async fn caller_may_manage(
    st: &WorkflowState,
    ctx: &egret_core::WorkspaceContext,
) -> EgretResult<bool> {
    match egret_core::authz::require(&*st.pdp, ctx, WF_MANAGE).await {
        Ok(()) => Ok(true),
        Err(Error::Forbidden) => Ok(false),
        Err(e) => Err(e),
    }
}

/// One page of executions, with the size of the whole match behind it and the
/// window that was actually served.
///
/// **`limit`/`offset` are echoed because they are CLAMPED**, and a clamp
/// nobody can see is the indefensible one (the same reasoning
/// `egret-dataapi`'s `ActivityCallsQuery::limit` states). A caller that asks
/// for 1000 rows is told it was served 200 rather than having to infer it from
/// a short page — which it could not, since a short page is also what the last
/// page looks like.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionPageDto {
    pub rows: Vec<ExecutionListItemDto>,
    /// Every execution matching the filter, not just this page's. Counted by a
    /// second statement carrying the same predicates, workspace one included.
    pub total: i64,
    /// The page size actually used, after the clamp.
    pub limit: i64,
    /// The offset actually used, after the clamp.
    pub offset: i64,
    /// Whether this deployment runs the workflow worker
    /// (`workflow.worker_enabled`, `EGRET_WORKFLOW__WORKER_ENABLED`; off by
    /// default). **False: an execution is created and stays `RUNNING`, and
    /// nothing runs it until an administrator turns the worker on** (KI-013).
    /// The same flag the trigger routes echo ([`TriggerFlags`]).
    pub worker_enabled: bool,
}

/// `GET /executions`'s query parameters. Each is optional and each is ANDed
/// with the workspace predicate [`ExecutionRepo::list`] itself carries — never
/// a way to reach another workspace's executions: every filter narrows within
/// `ctx.workspace_id`, and there is no filter that widens past it.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListExecutionsQuery {
    /// Only executions started from this draft. A trashed draft still matches:
    /// `FlowRepo`'s delete is soft, so the column keeps pointing at it.
    pub flow_id: Option<Uuid>,
    /// Exactly this namespace. Cannot be combined with `namespace_prefix` (400).
    pub namespace: Option<String>,
    /// This namespace and every namespace beneath it, dot-anchored: `a.b`
    /// matches `a.b` and `a.b.c`, never `a.bc`. A `_` or `%` is a literal
    /// character, not a wildcard. Cannot be combined with `namespace` (400).
    pub namespace_prefix: Option<String>,
    /// The YAML's own `id` — the only filter that still finds an execution
    /// whose `flow_id` was `NULL`ed by a real `DELETE`.
    pub flow_key: Option<String>,
    /// One or more states, **comma-separated** (`RUNNING,FAILED`): a run list
    /// shows failures beside what is in flight.
    ///
    /// **A repeated `state=` key is REFUSED, not narrowed.** axum's `Query`
    /// decodes with `serde_urlencoded`, which answers
    /// ``Err("duplicate field `state`")`` for `?state=A&state=B`; over HTTP
    /// that is a **400**, body ``Failed to deserialize query string: duplicate
    /// field `state` ``. So comma-separation is not a guard against a silent
    /// drop — there is no silent drop — it is simply the spelling this
    /// parameter accepts for more than one value, and a caller who reaches for
    /// the repeated-key spelling is told so rather than served a narrower
    /// answer. `a_repeated_state_key_is_refused_rather_than_silently_narrowed`
    /// in `tests/executions_routes.rs` measures both halves.
    ///
    /// An earlier version of this comment claimed the opposite — that a
    /// repeated key is silently reduced to its last value. The DESIGN it
    /// defended was right and the MECHANISM was false, and the mechanism is
    /// what shipped: it reached `openapi.json` and the generated TypeScript
    /// JSDoc, where a frontend author would have read it and reasoned from it.
    /// **A decision and its mechanism are two claims, and the next person
    /// reasons from the mechanism.**
    ///
    /// **Present but empty (`?state=`) matches nothing**, which is what a UI
    /// that has deselected every chip means. Absent means "every state".
    pub state: Option<String>,
    /// Window over the execution's `created_at`, **inclusive**.
    pub since: Option<DateTime<Utc>>,
    /// Window over the execution's `created_at`, **exclusive**, so consecutive
    /// windows tile without double-counting the row on the boundary.
    pub until: Option<DateTime<Utc>>,
    /// `true`: hide child executions — a loop iteration (`loop_index` set)
    /// and a subflow or flow-trigger child (`trigger_execution_id` set). A
    /// `CREATE_NEW_EXECUTION` retry is NOT hidden: it carries
    /// `parent_execution_id` without a `loop_index`, and continues a root run.
    /// Absent or `false` lists every execution. Cannot be combined with
    /// `parent` (400).
    pub root_only: Option<bool>,
    /// Only the direct children of this execution: the rows whose
    /// `parent_execution_id` or `trigger_execution_id` is this id. An id from
    /// another workspace answers an empty page, never an error. Cannot be
    /// combined with `root_only=true` (400).
    pub parent: Option<Uuid>,
    /// How many rows to return. Default 50, maximum 200; a larger value is
    /// clamped rather than refused, and the clamped value comes back in the
    /// response's own `limit`.
    #[serde(default = "default_execution_limit")]
    pub limit: i64,
    /// How many rows to skip. Negative is clamped to zero.
    #[serde(default)]
    pub offset: i64,
}

fn default_execution_limit() -> i64 {
    DEFAULT_PAGE_LIMIT
}

/// Split the `state` query parameter into the repository's `Option<Vec<String>>`.
///
/// `None` (absent) and `Some(vec![])` (present and empty) are DIFFERENT
/// filters and both are reachable: absent lists every state, empty matches
/// nothing. Blank segments are dropped so `?state=RUNNING,` is one state and
/// not one state plus an unmatchable empty one.
fn parse_state_filter(raw: Option<String>) -> Option<Vec<String>> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    })
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions",
    tag = "workflow",
    params(WorkspacePath, ListExecutionsQuery),
    responses(
        (status = 200, description = "One page of the workspace's executions, newest first, with the total behind it", body = ExecutionPageDto),
        (status = 400, description = "The query string does not decode — a malformed `flow_id`/`parent`/`since`/`until`, a `root_only` that is not `true`/`false`, a non-numeric `limit`/`offset`, or a **repeated** key (`?state=A&state=B` answers `duplicate field state`; several states go in ONE comma-separated `state=`); or `root_only=true` and `parent` were both sent; or `namespace` and `namespace_prefix` were both sent", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_executions(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListExecutionsQuery>,
) -> EgretResult<Json<ExecutionPageDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    // Clamped HERE as well as in the repository, so the window this response
    // reports is the window that was served. The repository clamps too and
    // must keep doing so -- it is a public type with callers of its own, and a
    // negative `LIMIT` is an ERROR in Postgres rather than an empty page -- but
    // clamping an already-clamped value is a no-op, so the two cannot disagree
    // the way a duplicated predicate could.
    let page = Page {
        limit: q.limit.clamp(1, MAX_PAGE_LIMIT),
        offset: q.offset.max(0),
    };
    let root_only = q.root_only.unwrap_or(false);
    // Two views of the tree that do not compose: `parent` lists one level
    // beneath an execution, `root_only` hides the children that `parent` is
    // for. A caller who sent both has a bug worth naming. `root_only=false`
    // beside `parent` is the default spelled out and is accepted.
    if root_only && q.parent.is_some() {
        return Err(Error::InvalidInput(
            "`root_only=true` and `parent` cannot be combined: `parent` lists children, which `root_only` hides"
                .into(),
        ));
    }
    // Two answers to one question: the exact namespace, or it and its
    // children. Sending both is a caller bug, named rather than intersected.
    if q.namespace.is_some() && q.namespace_prefix.is_some() {
        return Err(Error::InvalidInput(
            "`namespace` and `namespace_prefix` cannot be combined: send one".into(),
        ));
    }
    let filter = ExecutionFilter {
        flow_id: q.flow_id,
        namespace: q.namespace,
        namespace_prefix: q.namespace_prefix,
        flow_key: q.flow_key,
        states: parse_state_filter(q.state),
        since: q.since,
        until: q.until,
        root_only,
        parent: q.parent,
    };
    let found = st
        .executions
        .list(ctx.workspace_id.0, &filter, page)
        .await?;
    Ok(Json(ExecutionPageDto {
        rows: found
            .rows
            .into_iter()
            .map(ExecutionListItemDto::from)
            .collect(),
        total: found.total,
        limit: page.limit,
        offset: page.offset,
        worker_enabled: st.trigger_flags.worker_enabled,
    }))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id"),
    ),
    responses(
        (status = 200, description = "The execution, its frozen source, and every task run in run_ix order", body = ExecutionDto),
        (status = 400, description = "`{id}` is not a UUID. Decided by axum's `Path` extractor before any handler runs — and AFTER `Auth`, so an anonymous caller still gets 401 and this leaks no existence", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace. **An execution belonging to another workspace answers exactly this**, indistinguishably from one that never existed: a 403 would confirm the id is real.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_execution(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<ExecutionDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    // No containment pre-check the way `list_flow_versions` needs one: this is
    // a SINGLE call that already answers `NotFound` for a foreign id, so there
    // is no second statement whose empty result could be mistaken for one.
    let detail = st.executions.get(ctx.workspace_id.0, id).await?;
    let can_manage = caller_may_manage(&st, &ctx).await?;
    Ok(Json(ExecutionDto::new(
        detail,
        can_manage,
        st.trigger_flags.worker_enabled,
    )))
}

// ---------- Execution logs (W11 Task 5, spec §6) ----------
//
// GET /api/ws/{workspace}/workflows/executions/{id}/logs
//
// A cursor over `workflow.task_logs`, not W10's offset: the log is append-only
// and grows while it is read, and offset paging over a growing set repeats or
// skips rows. W7's logs panel polls it.

/// `GET …/logs`' default `limit` (spec §6).
pub const DEFAULT_LOG_PAGE_LIMIT: i64 = 500;
/// `GET …/logs`' largest `limit` (spec §6). A larger one is **refused (400)**,
/// not clamped: a cursor client that asked for more than it got could not tell
/// a clamp from the end of what has settled.
pub const MAX_LOG_PAGE_LIMIT: i64 = 2_000;

fn default_log_limit() -> i64 {
    DEFAULT_LOG_PAGE_LIMIT
}

/// `GET …/executions/{id}/logs`' query parameters. Every filter narrows within
/// the one execution; none widens past it.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ExecutionLogsQuery {
    /// The cursor: only lines with an `id` greater than this. Omit it for the
    /// first call, then pass the previous response's `next_after`.
    pub after: Option<i64>,
    /// How many lines to return, 1..=2000 (default 500). Outside that range is
    /// a 400, not a clamp.
    #[serde(default = "default_log_limit")]
    #[param(minimum = 1, maximum = 2000, default = 500)]
    pub limit: i64,
    /// Only lines of this task (the YAML's task `id`).
    pub task_id: Option<String>,
    /// Only lines of this task run.
    pub task_run_id: Option<Uuid>,
    /// Only lines at this level or above. The five names, upper case, compared
    /// case-sensitively; anything else is a 400.
    pub min_level: Option<LogLevel>,
}

/// One stored log line. `task_id`, `task_run_id` and `attempt` are all `null`
/// for a line about the execution rather than one run; `attempt` is 0-based,
/// as Kestra's is. A line longer than 15,360 characters was stored as several
/// consecutive lines (W11 D6), so there is no `truncated` flag.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogLineDto {
    /// The cursor value: strictly increasing in the order lines are served.
    pub id: i64,
    pub task_id: Option<String>,
    pub task_run_id: Option<Uuid>,
    pub attempt: Option<i32>,
    pub level: LogLevel,
    /// When the line was produced, by the worker's clock.
    pub ts: DateTime<Utc>,
    pub message: String,
}

impl From<StoredLine> for LogLineDto {
    fn from(l: StoredLine) -> Self {
        Self {
            id: l.id,
            task_id: l.task_id,
            task_run_id: l.task_run_id,
            attempt: l.attempt,
            level: l.level,
            ts: l.ts,
            message: l.message,
        }
    }
}

/// One page of an execution's log.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogPageDto {
    /// In `id` order, at most `limit` of them.
    pub items: Vec<LogLineDto>,
    /// The last returned `id` -- the next call's `after` -- or `null` when
    /// nothing was returned (keep the `after` you sent).
    pub next_after: Option<i64>,
    /// The execution's state (Kestra's wire spelling, `RUNNING`, `SUCCESS`,
    /// ...), read on this call. For display: **do not stop polling on a
    /// terminal state alone** -- see `complete`.
    pub execution_state: String,
    /// The execution is idle -- terminal AND every one of its task runs
    /// terminal, `afterExecution:` runs included -- AND its last run ended
    /// more than the 2 s settle lag ago. **Stop polling when `complete` is
    /// true and `items` is empty**: then every stored line has been served. A
    /// terminal `execution_state` alone is not enough -- an `afterExecution:`
    /// run may still be logging, and the last lines may be younger than the
    /// settle lag and not served yet.
    pub complete: bool,
}

/// An execution's log lines after a cursor, in `id` order.
///
/// **Lines appear about 2 seconds after they are written (the settle lag,
/// [`crate::logs::SETTLE_LAG`]).** A page stops before the first line whose
/// `inserted_at` is less than 2 s old, and serves nothing above it. The reason
/// is out-of-order commits: `bigserial` ids are allocated at insert and
/// committed later, so with two writers (two egret replicas) id 100 can
/// commit after id 101. A client that had read 101 and moved its cursor past
/// it would never see 100. Holding lines back until the lowest unserved one
/// has settled lets the lower id commit first, provided a log write lasts less
/// than half the lag (a log batch is one short `INSERT`).
///
/// **When to stop polling: on `complete && items.is_empty()`, never on a
/// terminal `execution_state` alone.** A run's lines are inserted before its
/// outcome commits (`RunLog::finish` waits on the log writer's flush barrier),
/// so once the execution has been idle (terminal AND every task run terminal,
/// `afterExecution:` runs included; `workflow.execution_is_idle`) for longer
/// than the settle lag, measured from its last run's end, every line it will
/// ever have is visible; `complete` says exactly that. A terminal state seen earlier can
/// still have its last lines inside the lag. `complete` does NOT mean every
/// line the run printed was stored: see spec §11 for the cases (the process
/// dying, a dropped batch, a log-write fault during which the barrier does not
/// wait, a run executed twice).
///
/// Authorisation is exactly `GET …/executions/{id}`'s: `WsContext` (session,
/// membership, the `workflows` feature gate), then `workflow:read`, then
/// [`ExecutionRepo::log_status`] for existence and ownership (the same
/// contract as `ExecutionRepo::get`, one row, no source or task runs) -- **an
/// execution in another workspace is a 404, never a 403** -- and only then the
/// page, which filters on the workspace AND the execution again.
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}/logs",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id"),
        ExecutionLogsQuery,
    ),
    responses(
        (status = 200, description = "Up to `limit` settled lines after `after`, in id order, with the next cursor, the execution's current state, and `complete`. Lines appear about 2 s after they are written (the settle lag guards against out-of-order commits). Stop polling on `complete` with no `items`, not on a terminal state alone: an ended execution's `afterExecution:` runs can still be logging.", body = LogPageDto),
        (status = 400, description = "`limit` outside 1..=2000, a `min_level` that is not one of TRACE/DEBUG/INFO/WARN/ERROR, or a malformed `{id}`/`after`/`task_run_id`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace. **An execution belonging to another workspace answers exactly this**, indistinguishably from one that never existed.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_execution_logs(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<ExecutionLogsQuery>,
) -> EgretResult<Json<LogPageDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    if !(1..=MAX_LOG_PAGE_LIMIT).contains(&q.limit) {
        return Err(Error::InvalidInput(format!(
            "limit must be between 1 and {MAX_LOG_PAGE_LIMIT}, got {}",
            q.limit
        )));
    }
    // Existence AND ownership: `NotFound` for a foreign id, before any line is
    // read. `LogRepo::page` also filters on the workspace, but an empty page
    // would be a 200 -- the existence check is what makes a foreign id a 404.
    let status = st.executions.log_status(ctx.workspace_id.0, id).await?;
    let lines = st
        .logs
        .page(
            ctx.workspace_id.0,
            id,
            &LogQuery {
                after: q.after,
                limit: q.limit,
                task_id: q.task_id,
                task_run_id: q.task_run_id,
                min_level: q.min_level,
            },
        )
        .await?;
    let next_after = lines.last().map(|l| l.id);
    Ok(Json(LogPageDto {
        items: lines.into_iter().map(LogLineDto::from).collect(),
        next_after,
        execution_state: status.state,
        complete: status.complete,
    }))
}

// ---------- Executions: the WRITE surface (W10 Task 5) ----------
//
// The first writes in this subsystem. Everything above is a read; until this
// task the only way to start an execution in production was the unauthenticated
// webhook (`crate::webhook_routes`), which is why the UI had nothing to press.
//
// ```text
// POST /api/ws/{workspace}/workflows/flows/{id}/executions   start
// POST /api/ws/{workspace}/workflows/executions/{id}/resume  resume a pause
// POST /api/ws/{workspace}/workflows/executions/{id}/replay  run it again
// POST /api/ws/{workspace}/workflows/executions/{id}/kill    kill it
// ```
//
// **All four are [`WF_MANAGE`], and the test that says so is a `:read`-only
// holder.** Task 4 proved the read gate with a `:manage`-only holder, because a
// caller with no grants at all is refused by a handler that checks nothing
// (Cedar refuses by default) and so cannot tell the two apart. The mirror image
// is what a write endpoint needs: a `:read` holder is exactly the principal who
// SUCCEEDS if the gate is dropped to `WF_READ` or removed, so
// `workflow_read_alone_cannot_start_resume_or_replay` fails when the claim is
// false and passes only when it is true.
//
// Kill came later, with the kill package (see [`kill_execution`]).
//
// # Which source an execution runs
//
// The DRAFT row's `source` (`workflow.flows.source`), which is the third site
// to take that decision and the first that had a choice: `worker::subflow`'s
// resolver and `crate::webhook_routes` both already read it, and
// `subflow::resolve`'s doc records the binding reason — *a parent and its child
// must not disagree about which version of the world they are in*. A start
// route reading `flow_versions` would run a published parent against a draft
// `Subflow` child. The cost is stated there and is unchanged here: an author
// editing a draft changes what a start button runs, with no publish in between.
//
// **The consequence for REFERENCES, which fix round 1 found unrecorded.**
// Publish resolves each of the six `RefKind`s and rewrites the resolved id into
// the snapshot; a draft start runs the un-rewritten source, so any handler that
// needs the rewritten id is handed the author's name instead. Measured, and
// deliberately narrower than "a start can only run reference-free flows":
//
// * **Exactly one handler of the eight** in `worker/handlers/` parses a spec
//   field as a UUID. `handlers::sql` refuses with *"an execution whose source
//   still holds the author's name was started from unresolved source"*
//   (`handlers/sql.rs:294`), so a flow whose `egret.sql` task names its
//   `database:` by name **starts** and then **that task fails**. The refusal is
//   at the task, not at this route — §6.2's rigidity survives, by that
//   mechanism and not by the executor never holding an unresolved id.
// * **A `Subflow` reference is unaffected**, which is why the general sentence
//   would have been false: `subflow::resolve` resolves by `(namespace, flowId)`
//   against the draft table at run time and never reads a rewritten id.
//
// Recorded in `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`
// §6.1, whose schema line claimed a `flow_version_id` column that has never
// existed (**0** occurrences across `migrations/`; control `flow_key` at
// `0134:32`).
//
// # Kill
//
// `POST .../executions/{id}/kill` shipped with the kill package. Why it was
// held back at first, and what answered each of the seven obstacles recorded
// here before, is `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md`
// §1's table; the route is [`kill_execution`], the transaction
// `worker::kill`.

/// `POST .../flows/{id}/executions`'s body. Optional in whole: a flow with no
/// `inputs:` is started with `{}` and a caller should not have to send a body
/// to say so.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct StartExecutionRequest {
    /// The flow's `inputs:`, as a JSON **object**.
    ///
    /// **A non-object is REFUSED (400), not coerced.**
    /// [`worker::start_execution_with_trigger`](crate::worker::start_execution_with_trigger)
    /// writes `{}` for anything that is not an object, which is right for a
    /// webhook (whose inputs are rendered, never supplied) and wrong for an API
    /// caller: it would turn `"inputs": [1, 2]` into a silently empty start, and
    /// the author would look for their values in the render context and not find
    /// them. The guard is therefore two-sided and its test is too — `[1,2]` must
    /// 400 **and** an absent body and `{}` must both start — because a guard
    /// that only refuses is indistinguishable from one that refuses everything.
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,
}

/// **The one gate every write that RUNS a flow passes through** — workspace
/// scoping, the trash, and both doors a flow can be switched off by.
///
/// # It is one function because two copies already diverged
///
/// Fix round 1, shipped over-acceptance. `start_flow_execution` closed all
/// three and `replay_execution` closed **none** of them: measured as 7
/// occurrences of `deleted_at|disabled` inside the first handler and **0**
/// inside the second, and probed live on one flow — `start` answered **404**
/// where `replay` answered **201**, linking the replacement to a flow that was
/// both trashed and disabled. Replay walked around every door start had just
/// built.
///
/// The fix is not "remember to add the checks to replay too". It is that there
/// is now **one** place to add a door to, and a route that wants to run a flow
/// cannot reach one without coming through here —
/// `replay_closes_the_same_three_doors_start_does` drives both routes over the
/// same fixtures so a fourth door added to one is added to both or the test
/// says so.
///
/// # The sentence that used to be here was false, and the rule it cost
///
/// It read: *"It agrees with `worker::subflow::resolve`, which closes the same
/// two doors in SQL — a flow egret will not start from an API is a flow egret
/// will not reach as a `Subflow` target either."* **`resolve` did not close the
/// same two doors.** Its SQL tests `deleted_at IS NULL AND state <> 'disabled'`
/// — the trash and the **column** — and it never parses the YAML, so
/// `disabled: true` was honoured here and nowhere on the subflow path.
/// Measured: `.disabled` occurred **0** times under `src/worker/` (positive
/// control: `.namespace`, 11 occurrences across 4 files). The failure was
/// concrete — an operator writes `disabled: true`, this route answers 409, and a
/// sibling flow naming that flow as a `Subflow` target ran it.
///
/// **What is true now**, after W10 Task 5 fix round 2: all three doors are
/// closed on both paths, but by **different code in different places**.
/// `resolve` closes the trash and the column in SQL; `subflow::spawn_one`
/// closes the YAML door after `spec::parse`, because `disabled:` is not a
/// column and no predicate can reach it. Each of the three has a test of its
/// own in `tests/worker.rs` — they had **none** before, and deleting either SQL
/// predicate left `--test worker` green.
///
/// **THE RULE, stated here because this is where the next person will write the
/// next such sentence: if your comment asserts what a DIFFERENT function does,
/// that assertion needs its own test, in that function.** A claim about your
/// own code is checked by the tests around it. A claim about somebody else's is
/// checked by nothing at all, and it is read as licence not to look. This is the
/// fourth time in this package that a correct decision was defended by a false
/// mechanism and the first where the false mechanism was a claim about another
/// component — which is the variety that survives review longest, because it
/// reads like a cross-reference rather than an assertion.
///
/// **Refusals, and why each has the status it has**
///
/// * **404** — no such flow in this workspace, or it is in the trash.
///   Indistinguishable from an id that never existed, because a 409 would
///   confirm the id is real.
/// * **400** — the flow's stored source does not parse, so the YAML door
///   cannot be evaluated at all. For `replay` this is a refusal about the
///   **draft**, not about the frozen source it would have run; the two are
///   different strings and only the draft can answer "is this flow switched
///   off right now".
/// * **409** — disabled, by either door.
async fn startable_flow(
    st: &WorkflowState,
    ws: Uuid,
    flow_id: Uuid,
) -> EgretResult<crate::repo::FlowRow> {
    // `FlowRepo::get` carries the workspace predicate, so a foreign id is a
    // `NotFound` here and never a 403 -- the `chart_id` class this package
    // keeps guarding against.
    let flow = st.repo.get(ws, flow_id).await?;
    // **`FlowRepo::get` does NOT filter `deleted_at`** -- its own doc says so,
    // and `get_flow` deliberately still serves a trashed draft so the UI can
    // show what is in the trash. Running one is different.
    if flow.deleted_at.is_some() {
        return Err(Error::NotFound("flow".into()));
    }
    // **Both doors a flow can be switched off by.** The `state` column is
    // honoured here, though nothing writes it yet; `disabled: true` is the
    // YAML's. The runtime switch is per trigger, not per flow
    // (`workflow.disabled_triggers`).
    let parsed = parse(&flow.source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not parse: {}", e.message)))?;
    if parsed.disabled || flow.state == "disabled" {
        return Err(Error::Conflict(
            "Cannot execute flow: flow is disabled.".into(),
        ));
    }
    Ok(flow)
}

/// The `inputs` object a start will actually run with.
///
/// Split out so the refusal has one site and both routes that need the empty
/// default agree on what it is.
fn start_inputs(body: Option<Json<StartExecutionRequest>>) -> EgretResult<serde_json::Value> {
    match body.and_then(|Json(b)| b.inputs) {
        None | Some(serde_json::Value::Null) => Ok(serde_json::json!({})),
        Some(v) if v.is_object() => Ok(v),
        Some(v) => Err(Error::InvalidInput(format!(
            "inputs must be a JSON object, not {}",
            match v {
                serde_json::Value::Array(_) => "an array",
                serde_json::Value::String(_) => "a string",
                serde_json::Value::Bool(_) => "a boolean",
                _ => "a number",
            }
        ))),
    }
}

/// `POST .../flows/{id}/executions` as `multipart/form-data` (W9a spec §7.1,
/// Kestra's shape) -- the schema is for the OpenAPI document only; the
/// handler reads the parts one by one and never builds this type.
///
/// **Each part is named by an input id.** A part with a file name is that
/// `FILE` input's file: it is streamed to
/// `kestra:///{namespace}/{flow}/executions/{execution}/inputs/{input}/{file name}`
/// and the input's value is that URI. Any other part is a text value. A part
/// naming no declared input, or one named twice, is refused naming it.
///
/// The values are `format: binary` (fix round 1, M3), so a generated client
/// types a part as a file (`Blob`) rather than JSON. A text part is a binary
/// value too on the wire: multipart carries bytes either way.
pub struct StartExecutionMultipart(pub std::collections::BTreeMap<String, String>);

impl utoipa::PartialSchema for StartExecutionMultipart {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
        ObjectBuilder::new()
            .description(Some(
                "`POST .../flows/{id}/executions` as `multipart/form-data`. **Each part is \
                 named by an input id.** A part with a file name is that `FILE` input's file: \
                 it is streamed to \
                 `kestra:///{namespace}/{flow}/executions/{execution}/inputs/{input}/{file name}` \
                 and the input's value is that URI. Any other part is a text value. A part \
                 naming no declared input, or one named twice, is refused naming it. One start \
                 carries at most `workflow.storage.max_file_bytes` + 1 MiB in total.",
            ))
            .additional_properties(Some(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary))),
            ))
            .into()
    }
}

impl utoipa::ToSchema for StartExecutionMultipart {}

/// The JSON body's cap on the start route: axum's own default, unchanged by
/// W9a. The multipart cap is [`multipart_body_limit`].
const MAX_JSON_START_BODY_BYTES: usize = 2 * 1024 * 1024;

/// One text part's cap. A text part is held in memory whole -- unlike a file,
/// which is streamed -- so it has a cap of its own, far below the body's.
pub const MAX_TEXT_PART_BYTES: usize = 1024 * 1024;

/// An error response in [`egret_core::http::ErrorBody`]'s shape, for the
/// statuses `egret_core::Error` has no variant for (413, 503).
pub(crate) fn refuse(status: StatusCode, error: String) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    (
        status,
        Json(egret_core::http::ErrorBody { error, code: None }),
    )
        .into_response()
}

fn core_refusal(e: Error) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    e.into_response()
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/{id}/executions",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    request_body(
        description = "The flow's `inputs`. **JSON** (`StartExecutionRequest`, at most 2 MiB): a `FILE` input's value must be a `kestra://` URI readable from the execution being created -- in practice one a `Subflow`, a `Flow` trigger or a replay passes on; any other string is refused naming the input. **`multipart/form-data`** (`StartExecutionMultipart`; one start carries at most `workflow.storage.max_file_bytes` + 1 MiB in total, and each part's headers at most 64 KiB): each part is named by an input id; a part with a file name is that `FILE` input's file (its `allowedFileExtensions` enforced, at most `max_file_bytes`), stored as `inputs/{input}/{file name}` of the new execution; any other part is a text value (at most 1 MiB). **A multipart caller must build the body itself** as a `FormData` (one entry per input id, a `File`/`Blob` for each `FILE` input) and send it with the browser's own `multipart/form-data; boundary=…` header: the generated client serialises this operation's `data` with `JSON.stringify` and sets no `Content-Type`, which reaches the JSON branch.",
        content(
            (StartExecutionRequest = "application/json"),
            (StartExecutionMultipart = "multipart/form-data"),
        ),
    ),
    responses(
        (status = 201, description = "The execution was created and driven as far as it goes without a worker; the body is its detail, read back after the start committed. Its uploaded files and its row were committed in one transaction.", body = ExecutionDto),
        (status = 400, description = "`inputs` is not a JSON object; the flow's stored source does not parse or plan, or uses a task type this deployment does not serve; a `FILE` input's value is not a readable `kestra://` URI or its extension is not allowed; a multipart part names no declared input, names one twice, carries a file for a non-`FILE` input, or has a file name no storage URI can hold; or egret has no internal storage (`[storage]`) for an upload. The message names the input.", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough** — starting an execution is a write.", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace, or it is in the trash. **A flow belonging to another workspace answers exactly this**, indistinguishably from one that never existed.", body = egret_core::http::ErrorBody),
        (status = 409, description = "The flow is disabled — by its `state` column or by its own YAML's `disabled: true`", body = egret_core::http::ErrorBody),
        (status = 413, description = "An uploaded file is over `workflow.storage.max_file_bytes` (the message names the input and the cap), a text part is over 1 MiB, or the JSON body is over 2 MiB. Nothing is created.", body = egret_core::http::ErrorBody),
        (status = 415, description = "A `Content-Type` that is neither `application/json` nor `multipart/form-data`. A request with no `Content-Type` is read as JSON (an empty body is `{}`).", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage failed while an uploaded file was being stored. Nothing is created; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
#[allow(clippy::result_large_err)]
async fn start_flow_execution(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> std::result::Result<(StatusCode, Json<ExecutionDto>), axum::response::Response> {
    use axum::extract::FromRequest as _;
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE)
        .await
        .map_err(core_refusal)?;
    // Scoping, the trash and both disabled doors, in the one place `replay`
    // also has to come through -- see [`startable_flow`] for the over-acceptance
    // that made it a function instead of a run of `if`s in this handler.
    // Before the body is read: a refused start stores nothing.
    let flow = startable_flow(&st, ctx.workspace_id.0, id)
        .await
        .map_err(core_refusal)?;
    let execution_id = if is_multipart(req.headers()) {
        start_multipart(&st, ctx.workspace_id.0, &flow, req).await?
    } else {
        // The JSON body keeps axum's 2 MiB: the route's own limit is off
        // (`DefaultBodyLimit::disable` in [`management_router`]) so that the
        // multipart branch can follow `max_file_bytes`, and a JSON body is
        // buffered whole, so it must not inherit that.
        let req = req.map(|b| {
            axum::body::Body::new(http_body_util::Limited::new(b, MAX_JSON_START_BODY_BYTES))
        });
        // **No `Content-Type` is JSON** (controller decision on W9a Task 7's
        // concern 2). `Option<Json>` alone reads a headerless request as "no
        // body" and would start a headerless JSON body with `{}` -- and the
        // client orval generates for this route's two content types sends no
        // `Content-Type`. So a headerless body is read as JSON: empty is `{}`
        // as before, anything else is parsed (and refused if it is not JSON).
        // An explicit type that is neither JSON nor multipart stays 415 --
        // and a browser's `fetch` labels a string body `text/plain`, so the
        // web UI says `application/json` itself (W7a Task 8, `RunFlow.tsx`).
        let req = if req.headers().contains_key(axum::http::header::CONTENT_TYPE) {
            req
        } else {
            let (mut parts, body) = req.into_parts();
            // Only the limit is a 413 (fix round 1, M2); a body that fails to
            // arrive is the request's, a 400.
            let bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| {
                if is_length_limit(&e) {
                    refuse(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!("the JSON body is over {MAX_JSON_START_BODY_BYTES} bytes"),
                    )
                } else {
                    refuse(
                        StatusCode::BAD_REQUEST,
                        format!("the request body could not be read: {e}"),
                    )
                }
            })?;
            if !bytes.is_empty() {
                parts.headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("application/json"),
                );
            }
            axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes))
        };
        let body = Option::<Json<StartExecutionRequest>>::from_request(req, &st)
            .await
            .map_err(axum::response::IntoResponse::into_response)?;
        let inputs = start_inputs(body).map_err(core_refusal)?;
        // **`start_execution`, not a second creation path.** The webhook calls
        // `start_execution_with_trigger` because it HAS a trigger record; an
        // API start has none, and `start_execution` is that function with
        // `None` already written in. A FILE input given here is checked by
        // `start_execution_in` (`storage::inputs::check_file_inputs`).
        crate::worker::start_execution(
            &st.db.pool,
            ctx.workspace_id.0,
            Some(flow.id),
            &flow.source,
            inputs,
        )
        .await
        .map_err(core_refusal)?
    };
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_execution:start",
        "workflow_execution",
        Some(execution_id.to_string()),
        serde_json::json!({
            "flow_id": flow.id,
            "namespace": flow.namespace,
            "flow_key": flow.flow_id,
        }),
    ));
    // Read back rather than assembled from what was just written: the start
    // drives `advance()` to a fixed point inside its own transaction, so a
    // flow with no runnable task is already terminal by the time this returns
    // and a DTO built from `"CREATED"` would be a guess that is usually wrong.
    let detail = st
        .executions
        .get(ctx.workspace_id.0, execution_id)
        .await
        .map_err(core_refusal)?;
    Ok((
        StatusCode::CREATED,
        Json(ExecutionDto::new(
            detail,
            true,
            st.trigger_flags.worker_enabled,
        )),
    ))
}

/// The multipart start (W9a spec §7.1), in its order: the execution id is
/// generated first; each file part is streamed to `inputs/{input}/{file}` of
/// it ([`crate::storage::inputs::upload_input`]); then ONE transaction
/// inserts the row, checks the FILE inputs, indexes the uploads and runs the
/// first settle (`start_execution_in`). Any failure after an upload began
/// deletes the execution's prefix, best effort: a start that never committed
/// has no execution for the pruner to find. (A client that disconnects drops
/// this future: the in-flight upload is aborted by `put_stream`'s guard and
/// any finished one is left for the orphan sweep.)
#[allow(clippy::result_large_err)]
async fn start_multipart(
    st: &WorkflowState,
    workspace: Uuid,
    flow: &crate::repo::FlowRow,
    req: axum::extract::Request,
) -> std::result::Result<Uuid, axum::response::Response> {
    use axum::extract::FromRequest as _;
    let Some(fs) = crate::storage::current() else {
        return Err(core_refusal(Error::InvalidInput(
            "an uploaded FILE input needs internal storage, and egret has none configured \
             (the `[storage]` section of its configuration)"
                .into(),
        )));
    };
    let parsed = parse(&flow.source).map_err(|e| {
        core_refusal(Error::InvalidInput(format!(
            "the flow does not parse: {}",
            e.message
        )))
    })?;
    let limit = multipart_body_limit(fs.caps);
    let gate = Arc::new(MultipartGate::default());
    let req = req.map(|b| {
        let limited = axum::body::Body::new(http_body_util::Limited::new(b, limit));
        axum::body::Body::from_stream(HeaderBudget {
            inner: Box::pin(limited.into_data_stream()),
            pending: axum::body::Bytes::new(),
            gate: gate.clone(),
            yielded: false,
        })
    });
    let mut multipart = axum::extract::Multipart::from_request(req, &())
        .await
        .map_err(axum::response::IntoResponse::into_response)?;
    let execution = Uuid::new_v4();
    let mut uploading = false;
    let started = read_parts_and_start(
        st,
        &fs,
        workspace,
        flow,
        &parsed,
        execution,
        &mut multipart,
        &gate,
        limit,
        &mut uploading,
    )
    .await;
    if started.is_err() && uploading {
        let prefix = fs.execution_prefix(workspace, &parsed.namespace, &parsed.id, execution);
        if let Err(e) = fs.store.delete_prefix(&prefix).await {
            tracing::warn!(
                prefix = %prefix,
                error = %e,
                "a refused start's uploaded files could not be deleted; the orphan sweep will"
            );
        }
    }
    started
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
async fn read_parts_and_start(
    st: &WorkflowState,
    fs: &crate::storage::FileStore,
    workspace: Uuid,
    flow: &crate::repo::FlowRow,
    parsed: &crate::models::Flow,
    execution: Uuid,
    multipart: &mut axum::extract::Multipart,
    gate: &MultipartGate,
    limit: usize,
    uploading: &mut bool,
) -> std::result::Result<Uuid, axum::response::Response> {
    use futures_util::StreamExt as _;
    let bad = |m: String| core_refusal(Error::InvalidInput(m));
    let mut inputs = serde_json::Map::new();
    let mut files = Vec::new();
    loop {
        // The header budget is spent only here, while multer looks for the
        // next part's headers (I1); a part's data is bounded by its own caps.
        gate.start_headers();
        let next = multipart.next_field().await;
        gate.end_headers();
        let Some(field) =
            next.map_err(|e| multipart_refusal(gate, limit, e.status(), e.body_text()))?
        else {
            break;
        };
        let Some(name) = field.name().map(str::to_string) else {
            return Err(bad(
                "a multipart part has no name: each part is named by the input it is for".into(),
            ));
        };
        let Some(input) = parsed.inputs.iter().find(|i| i.id == name) else {
            return Err(bad(format!(
                "the flow declares no input `{name}`: each multipart part is named by an input id"
            )));
        };
        if inputs.contains_key(&name) {
            return Err(bad(format!(
                "input `{name}` is given by more than one part"
            )));
        }
        let value = match field.file_name().map(str::to_string) {
            Some(file_name) => {
                *uploading = true;
                let mut body = tokio_util::io::StreamReader::new(
                    field.map(|chunk| chunk.map_err(std::io::Error::other)),
                );
                let stored = crate::storage::inputs::upload_input(
                    fs,
                    workspace,
                    &parsed.namespace,
                    &parsed.id,
                    execution,
                    input,
                    &file_name,
                    &mut body,
                )
                .await
                .map_err(|e| upload_refusal(e, gate, limit))?;
                let uri = stored.uri.clone();
                files.push(stored);
                uri
            }
            None => read_text_part(field, &name, gate, limit).await?,
        };
        inputs.insert(name, serde_json::Value::String(value));
    }
    // W9c Task 9: in `worker::txn::transaction!`, so a first settle that
    // needs a read is rolled back, the read fetched, and the start run again
    // -- with the same id and the same uploads: they happened above, before
    // any transaction, and are not repeated.
    crate::worker::txn::transaction!(&st.db.pool, |at| {
        crate::worker::start_execution_in_with(
            at,
            crate::worker::NewExecution {
                workspace_id: workspace,
                flow_id: Some(flow.id),
                source: &flow.source,
                inputs: serde_json::Value::Object(inputs.clone()),
                trigger: None,
                scheduled_date: None,
                trigger_execution_id: None,
                execution_depth: 0,
                flow_trigger_event_id: None,
                born_failed: false,
                replayed_from_execution_id: None,
                id: Some(execution),
                input_files: files.clone(),
                labels: None,
                backfill_id: None,
                variables: None,
            },
        )
        .await
    })
    .map_err(core_refusal)
}

/// A text part, whole, at most [`MAX_TEXT_PART_BYTES`], as UTF-8.
#[allow(clippy::result_large_err)]
async fn read_text_part(
    mut field: axum::extract::multipart::Field<'_>,
    name: &str,
    gate: &MultipartGate,
    limit: usize,
) -> std::result::Result<String, axum::response::Response> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| multipart_refusal(gate, limit, e.status(), e.body_text()))?
    {
        if bytes.len() + chunk.len() > MAX_TEXT_PART_BYTES {
            return Err(refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Invalid value for input `{name}`: a text part is at most \
                     {MAX_TEXT_PART_BYTES} bytes; upload a file for a FILE input"
                ),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| {
        core_refusal(Error::InvalidInput(format!(
            "Invalid value for input `{name}`: a text part must be UTF-8"
        )))
    })
}

/// [`crate::storage::inputs::UploadError`] as the response: the caller's
/// errors are 4xx naming the input, a store failure is a 503 (spec §11: not
/// the caller's fault, and retryable). A body cut off by the route's own
/// limit is the BODY's refusal, naming that limit (M1).
fn upload_refusal(
    e: crate::storage::inputs::UploadError,
    gate: &MultipartGate,
    limit: usize,
) -> axum::response::Response {
    use crate::storage::inputs::UploadError;
    match e {
        UploadError::Author(m) => core_refusal(Error::InvalidInput(m)),
        UploadError::TooLarge(m) => refuse(StatusCode::PAYLOAD_TOO_LARGE, m),
        UploadError::Body { message, source } => {
            let status = source
                .get_ref()
                .and_then(|e| e.downcast_ref::<axum::extract::multipart::MultipartError>())
                .map(|m| m.status())
                .unwrap_or(StatusCode::BAD_REQUEST);
            multipart_refusal(gate, limit, status, message)
        }
        UploadError::Store(m) => {
            tracing::warn!(error = %m, "a FILE input upload failed in internal storage");
            refuse(StatusCode::SERVICE_UNAVAILABLE, m)
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/executions/{id}/resume",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id"),
    ),
    responses(
        (status = 200, description = "Every paused `Pause` run was released at once and ends `SUCCESS`, whatever its `pauseDuration:` or `timeout:` (Kestra: a pause resumed by hand always succeeds); the execution then runs on. The body is its detail afterwards.", body = ExecutionDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, indistinguishably from one that never existed", body = egret_core::http::ErrorBody),
        (status = 409, description = "The execution is not PAUSED (or its pause ended before the resume took the execution's lock), or it is PAUSED only because a `LoopUntil` waits for its next iteration and no `Pause` task is waiting. A resume that changes nothing must not answer 200.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
/// Resume a `PAUSED` execution by hand.
///
/// W7b Task 3 fix round 1, the user's ruling of 2026-10-11, after review
/// found the old route released nothing a human needs: it only re-settled,
/// which ends a pause whose deadline had passed -- what the waker does anyway.
/// It now calls [`worker::resume_by_hand`](crate::worker::resume_by_hand),
/// Kestra's `ExecutionService.resume`: every paused `Pause` run ends `SUCCESS`
/// now, through the natural end's own path, and the execution settles on.
///
/// The `PAUSED` check is made twice: here off the scoped read (so a foreign id
/// never reaches the unscoped worker, and the common refusal is cheap), and
/// again by the worker under the execution's lock, which decides the race with
/// the waker. A pause the waker ended first is a 409, like any execution that
/// is not paused.
async fn resume_execution(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<Json<ExecutionDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    // The containment check AND the state check off one scoped read, so a
    // foreign id never reaches `worker::resume_by_hand` -- which is unscoped by
    // workspace, as every `worker::` entry point is.
    let before = st.executions.get(ctx.workspace_id.0, id).await?;
    if crate::worker::from_wire(&before.execution.state) != Some(RunState::Paused) {
        return Err(Error::Conflict(format!(
            "execution {id} is {}, not PAUSED; only a paused execution can be resumed",
            before.execution.state
        )));
    }
    match crate::worker::resume_by_hand(&st.db.pool, id).await? {
        crate::worker::Resumed::Released(_) => {}
        crate::worker::Resumed::NotPaused => {
            return Err(Error::Conflict(format!(
                "execution {id} is no longer PAUSED; its pause ended before the resume reached it"
            )));
        }
        // An execution paused only by a waiting `LoopUntil` (W7b Task 3 fix
        // round 2): a 409, never a 200 that changed nothing.
        crate::worker::Resumed::NoPauseWaiting => {
            return Err(Error::Conflict(
                "Nothing to resume: no Pause task is waiting. The execution is waiting on a loop condition, which ends on its own schedule."
                    .to_string(),
            ));
        }
    }
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_execution:resume",
        "workflow_execution",
        Some(id.to_string()),
        serde_json::json!({ "from_state": before.execution.state }),
    ));
    let detail = st.executions.get(ctx.workspace_id.0, id).await?;
    Ok(Json(ExecutionDto::new(
        detail,
        true,
        st.trigger_flags.worker_enabled,
    )))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/executions/{id}/replay",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id to replay"),
    ),
    responses(
        (status = 201, description = "A NEW execution, started from the replayed one's frozen source and inputs. The body is the new execution, not the old one — read its `id`.", body = ExecutionDto),
        (status = 400, description = "The execution is a **loop iteration** (replay its loop root instead); or the **flow's current draft** does not parse, so whether it is disabled cannot be decided; or the replayed execution's frozen source no longer plans, or uses a task type this deployment does not serve", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, indistinguishably from one that never existed — **or its flow is in the trash, or no longer exists at all**. A replay is a run of the flow and answers to the flow's current state.", body = egret_core::http::ErrorBody),
        (status = 409, description = "The execution has not finished, **or its flow is disabled** (by the `state` column or by its own YAML's `disabled: true`). A replay of something still running would put two copies of one flow in flight at once.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
/// Run a finished execution again.
///
/// # It is a start, deliberately, and from the FROZEN source
///
/// `worker::start_execution` with `workflow.executions.source` and
/// `workflow.executions.inputs` — the row's own copies, not a re-read of the
/// draft. `worker::replay::spawn_due` takes the same decision for the
/// `CREATE_NEW_EXECUTION` replacement and states the reason: *"a replay that
/// picked up an edit made since would be a different flow with the same
/// trace."* Starting from the draft is what `POST .../flows/{id}/executions`
/// is for, and the two routes exist to be different.
///
/// # The link is `replayed_from_execution_id`, not a parent
///
/// `replay::spawn_due` links its replacement with `parent_execution_id` set and
/// `parent_run_ix` NULL. That slot is **one per execution and already spoken
/// for**: migration 0138's `workflow_executions_loop_run_idx` is
/// `UNIQUE (parent_execution_id, parent_run_ix, loop_index) NULLS NOT DISTINCT
/// WHERE parent_execution_id IS NOT NULL`, so `(X, NULL, NULL)` can exist once,
/// and `spawn_due` reads exactly that to answer "it already exists". A manual
/// replay that took the slot would make a later retry-replay of the same
/// execution fail a unique constraint, and a second manual replay fail one too.
///
/// So the replacement has no parent, and the link has a column of its own:
/// `replayed_from_execution_id` (migration 0166, W9a spec §4.4 item 3), written
/// here through `start_execution_in` in one transaction with the row. It is
/// what lets a replay read its source's files (a replay copies its source's
/// inputs verbatim, and those may be `kestra://` URIs of the source). The audit
/// record still carries both ids.
///
/// # Not a loop iteration
///
/// An iteration (`parent_execution_id` with a `loop_index`) is refused (400)
/// naming its loop root: it is a part of the root's run, `replay::spawn_due`
/// never replays one, and a replay of it would be a top-level run of the
/// loop's body alone.
///
/// # Terminal only
///
/// A non-terminal execution is refused (409) rather than replayed: two RUNNING
/// executions of one flow, a replay and its original, would be in flight at
/// once, and the narrow door cannot over-accept. `RunState::is_terminal` is the predicate — read through
/// `worker::from_wire`, which is derived from `RunState::ALL`, so a state added
/// to the enum is classified here without anyone remembering to come back.
async fn replay_execution(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> EgretResult<(StatusCode, Json<ExecutionDto>)> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let source = st.executions.get(ctx.workspace_id.0, id).await?;
    // **A loop iteration is not replayed** (user ruling, W9a Task 3 fix round
    // 1): it is a part of its loop root's run, not a run of the flow, and
    // `replay::spawn_due` never replays one either. Its loop root is.
    if let (Some(root), Some(index)) = (
        source.execution.parent_execution_id,
        source.execution.loop_index,
    ) {
        return Err(Error::InvalidInput(format!(
            "execution {id} is iteration {index} of a Loop in execution {root}, not a run of \
             the flow, and cannot be replayed on its own; replay its loop root {root}"
        )));
    }
    // **The same gate `start_flow_execution` passes**, and fix round 1 is what
    // put it here: this handler used to check terminality and NOTHING else, so
    // a replay ran a flow that was trashed, disabled, or both — every door
    // start had just closed, walked around by the route beside it. A replay is
    // a run of the flow, so it answers to the flow's current state, not to the
    // state the flow was in when the execution being replayed started.
    //
    // **The frozen source is still what RUNS** (see this handler's doc); the
    // flow row is consulted only for permission to run at all, and the returned
    // row is deliberately discarded.
    //
    // An execution whose `flow_id` is NULL names no flow, so neither door can
    // be asked. `executions_flow_id_fkey` is `ON DELETE SET NULL`, and
    // `FlowRepo::purge` (deleting a trashed flow permanently, KI-119) issues
    // that DELETE, so this arm is reached by replaying an execution of a
    // purged flow — and it refuses rather than running, because a flow that
    // has been hard-deleted is a stronger "this is gone" than a trashed one,
    // not a weaker one.
    let flow_id = source
        .execution
        .flow_id
        .ok_or_else(|| Error::NotFound("flow".into()))?;
    let _ = startable_flow(&st, ctx.workspace_id.0, flow_id).await?;
    // An unknown spelling is refused with the non-terminal ones rather than
    // treated as terminal: `from_wire` answers `None` only for a state this
    // build does not have, and guessing "finished" about it is the
    // over-accepting half.
    if !crate::worker::from_wire(&source.execution.state).is_some_and(RunState::is_terminal) {
        return Err(Error::Conflict(format!(
            "execution {id} is {} and has not finished; only a terminal execution can be replayed",
            source.execution.state
        )));
    }
    // KI-112: the replay runs with the stored SECRET values, not the mask the
    // row shows; the start masks and seals them again for the new row.
    let (inputs, _) = crate::secret_inputs::reveal(
        source.execution.inputs.clone(),
        source.secret_inputs.as_deref(),
    )?;
    // W9c Task 9: in `worker::txn::transaction!`, so a first settle that
    // needs a read is rolled back, the read fetched, and the start run again.
    let replacement = crate::worker::txn::transaction!(&st.db.pool, |at| {
        crate::worker::start_execution_in_with(
            at,
            crate::worker::NewExecution {
                workspace_id: ctx.workspace_id.0,
                flow_id: source.execution.flow_id,
                source: &source.source,
                inputs: inputs.clone(),
                trigger: None,
                scheduled_date: None,
                trigger_execution_id: None,
                execution_depth: 0,
                flow_trigger_event_id: None,
                born_failed: false,
                // The link (this handler's doc), committed with the row.
                replayed_from_execution_id: Some(id),
                id: None,
                input_files: Vec::new(),
                // The run's own labels travel with it; the backfill that fired
                // the original does not (W7f spec §3.3).
                labels: source.execution.labels.clone(),
                backfill_id: None,
                // D3 (final review I3): the run's `variables` travel with it,
                // as the retry replay's do and Kestra's `childExecution`'s.
                variables: source.variables.clone(),
            },
        )
        .await
    })?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_execution:replay",
        "workflow_execution",
        Some(replacement.to_string()),
        // Both ids; `replayed_from_execution_id` carries the link in the data.
        serde_json::json!({ "replayed_from": id, "replacement": replacement }),
    ));
    let detail = st.executions.get(ctx.workspace_id.0, replacement).await?;
    Ok((
        StatusCode::CREATED,
        Json(ExecutionDto::new(
            detail,
            true,
            st.trigger_flags.worker_enabled,
        )),
    ))
}

/// `POST .../executions/{id}/kill`'s query.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct KillQuery {
    /// Also kill the execution's child executions -- its `Loop` iterations
    /// and its `Subflow` children, recursively (default `true`). A replay of
    /// it, or an execution a Flow trigger fired on it, is not a child. With
    /// `false`, the children run on, and what they report when they end
    /// changes nothing in the killed parent.
    pub cascade: Option<bool>,
}

/// `ErrorBody.code` of the kill route's 409 (spec D3).
pub const EXECUTION_TERMINATED_CODE: &str = "execution_terminated";

/// `ErrorBody.code` of the kill route's RETRYABLE 409 (end hooks #2b, B3 fix
/// round 1, I1): the execution has ended and its `afterExecution:` is still
/// running, but its only live task is a block being prepared or finished,
/// which cannot be killed until that step has landed. Nothing is written.
pub const KILL_NOT_APPLIED_CODE: &str = "kill_not_applied";

/// The kill route's error: every [`egret_core::Error`], plus the coded 409.
#[derive(Debug)]
pub enum KillError {
    Core(Error),
    /// The execution is terminal, in this state.
    Terminated(Uuid, String),
    /// The execution is terminal and its `afterExecution:` runs, but the kill
    /// could not be applied now ([`crate::worker::KillOutcome::NotApplied`]):
    /// a retryable 409 with [`KILL_NOT_APPLIED_CODE`].
    NotApplied(Uuid),
}

impl From<Error> for KillError {
    fn from(e: Error) -> Self {
        KillError::Core(e)
    }
}

impl axum::response::IntoResponse for KillError {
    fn into_response(self) -> axum::response::Response {
        match self {
            KillError::Core(e) => e.into_response(),
            KillError::Terminated(id, state) => (
                StatusCode::CONFLICT,
                Json(egret_core::http::ErrorBody {
                    error: Error::Conflict(format!(
                        "execution {id} is {state} and has already ended; there is nothing to kill"
                    ))
                    .to_string(),
                    code: Some(EXECUTION_TERMINATED_CODE.to_string()),
                }),
            )
                .into_response(),
            KillError::NotApplied(id) => (
                StatusCode::CONFLICT,
                Json(egret_core::http::ErrorBody {
                    error: Error::Conflict(format!(
                        "execution {id}: the kill did not apply: an afterExecution block is \
                         being prepared or finished; retry"
                    ))
                    .to_string(),
                    code: Some(KILL_NOT_APPLIED_CODE.to_string()),
                }),
            )
                .into_response(),
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/executions/{id}/kill",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id"),
        KillQuery,
    ),
    responses(
        (status = 202, description = "The kill is recorded. The body is the execution afterwards: `KILLING` while a running task still has to be stopped by a worker (it then becomes `KILLED` on its own), or `KILLED` if nothing was running. Every run that was retrying, paused or not yet started is `KILLED` already. On an execution that has already ended but whose `afterExecution:` tasks still run, only those tasks are killed: the body keeps the execution's own final state (e.g. `SUCCESS`).", body = ExecutionDto),
        (status = 400, description = "`cascade` is not `true` or `false`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, indistinguishably from one that never existed", body = egret_core::http::ErrorBody),
        (status = 409, description = "The execution has already ended and none of its `afterExecution:` tasks is running (code `execution_terminated`), or its `afterExecution:` kill cannot apply yet because a block is being prepared or finished (code `kill_not_applied`; retry). Nothing is written.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
/// Kill an execution, or the `afterExecution:` tasks of one that has ended.
///
/// Tasks retrying, paused or not yet started end `KILLED` at once; a running
/// task is stopped by a worker -- the one running it, or the next one to claim
/// it -- and the execution is `KILLING` until it has. With `cascade` (the default) its `Loop` iterations
/// and `Subflow` child executions are killed the same way. On an ended
/// execution whose `afterExecution:` tasks still run, only those are killed
/// and the execution's state is not changed; the audit `outcome` is then that
/// state, with `after_execution` saying what happened to the tasks.
async fn kill_execution(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<KillQuery>,
) -> std::result::Result<(StatusCode, Json<ExecutionDto>), KillError> {
    // Workflow kill spec §3. The scoped read keeps a foreign id from ever
    // reaching the worker. The terminal check is `worker::kill_execution`'s
    // alone, under the execution's lock: since end hooks #2b (the user's
    // answer to Q3, 2026-10-27) a terminal execution whose `afterExecution:`
    // is still running is killable, and only its plan can tell.
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let before = st.executions.get(ctx.workspace_id.0, id).await?;
    let cascade = q.cascade.unwrap_or(true);
    let (outcome, forced) =
        crate::worker::kill_execution_reporting(&st.db.pool, ctx.workspace_id.0, id, cascade)
            .await?;
    // `after_execution`: a kill of the post-terminal phase (Q3) leaves the
    // execution's state alone, so the outcome is the execution's own state
    // and the record says what happened to the phase.
    let (outcome, after_execution) = match outcome {
        crate::worker::KillOutcome::Killing => (RunState::Killing, None),
        crate::worker::KillOutcome::Killed => (RunState::Killed, None),
        crate::worker::KillOutcome::AfterExecution { state, stopped } => (
            state,
            Some(if stopped {
                RunState::Killed
            } else {
                RunState::Killing
            }),
        ),
        crate::worker::KillOutcome::Terminated(s) => {
            return Err(KillError::Terminated(id, s.wire().to_string()));
        }
        crate::worker::KillOutcome::NotApplied(_) => return Err(KillError::NotApplied(id)),
    };
    let mut audited = serde_json::json!({
        "from_state": before.execution.state,
        "cascade": cascade,
        "outcome": outcome.wire(),
    });
    if let Some(phase) = after_execution {
        audited["after_execution"] = serde_json::Value::String(phase.wire().to_string());
    }
    // Debt D4: only when the kill closed an execution it could not load by
    // force, so every other record keeps its shape.
    if forced {
        audited["forced"] = serde_json::Value::Bool(true);
    }
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "workflow_execution:kill",
        "workflow_execution",
        Some(id.to_string()),
        audited,
    ));
    let detail = st.executions.get(ctx.workspace_id.0, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ExecutionDto::new(
            detail,
            true,
            st.trigger_flags.worker_enabled,
        )),
    ))
}

/// One namespace of the workspace (W7d spec §3.1). egret has no namespace
/// table: a namespace exists while a flow that is not deleted, a live
/// namespace file or directory (even an empty one) or a live KV key is in
/// it, or while it is a parent by dots of one that is.
///
/// **The counts are this namespace's OWN**, never rolled up from its
/// children: `company` with one flow in `company.team` answers `flows: 0`.
///
/// Named `Workflow…` because the aggregated OpenAPI document already has a
/// `NamespaceDto` (the catalog's), and a second one of that name is dropped.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WorkflowNamespaceDto {
    pub namespace: String,
    /// Flows in exactly this namespace that are not deleted.
    pub flows: i64,
    /// Live namespace files (a path whose latest revision is not deleted;
    /// several revisions of one path are one file; directories are not counted).
    pub files: i64,
    /// Live KV keys (the latest revision neither deleted nor expired).
    pub kv_keys: i64,
    /// False when the namespace is present only as a parent by dots of
    /// another (its counts are then all zero). True for a source, whose
    /// counts can all be zero when it holds only a live empty directory.
    pub own: bool,
}

impl From<crate::repo::NamespaceRow> for WorkflowNamespaceDto {
    fn from(r: crate::repo::NamespaceRow) -> Self {
        Self {
            namespace: r.namespace,
            flows: r.flows,
            files: r.files,
            kv_keys: r.kv_keys,
            own: r.own,
        }
    }
}

/// One namespace as its detail route answers it (W7d spec §4.5): the
/// listing's row ([`WorkflowNamespaceDto`]) and whether the caller may
/// manage it. The listing carries no `can_manage`; this is a type of its own
/// so that the client's type of each answer is exact.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WorkflowNamespaceDetailDto {
    pub namespace: String,
    /// Flows in exactly this namespace that are not deleted.
    pub flows: i64,
    /// Live namespace files (directories are not counted).
    pub files: i64,
    /// Live KV keys (the latest revision neither deleted nor expired).
    pub kv_keys: i64,
    /// False when the namespace is present only as a parent by dots of
    /// another.
    pub own: bool,
    /// The caller holds `workflow:manage` in this workspace: the same PDP
    /// decision as the namespace file and KV write routes, which still check
    /// for themselves. The UI disables its writes when this is false.
    pub can_manage: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListNamespacesQuery {
    pub prefix: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces",
    tag = "workflow",
    params(
        WorkspacePath,
        ("prefix" = Option<String>, Query, description = "Only this namespace and the ones beneath it, dot-anchored: `company` matches `company` and `company.team`, never `companyx`. Empty means no filter."),
    ),
    responses(
        (status = 200, description = "The workspace's namespaces, from flows that are not deleted, live namespace files and live KV keys, plus every parent by dots (`own: false`); a namespace holding only a live directory is listed (`own: true`, `files: 0`); sorted by namespace (byte order). Counts are each namespace's own, never its children's.", body = Vec<WorkflowNamespaceDto>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_namespaces(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListNamespacesQuery>,
) -> EgretResult<Json<Vec<WorkflowNamespaceDto>>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let prefix = q.prefix.as_deref().filter(|p| !p.is_empty());
    let rows = st.repo.namespaces(ctx.workspace_id.0, prefix).await?;
    Ok(Json(
        rows.into_iter().map(WorkflowNamespaceDto::from).collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
    ),
    responses(
        (status = 200, description = "The namespace's own counts; `own: false` when it is only a parent by dots of another; `can_manage` whether the caller holds workflow:manage", body = WorkflowNamespaceDetailDto),
        (status = 400, description = "Not a valid namespace name (`^[a-z0-9][a-z0-9._-]*`, 1-150 characters)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "Nothing lives in this namespace or beneath it in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_namespace(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
) -> EgretResult<Json<WorkflowNamespaceDetailDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    // The flow grammar, not W9b's `check_namespace`: every namespace a flow
    // can have (an empty segment or a `_files` one included) must be
    // addressable here, since the listing shows it.
    if !crate::spec::validate::is_valid_namespace(&namespace) {
        return Err(Error::InvalidInput(format!(
            "namespace '{namespace}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long"
        )));
    }
    let row = st
        .repo
        .namespace(ctx.workspace_id.0, &namespace)
        .await?
        .ok_or_else(|| Error::NotFound(format!("namespace '{namespace}'")))?;
    let can_manage = caller_may_manage(&st, &ctx).await?;
    Ok(Json(WorkflowNamespaceDetailDto {
        namespace: row.namespace,
        flows: row.flows,
        files: row.files,
        kv_keys: row.kv_keys,
        own: row.own,
        can_manage,
    }))
}

/// One flow of a namespace's dependencies graph (W7d spec §3.4).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NamespaceDependencyNodeDto {
    /// The flow's id (the uuid every flow route takes); edges name it.
    pub id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    /// The flow is switched off: its state is `disabled` or its YAML says
    /// `disabled: true`.
    pub disabled: bool,
    /// The flow is in the namespace or beneath it (dot-anchored); false for a
    /// flow elsewhere present only for an edge to or from one that is.
    pub in_namespace: bool,
    /// `Subflow` tasks whose `namespace` or `flowId` is an expression
    /// (`{{ }}`/`{% %}`): only an execution knows their target, so they draw
    /// no edge.
    pub dynamic_subflows: i64,
    /// The flow's saved draft does not parse: it is drawn so edges into it
    /// show, but its own edges are unknown.
    pub unparsable: bool,
}

/// What an edge of the dependencies graph means.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceDependencyKind {
    /// `source` calls `target` through a `Subflow` task with a literal
    /// target, anywhere in its task tree (`errors:` included).
    Subflow,
    /// `target` has a `Flow` trigger whose `dependsOn` names `source` (both
    /// `namespace` and `flowId`).
    FlowTrigger,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NamespaceDependencyEdgeDto {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: NamespaceDependencyKind,
}

/// A namespace's flow-to-flow graph, built from the flows' saved drafts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NamespaceDependenciesDto {
    /// The namespace's flows and its children's, then every flow elsewhere
    /// with a direct edge to or from one of them; by namespace, then flow id.
    pub nodes: Vec<NamespaceDependencyNodeDto>,
    /// Edges with at least one end in the namespace; one per (source,
    /// target, kind).
    pub edges: Vec<NamespaceDependencyEdgeDto>,
    /// Drafts in the WORKSPACE (not only this namespace) whose YAML does not
    /// parse. Each is still a node (`unparsable: true`) when it is in the
    /// namespace or an edge reaches it, but its own edges are unknown.
    pub unparsable: i64,
}

impl From<crate::namespaces::dependencies::Dependencies> for NamespaceDependenciesDto {
    fn from(d: crate::namespaces::dependencies::Dependencies) -> Self {
        use crate::namespaces::dependencies::EdgeKind;
        Self {
            nodes: d
                .nodes
                .into_iter()
                .map(|n| NamespaceDependencyNodeDto {
                    id: n.id,
                    namespace: n.namespace,
                    flow_id: n.flow_id,
                    disabled: n.disabled,
                    in_namespace: n.in_namespace,
                    dynamic_subflows: n.dynamic_subflows,
                    unparsable: n.unparsable,
                })
                .collect(),
            edges: d
                .edges
                .into_iter()
                .map(|e| NamespaceDependencyEdgeDto {
                    source: e.source,
                    target: e.target,
                    kind: match e.kind {
                        EdgeKind::Subflow => NamespaceDependencyKind::Subflow,
                        EdgeKind::FlowTrigger => NamespaceDependencyKind::FlowTrigger,
                    },
                })
                .collect(),
            unparsable: d.unparsable,
        }
    }
}

/// The `code` of the 422 a workspace over
/// [`crate::namespaces::dependencies::MAX_FLOWS`] flows answers.
pub const DEPENDENCIES_TOO_LARGE_CODE: &str = "dependencies_too_large";

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/dependencies",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; its children (dot-anchored) are inside it"),
    ),
    responses(
        (status = 200, description = "The flow-to-flow graph of the namespace, built from every flow's saved draft: `subflow` edges from `Subflow` tasks with a literal target, `flow_trigger` edges from `Flow` triggers' `dependsOn` entries naming a flow. A namespace with no flows answers an empty graph.", body = NamespaceDependenciesDto),
        (status = 400, description = "Not a valid namespace name (`^[a-z0-9][a-z0-9._-]*`, 1-150 characters)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 422, description = "The workspace has more than 2000 flows (not deleted), every one of which would be read (`code: dependencies_too_large`)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn namespace_dependencies(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
) -> std::result::Result<Json<NamespaceDependenciesDto>, DependenciesRouteError> {
    use crate::namespaces::dependencies::{build_blocking, MAX_FLOWS};
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    if !crate::spec::validate::is_valid_namespace(&namespace) {
        return Err(Error::InvalidInput(format!(
            "namespace '{namespace}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long"
        ))
        .into());
    }
    let drafts = st
        .repo
        .dependency_drafts(ctx.workspace_id.0, MAX_FLOWS as i64 + 1)
        .await?;
    if drafts.len() > MAX_FLOWS {
        return Err(DependenciesRouteError::TooManyFlows);
    }
    // Parsing up to MAX_FLOWS drafts is seconds of CPU: never on this worker.
    Ok(Json(build_blocking(namespace, drafts).await?.into()))
}

/// The error of the dependencies route: every [`egret_core::Error`], plus
/// the 422 `egret_core::Error` has no variant for.
#[derive(Debug)]
pub enum DependenciesRouteError {
    Core(Error),
    TooManyFlows,
}

impl From<Error> for DependenciesRouteError {
    fn from(e: Error) -> Self {
        DependenciesRouteError::Core(e)
    }
}

impl axum::response::IntoResponse for DependenciesRouteError {
    fn into_response(self) -> axum::response::Response {
        use crate::namespaces::dependencies::MAX_FLOWS;
        match self {
            DependenciesRouteError::Core(e) => e.into_response(),
            DependenciesRouteError::TooManyFlows => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(egret_core::http::ErrorBody {
                    error: format!(
                        "this workspace has more than {MAX_FLOWS} flows; the dependencies graph \
                         reads every flow's draft and is limited to {MAX_FLOWS}"
                    ),
                    code: Some(DEPENDENCIES_TOO_LARGE_CODE.to_string()),
                }),
            )
                .into_response(),
        }
    }
}

/// The Workflow management router. Mount under `/api` so the live paths are
/// `/api/ws/{workspace}/workflows/...` — see this module's doc comment for
/// the feature-gate convention that depends on it.
pub fn management_router() -> Router<WorkflowState> {
    Router::new()
        .route("/ws/{workspace}/workflows/schema", get(get_flow_schema))
        .route(
            "/ws/{workspace}/workflows/flows",
            get(list_flows).post(create_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/validate",
            post(validate_unsaved_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}",
            get(get_flow).put(update_flow).delete(delete_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/restore",
            post(restore_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/permanent",
            axum::routing::delete(purge_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/validate",
            post(validate_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/graph",
            post(graph_unsaved_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/graph",
            get(get_flow_graph),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/publish",
            post(publish_flow),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/executions",
            // The handler applies its own limit per content type -- 2 MiB for
            // JSON, `max_file_bytes` + 1 MiB for multipart (W9a spec §7.1) --
            // so the router's default is off here and only here.
            post(start_flow_execution).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/versions",
            get(list_flow_versions),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/triggers",
            get(crate::triggers::routes::list_flow_triggers),
        )
        .route(
            "/ws/{workspace}/workflows/triggers",
            get(crate::triggers::routes::list_triggers),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled",
            put(crate::triggers::routes::set_trigger_disabled),
        )
        .route(
            "/ws/{workspace}/workflows/triggers/disabled",
            // The bulk body's own cap, as the KV bulk delete's.
            put(axum::handler::Handler::layer(
                crate::triggers::routes::set_triggers_disabled,
                axum::extract::DefaultBodyLimit::max(
                    crate::triggers::routes::MAX_BULK_TRIGGERS_BODY_BYTES,
                ),
            )),
        )
        // W7f: the backfill routes, each body under its own cap.
        .route(
            "/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill",
            post(axum::handler::Handler::layer(
                crate::triggers::backfill_routes::create_trigger_backfill,
                axum::extract::DefaultBodyLimit::max(
                    crate::triggers::backfill_routes::MAX_BACKFILL_BODY_BYTES,
                ),
            ))
            .delete(crate::triggers::backfill_routes::delete_trigger_backfill),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill/paused",
            put(axum::handler::Handler::layer(
                crate::triggers::backfill_routes::set_trigger_backfill_paused,
                axum::extract::DefaultBodyLimit::max(
                    crate::triggers::backfill_routes::MAX_BACKFILL_BODY_BYTES,
                ),
            )),
        )
        .route(
            "/ws/{workspace}/workflows/triggers/backfill/paused",
            put(axum::handler::Handler::layer(
                crate::triggers::backfill_routes::set_triggers_backfill_paused,
                axum::extract::DefaultBodyLimit::max(
                    crate::triggers::backfill_routes::MAX_BACKFILL_BODY_BYTES,
                ),
            )),
        )
        .route(
            "/ws/{workspace}/workflows/triggers/backfill/delete",
            post(axum::handler::Handler::layer(
                crate::triggers::backfill_routes::delete_triggers_backfill,
                axum::extract::DefaultBodyLimit::max(
                    crate::triggers::backfill_routes::MAX_BACKFILL_BODY_BYTES,
                ),
            )),
        )
        .route(
            "/ws/{workspace}/workflows/flows/{id}/versions/{version}",
            get(get_flow_version),
        )
        .route("/ws/{workspace}/workflows/executions", get(list_executions))
        .route(
            "/ws/{workspace}/workflows/executions/{id}",
            get(get_execution),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/graph",
            get(get_execution_graph),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/logs",
            get(get_execution_logs),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/file",
            get(crate::storage::routes::download_execution_file),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/file/metas",
            get(crate::storage::routes::execution_file_metas),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/file/preview",
            get(crate::storage::routes::preview_execution_file),
        )
        .route("/ws/{workspace}/workflows/namespaces", get(list_namespaces))
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}",
            get(get_namespace),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/dependencies",
            get(namespace_dependencies),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files",
            get(crate::storage::nsfiles::routes::download_namespace_file)
                // The upload applies its own limit (`max_file_bytes` + 1 MiB,
                // W9a's multipart bound), so the router's default is off for
                // it, as for the start route.
                .post(crate::storage::nsfiles::routes::upload_namespace_file)
                .layer(axum::extract::DefaultBodyLimit::disable())
                .put(crate::storage::nsfiles::routes::move_namespace_file)
                .delete(crate::storage::nsfiles::routes::delete_namespace_file),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files/stats",
            get(crate::storage::nsfiles::routes::namespace_file_stats),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files/revisions",
            get(crate::storage::nsfiles::routes::namespace_file_revisions),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files/search",
            get(crate::storage::nsfiles::routes::search_namespace_files),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files/export",
            get(crate::storage::nsfiles::routes::export_namespace_files),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/files/directory",
            get(crate::storage::nsfiles::routes::list_namespace_directory)
                .post(crate::storage::nsfiles::routes::create_namespace_directory),
        )
        // W9c Task 6: the KV API (spec §9). The static `/kv/inheritance`
        // is matched before `/kv/{key}`, so a key named `inheritance` is
        // written and deleted on it (its GET is the listing, as in Kestra).
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/kv",
            get(crate::storage::kv::routes::list_kv).delete(
                // The bulk body's own cap, refused with egret's text.
                axum::handler::Handler::layer(
                    crate::storage::kv::routes::delete_kv_values,
                    axum::extract::DefaultBodyLimit::max(
                        crate::storage::kv::routes::MAX_BULK_BODY_BYTES,
                    ),
                ),
            ),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/kv/inheritance",
            get(crate::storage::kv::routes::list_inherited_kv)
                .put(crate::storage::kv::routes::put_kv_inheritance_key)
                .layer(axum::extract::DefaultBodyLimit::disable())
                .delete(crate::storage::kv::routes::delete_kv_inheritance_key),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}",
            get(crate::storage::kv::routes::get_kv_value)
                // The PUT applies its own limit (`max_read_bytes`), so the
                // router's default is off for it, as for the upload.
                .put(crate::storage::kv::routes::put_kv_value)
                .layer(axum::extract::DefaultBodyLimit::disable())
                .delete(crate::storage::kv::routes::delete_kv_value),
        )
        .route(
            "/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}/revisions",
            get(crate::storage::kv::routes::kv_revisions),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/resume",
            post(resume_execution),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/replay",
            post(replay_execution),
        )
        .route(
            "/ws/{workspace}/workflows/executions/{id}/kill",
            post(kill_execution),
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
        restore_flow,
        purge_flow,
        validate_flow,
        validate_unsaved_flow,
        get_flow_schema,
        publish_flow,
        list_flow_versions,
        get_flow_version,
        get_flow_graph,
        graph_unsaved_flow,
        get_execution_graph,
        list_executions,
        get_execution,
        get_execution_logs,
        crate::storage::routes::download_execution_file,
        crate::storage::routes::execution_file_metas,
        crate::storage::routes::preview_execution_file,
        start_flow_execution,
        resume_execution,
        replay_execution,
        kill_execution,
        list_namespaces,
        get_namespace,
        namespace_dependencies,
        crate::triggers::routes::list_flow_triggers,
        crate::triggers::routes::list_triggers,
        crate::triggers::routes::set_trigger_disabled,
        crate::triggers::routes::set_triggers_disabled,
        crate::triggers::backfill_routes::create_trigger_backfill,
        crate::triggers::backfill_routes::set_trigger_backfill_paused,
        crate::triggers::backfill_routes::delete_trigger_backfill,
        crate::triggers::backfill_routes::set_triggers_backfill_paused,
        crate::triggers::backfill_routes::delete_triggers_backfill,
        crate::storage::nsfiles::routes::download_namespace_file,
        crate::storage::nsfiles::routes::namespace_file_stats,
        crate::storage::nsfiles::routes::namespace_file_revisions,
        crate::storage::nsfiles::routes::search_namespace_files,
        crate::storage::nsfiles::routes::export_namespace_files,
        crate::storage::nsfiles::routes::list_namespace_directory,
        crate::storage::nsfiles::routes::upload_namespace_file,
        crate::storage::nsfiles::routes::create_namespace_directory,
        crate::storage::nsfiles::routes::move_namespace_file,
        crate::storage::nsfiles::routes::delete_namespace_file,
        crate::storage::kv::routes::list_kv,
        crate::storage::kv::routes::list_inherited_kv,
        crate::storage::kv::routes::get_kv_value,
        crate::storage::kv::routes::kv_revisions,
        crate::storage::kv::routes::put_kv_value,
        crate::storage::kv::routes::delete_kv_value,
        crate::storage::kv::routes::delete_kv_values,
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
        FlowGraphRequest,
        crate::graph::FlowGraph,
        crate::graph::ExecutionGraph,
        ExecutionListItemDto,
        LabelDto,
        TaskRunDto,
        ExecutionDto,
        ExecutionPageDto,
        StartExecutionRequest,
        StartExecutionMultipart,
        LogLevel,
        LogLineDto,
        LogPageDto,
        WorkflowNamespaceDto,
        WorkflowNamespaceDetailDto,
        NamespaceDependenciesDto,
        NamespaceDependencyNodeDto,
        NamespaceDependencyEdgeDto,
        NamespaceDependencyKind,
        crate::triggers::routes::TriggerKindDto,
        crate::triggers::routes::ToggleRefusal,
        crate::triggers::routes::MissedTicksDto,
        crate::triggers::routes::ScheduleInfoDto,
        crate::triggers::routes::ConditionDto,
        crate::triggers::routes::WindowDto,
        crate::triggers::routes::DependsOnDto,
        crate::triggers::routes::FlowTriggerInfoDto,
        crate::triggers::routes::WebhookInfoDto,
        crate::triggers::routes::LastExecutionDto,
        crate::triggers::routes::BackfillDto,
        crate::triggers::routes::CannotBackfill,
        crate::triggers::routes::TriggerEntryDto,
        crate::triggers::routes::FlowTriggersDto,
        crate::triggers::routes::TriggerRowDto,
        crate::triggers::routes::TriggerPageDto,
        crate::triggers::routes::SetTriggerDisabledRequest,
        crate::triggers::routes::BulkTriggerRef,
        crate::triggers::routes::SetTriggersDisabledRequest,
        crate::triggers::routes::BulkResultDto,
        crate::triggers::routes::BulkResultsDto,
        crate::triggers::backfill_routes::CreateBackfillRequest,
        crate::triggers::backfill_routes::SetBackfillPausedRequest,
        crate::triggers::backfill_routes::SetBackfillsPausedRequest,
        crate::triggers::backfill_routes::DeleteBackfillsRequest,
        crate::storage::routes::FileMetasDto,
        crate::storage::routes::FilePreviewDto,
        crate::storage::nsfiles::routes::NsFileStatsDto,
        crate::storage::nsfiles::routes::NsFileRevisionDto,
        crate::storage::nsfiles::routes::NsFileUploadForm,
        crate::storage::kv::routes::KvEntryDto,
        crate::storage::kv::routes::KvInheritedDto,
        crate::storage::kv::routes::KvDetailDto,
        crate::storage::kv::routes::KvRevisionDto,
        crate::storage::kv::routes::KvKeysBody,
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
