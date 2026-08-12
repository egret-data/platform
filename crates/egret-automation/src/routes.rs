//! HTTP surface for Alerts & Reports, mounted under
//! `/api/ws/{workspace}/alerts` — the path the `alerts` feature entitlement
//! claimed in P0 (`egret-workspace`'s catalog), so mounting it anywhere else
//! would silently escape the module gate.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::Error;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::deliver::email::{self, EmailMessage};
use crate::deliver::smtp_config::{self, SmtpConfigBody, SmtpConfigDto, SmtpConfigRepo};
use crate::error::{AutomationApiError, AutomationError};
use crate::model::{
    detail_dto, run_dto, summary_dto, update_body, CreateSchedule, RunDto, ScheduleDetailDto,
    ScheduleSummaryDto, ScheduleType, UpdateSchedule, WriteBody,
};
use crate::repo::ScheduleRepo;

/// Managing schedules is an editor act, following `chart:manage`. Reading is
/// implied by workspace membership plus the `alerts` module entitlement, which
/// `WsContext` has already enforced by the time a handler runs.
const MANAGE: &str = "alert:manage";

/// How many runs the drawer asks for. Bounded server-side so a client cannot
/// turn the run log into an unpaged table scan.
const RUNS_LIMIT: i64 = 100;

/// SMTP settings are workspace administration, not alert authoring: they carry
/// a credential and set the identity every alert in the workspace sends as.
/// `alert:manage` (an editor grant) is deliberately NOT enough.
const SMTP_MANAGE: &str = "workspace:manage_members";

#[derive(Clone)]
pub struct AutomationState {
    pub db: egret_metastore::Db,
    pub schedules: ScheduleRepo,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub smtp: SmtpConfigRepo,
    /// Per-render credentials (P4): mints/redeems the single-use tokens the
    /// print page's renderer authenticates with. See `render::token`.
    pub render_tokens: crate::render::token::RenderTokenRepo,
    /// The rest of P4's render-payload dependencies. Reused rather than
    /// reimplemented from `egret_bi::BiState`: `charts`/`dashboards`/`datasets`
    /// so a report loads the SAME rows the interactive UI does, `databases` +
    /// `users` + `engine_minter` so `evaluator::owner_identity` can mint the
    /// token's OWNER an engine identity (never a service one).
    pub charts: egret_bi::ChartRepo,
    pub dashboards: egret_bi::DashboardRepo,
    pub datasets: egret_bi::DatasetRepo,
    pub databases: egret_query::DatabaseRepo,
    pub users: egret_auth::users::UserRepo,
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
    /// The same execution seam `AutomationRunner` (the tick engine) uses —
    /// `crate::evaluator::QueryRunner` — so a rendered chart card runs through
    /// code a test can fake an engine behind, exactly like `tick_end_to_end.rs`
    /// already does for alert evaluation. Production wiring is `EngineRunner`
    /// (the real `run_query` path); tests inject a fake.
    pub query: std::sync::Arc<dyn crate::evaluator::QueryRunner>,
}

impl FromRef<AutomationState> for SessionService {
    fn from_ref(s: &AutomationState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<AutomationState> for WorkspaceRepo {
    fn from_ref(s: &AutomationState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<AutomationState> for Authorizer {
    fn from_ref(s: &AutomationState) -> Self {
        s.authz.clone()
    }
}

pub fn automation_router() -> Router<AutomationState> {
    Router::new()
        .route("/ws/{workspace}/alerts", get(list).post(create))
        .route(
            "/ws/{workspace}/alerts/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/ws/{workspace}/alerts/{id}/runs", get(runs))
        .route(
            "/ws/{workspace}/notifications/smtp",
            get(smtp_get).put(smtp_put).delete(smtp_delete),
        )
        .route("/ws/{workspace}/notifications/smtp/test", post(smtp_test))
        // NOT workspace-scoped, and deliberately not under `/ws/{workspace}`:
        // the caller (the headless renderer) has no session to put a workspace
        // in front of. It is authenticated by the render token alone — see
        // `render_payload` below.
        .route("/render/payload", get(render_payload))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list,
        create,
        detail,
        update,
        remove,
        runs,
        smtp_get,
        smtp_put,
        smtp_delete,
        smtp_test,
        render_payload
    ),
    components(schemas(
        egret_automation_api::ScheduleSummaryDto,
        egret_automation_api::ScheduleDetailDto,
        egret_automation_api::CreateSchedule,
        egret_automation_api::UpdateSchedule,
        egret_automation_api::RecipientDto,
        egret_automation_api::RecipientInput,
        egret_automation_api::RecipientKind,
        egret_automation_api::OwnerDto,
        egret_automation_api::RunDto,
        egret_automation_api::RunState,
        egret_automation_api::ScheduleType,
        egret_automation_api::ValidatorType,
        egret_automation_api::ReportFormat,
        crate::deliver::smtp_config::SmtpConfigDto,
        crate::deliver::smtp_config::SmtpConfigBody,
        crate::deliver::smtp_config::TlsMode,
        SmtpTestBody,
        crate::render::payload::RenderPayload,
        crate::render::payload::RenderCard,
    ))
)]
pub struct AutomationApi;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ListQuery {
    /// Narrow to one tab. Omitted returns both.
    #[serde(rename = "type")]
    ty: Option<ScheduleType>,
}

#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(dead_code)]
struct WorkspacePath {
    /// Workspace slug.
    workspace: String,
}

#[utoipa::path(
    get, path = "/api/ws/{workspace}/alerts", tag = "alerts",
    params(WorkspacePath, ListQuery),
    responses((status = 200, description = "Schedules in this workspace", body = Vec<ScheduleSummaryDto>)),
)]
async fn list(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ScheduleSummaryDto>>, AutomationApiError> {
    let rows = st
        .schedules
        .list(ctx.workspace_id.0, q.ty.map(|t| t.as_str()))
        .await
        .map_err(AutomationApiError::from)?;
    // One batched owner query for the whole page, not one per row.
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let owners = st
        .schedules
        .owners_for(&ids)
        .await
        .map_err(AutomationApiError::from)?;
    // ONE decision for the whole list: `alert:manage` is workspace-scoped, so
    // it is the same answer on every row. This is the same check the mutating
    // handlers make, so the toolbar and the routes cannot disagree.
    let can_manage = egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .is_ok();
    Ok(Json(
        rows.iter()
            .map(|r| summary_dto(r, &owners, can_manage))
            .collect(),
    ))
}

#[utoipa::path(
    post, path = "/api/ws/{workspace}/alerts", tag = "alerts", params(WorkspacePath),
    request_body = CreateSchedule,
    responses(
        (status = 200, description = "Created", body = ScheduleDetailDto),
        (status = 400, description = "Invalid body"),
        (status = 409, description = "Name already used by a schedule of this type"),
    ),
)]
async fn create(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Json(body): Json<CreateSchedule>,
) -> Result<Json<ScheduleDetailDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;

    let new = crate::model::to_new(&WriteBody::from(&body), Some(ctx.principal.user_id.0))
        .map_err(AutomationApiError::from)?;
    let made = st
        .schedules
        .create(ctx.workspace_id.0, &new, Some(ctx.principal.user_id))
        .await
        .map_err(AutomationApiError::from)?;
    load_detail(&st, ctx.workspace_id.0, made.id).await
}

#[utoipa::path(
    get, path = "/api/ws/{workspace}/alerts/{id}", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Schedule", body = ScheduleDetailDto),
        (status = 404, description = "No such schedule in this workspace"),
    ),
)]
async fn detail(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<ScheduleDetailDto>, AutomationApiError> {
    load_detail(&st, ctx.workspace_id.0, id).await
}

#[utoipa::path(
    put, path = "/api/ws/{workspace}/alerts/{id}", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    request_body = UpdateSchedule,
    responses(
        (status = 200, description = "Updated", body = ScheduleDetailDto),
        (status = 400, description = "Invalid body"),
        (status = 404, description = "No such schedule in this workspace"),
    ),
)]
async fn update(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(body): Json<UpdateSchedule>,
) -> Result<Json<ScheduleDetailDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;

    // Read first, for two reasons: an id from another workspace must 404 rather
    // than 400 on a body it was never going to write, and `type` is immutable —
    // the stored one is what the body is validated against.
    let stored = st
        .schedules
        .get(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;
    let ty = ScheduleType::parse(&stored.r#type).ok_or_else(|| {
        // Unreachable while `schedule_type_valid` holds; if it ever fires, the
        // row is corrupt rather than the request being wrong.
        AutomationApiError::from(Error::Internal(anyhow::anyhow!(
            "stored schedule type is outside the CHECK vocabulary: {}",
            stored.r#type
        )))
    })?;

    // An update that names NO owners keeps the ones the schedule already has,
    // rather than falling back to the creator the way a create does.
    //
    // The two paths differ because the absence means different things: on
    // create there is nobody yet, so the creator is the only sane owner; on
    // update there already are owners, and a body that does not mention them is
    // not asking for a change. Treating it as one would let any edit through a
    // UI that does not manage owners — which is every UI in P1 — silently drop
    // a co-owner, and with them the identity P2 runs the query as.
    let mut new = crate::model::to_new(
        &update_body(&body, ty),
        stored.created_by.or(Some(ctx.principal.user_id.0)),
    )
    .map_err(AutomationApiError::from)?;
    if body.owners.is_empty() {
        let existing = st
            .schedules
            .owners_for(&[id])
            .await
            .map_err(AutomationApiError::from)?;
        if let Some(owners) = existing.get(&id) {
            if !owners.is_empty() {
                new.owners = owners.iter().map(|o| o.user_id).collect();
            }
        }
    }
    st.schedules
        .update(ctx.workspace_id.0, id, &new, Some(ctx.principal.user_id))
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;
    load_detail(&st, ctx.workspace_id.0, id).await
}

#[utoipa::path(
    delete, path = "/api/ws/{workspace}/alerts/{id}", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Deleted"),
        (status = 404, description = "No such schedule in this workspace"),
    ),
)]
async fn remove(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    if !st
        .schedules
        .delete(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
    {
        return Err(AutomationApiError::from(AutomationError::NotFound(
            "schedule".into(),
        )));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get, path = "/api/ws/{workspace}/alerts/{id}/runs", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 200, description = "Recent runs, newest first", body = Vec<RunDto>)),
)]
async fn runs(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<RunDto>>, AutomationApiError> {
    // The schedule must be visible in this workspace before its runs are, or
    // the child would leak the existence of a parent that 404s.
    if st
        .schedules
        .get(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
        .is_none()
    {
        return Err(AutomationApiError::from(AutomationError::NotFound(
            "schedule".into(),
        )));
    }
    let rows = st
        .schedules
        .runs(ctx.workspace_id.0, id, RUNS_LIMIT)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(rows.iter().map(run_dto).collect()))
}

/// Read a schedule with its owners and recipients, or 404.
async fn load_detail(
    st: &AutomationState,
    ws: Uuid,
    id: Uuid,
) -> Result<Json<ScheduleDetailDto>, AutomationApiError> {
    let row = st
        .schedules
        .get(ws, id)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;
    let owners = st
        .schedules
        .owners_for(&[id])
        .await
        .map_err(AutomationApiError::from)?;
    let recipients = st
        .schedules
        .recipients_for(id)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(detail_dto(
        &row,
        owners.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        &recipients,
    )))
}

// ── Workspace SMTP (P3) ─────────────────────────────────────────────────────

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct SmtpTestBody {
    /// Where to send the probe. Defaults to the configured from-address.
    #[serde(default)]
    pub to: Option<String>,
}

#[utoipa::path(
    get, path = "/api/ws/{workspace}/notifications/smtp", tag = "alerts",
    params(WorkspacePath),
    responses(
        (status = 200, description = "The workspace's SMTP settings, without the password",
         body = SmtpConfigDto),
        (status = 404, description = "Not configured"),
    ),
)]
async fn smtp_get(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
) -> Result<Json<SmtpConfigDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    let cfg = st
        .smtp
        .get(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("smtp config".into())))?;
    // `dto` has no password field at all — see its docs.
    Ok(Json(smtp_config::dto(&cfg)))
}

#[utoipa::path(
    put, path = "/api/ws/{workspace}/notifications/smtp", tag = "alerts",
    params(WorkspacePath), request_body = SmtpConfigBody,
    responses((status = 200, description = "Saved", body = SmtpConfigDto)),
)]
async fn smtp_put(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Json(body): Json<SmtpConfigBody>,
) -> Result<Json<SmtpConfigDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    if body.host.trim().is_empty() {
        return Err(AutomationApiError::from(AutomationError::BadRequest(
            "host is required".into(),
        )));
    }
    if !body.from_address.contains('@') {
        return Err(AutomationApiError::from(AutomationError::BadRequest(
            "from_address must be an email address".into(),
        )));
    }
    let saved = st
        .smtp
        .upsert(ctx.workspace_id.0, &body, Some(ctx.principal.user_id.0))
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(smtp_config::dto(&saved)))
}

#[utoipa::path(
    delete, path = "/api/ws/{workspace}/notifications/smtp", tag = "alerts",
    params(WorkspacePath),
    responses((status = 200, description = "Removed")),
)]
async fn smtp_delete(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
) -> Result<Json<serde_json::Value>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    st.smtp
        .delete(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Send a probe message.
///
/// This endpoint is the whole reason the settings page is trustworthy:
/// "configured" and "configured correctly" are otherwise indistinguishable
/// until an alert fires at 3am and silently does not arrive. The relay's own
/// error text is returned verbatim, because "authentication failed" and
/// "connection refused" send the operator to different places.
#[utoipa::path(
    post, path = "/api/ws/{workspace}/notifications/smtp/test", tag = "alerts",
    params(WorkspacePath), request_body = SmtpTestBody,
    responses(
        (status = 200, description = "The probe was accepted by the relay"),
        (status = 400, description = "The relay refused; the message says why"),
    ),
)]
async fn smtp_test(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Json(body): Json<SmtpTestBody>,
) -> Result<Json<serde_json::Value>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    let cfg = st
        .smtp
        .get(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("smtp config".into())))?;
    let password = st
        .smtp
        .resolve_password(&cfg)
        .await
        .map_err(AutomationApiError::from)?;

    let to = vec![body.to.unwrap_or_else(|| cfg.from_address.clone())];
    email::send(
        &cfg,
        password,
        &EmailMessage {
            to: &to,
            subject: "egret test message",
            body: "This is a test message from egret. If you are reading it, this \
                   workspace's SMTP settings work.\n"
                .into(),
            attachment: None,
        },
    )
    .await
    .map_err(|e| AutomationApiError::from(AutomationError::BadRequest(e.to_string())))?;
    Ok(Json(serde_json::json!({ "ok": true, "sent_to": to })))
}

// ── Report rendering (P4) ────────────────────────────────────────────────────

/// The print page's ONE request. Deliberately token-only: there is no
/// `WsContext` extractor here, so an ordinary session cookie authenticates
/// nothing on this route — widening it to sessions would make it a second,
/// unaudited read path for any chart or dashboard.
#[utoipa::path(
    get, path = "/api/render/payload", tag = "alerts",
    responses(
        (status = 200, description = "Everything the print page needs: the object's spec and every card's already-executed rows", body = crate::render::payload::RenderPayload),
        (status = 401, description = "No render token"),
        (status = 404, description = "Token unknown, expired, or already used"),
    ),
)]
async fn render_payload(
    State(st): State<AutomationState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<crate::render::payload::RenderPayload>, AutomationApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AutomationApiError::from(egret_core::Error::Unauthorized))?;
    let grant = st
        .render_tokens
        .redeem(token)
        .await
        .map_err(AutomationApiError::from)?;
    let payload = crate::render::payload::build(&st, &grant)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(payload))
}
