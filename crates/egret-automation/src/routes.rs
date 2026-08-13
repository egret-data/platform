//! HTTP surface for Alerts & Reports, mounted under
//! `/api/ws/{workspace}/alerts` — the path the `alerts` feature entitlement
//! claimed in P0 (`egret-workspace`'s catalog), so mounting it anywhere else
//! would silently escape the module gate.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{get, patch, post, put};
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

/// Reading a schedule.
///
/// Added by migration 0113 and, until now, checked NOWHERE on alerts — the
/// generic folder router was its only consumer, so a permission whose whole
/// name says "may view alerts" granted nothing on the alerts themselves.
/// Meanwhile `detail` and `runs` had no gate at all: workspace membership plus
/// the `alerts` entitlement was enough to read any schedule's SQL, its
/// database, its recipient addresses, and the engine errors in its run log.
///
/// Held by all three seeded workspace roles (`ws_viewer`, `ws_editor`,
/// `ws_admin`), so ordinary members keep the access they had. A workspace
/// using a hand-made role predating 0113 will need it granted.
const READ: &str = "alert:read";

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
    /// The tick engine itself (P5's "Run now"). The SAME `Arc` production
    /// wires into `runner::spawn` — never a second instance built from this
    /// state's own fields — so a manual run and a scheduled run share
    /// `evaluate_and_deliver` byte for byte and cannot diverge.
    pub runner: std::sync::Arc<crate::runner::AutomationRunner>,
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
        .route("/ws/{workspace}/alerts/{id}/run", post(run_now))
        .route("/ws/{workspace}/alerts/{id}/active", put(set_active))
        .route("/ws/{workspace}/alerts/{id}/trash", post(trash))
        .route("/ws/{workspace}/alerts/{id}/restore", post(restore))
        .route("/ws/{workspace}/alerts/{id}/folder", patch(set_folder))
        .route(
            "/ws/{workspace}/alerts/{id}/bookmark",
            post(bookmark_add).delete(bookmark_remove),
        )
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
        run_now,
        set_active,
        trash,
        restore,
        set_folder,
        bookmark_add,
        bookmark_remove,
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
        SetActiveBody,
        SetFolderBody,
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
    /// Show the trash bin (soft-deleted rows) instead of the live list.
    #[serde(default)]
    trashed: bool,
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
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(AutomationApiError::from)?;
    let rows = st
        .schedules
        .list(
            ctx.workspace_id.0,
            q.ty.map(|t| t.as_str()),
            ctx.principal.user_id.0,
            q.trashed,
        )
        .await
        .map_err(AutomationApiError::from)?;
    // One batched owner query for the whole page, not one per row.
    let ids: Vec<Uuid> = rows.iter().map(|(s, _)| s.id).collect();
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
            .map(|(s, bookmarked)| summary_dto(s, &owners, can_manage, *bookmarked))
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
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(AutomationApiError::from)?;
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
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(AutomationApiError::from)?;
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

/// Run one schedule immediately (P5's "Run now"). Goes through the exact
/// same `AutomationRunner::run_now` the tick's own claim loop calls
/// `evaluate_and_deliver` through — see that function's doc comment for why
/// a manual run is not a second evaluator and does not bypass the grace
/// period. Returns the run-log row it just wrote, so the caller can show the
/// result without a second request.
#[utoipa::path(
    post, path = "/api/ws/{workspace}/alerts/{id}/run", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Ran once; the run-log row it wrote", body = RunDto),
        (status = 404, description = "No such schedule in this workspace"),
    ),
)]
async fn run_now(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<RunDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;

    // Read (workspace-scoped) first: an id from another workspace must 404
    // rather than run a schedule the caller cannot even see.
    let schedule = st
        .schedules
        .get(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;

    st.runner.run_now(&schedule).await;

    // `run_now` always writes exactly one run-log row (`run_and_log`'s
    // contract), so the newest row for this schedule IS the one it just
    // wrote — no id needs threading back out of the runner for this.
    let mut written = st
        .schedules
        .runs(ctx.workspace_id.0, id, 1)
        .await
        .map_err(AutomationApiError::from)?;
    let row = written.pop().ok_or_else(|| {
        AutomationApiError::from(egret_core::Error::Internal(anyhow::anyhow!(
            "run_now wrote no run-log row for schedule {id}"
        )))
    })?;
    Ok(Json(run_dto(&row)))
}

/// Flip only `active` (P5's list-row toggle). A dedicated endpoint rather
/// than routing a toggle through `PUT .../alerts/{id}`, which is a FULL
/// REPLACE requiring every field — building a toggle on top of it would mean
/// a read-modify-write that could clobber a concurrent edit to the rest of
/// the schedule.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct SetActiveBody {
    pub active: bool,
}

#[utoipa::path(
    put, path = "/api/ws/{workspace}/alerts/{id}/active", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    request_body = SetActiveBody,
    responses(
        (status = 200, description = "Updated", body = ScheduleSummaryDto),
        (status = 404, description = "No such schedule in this workspace"),
    ),
)]
async fn set_active(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(body): Json<SetActiveBody>,
) -> Result<Json<ScheduleSummaryDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;

    let row = st
        .schedules
        .set_active(ctx.workspace_id.0, id, body.active)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;
    let owners = st
        .schedules
        .owners_for(&[id])
        .await
        .map_err(AutomationApiError::from)?;
    // The caller just passed `alert:manage` above, so `can_manage` is `true`
    // for this response — same reasoning `list` uses for every row.
    Ok(Json(summary_dto(&row, &owners, true, false)))
}

/// Move to the trash (soft delete), so the list keeps a Restore path and the
/// scheduler stops claiming it — `deleted_at IS NULL` is part of the claim
/// predicate, not merely a display filter.
#[utoipa::path(
    post, path = "/api/ws/{workspace}/alerts/{id}/trash", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Moved to the trash", body = egret_core::Ack),
        (status = 401, body = egret_core::http::ErrorBody, description = "Not authenticated"),
        (status = 403, body = egret_core::http::ErrorBody, description = "Needs alert:manage"),
        (status = 404, body = egret_core::http::ErrorBody, description = "No such schedule here"),
        (status = 500, body = egret_core::http::ErrorBody, description = "Internal error"),
    ),
)]
async fn trash(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    ack(st.schedules.set_trashed(ctx.workspace_id.0, id, true).await)
}

#[utoipa::path(
    post, path = "/api/ws/{workspace}/alerts/{id}/restore", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Restored from the trash", body = egret_core::Ack),
        (status = 401, body = egret_core::http::ErrorBody, description = "Not authenticated"),
        (status = 403, body = egret_core::http::ErrorBody, description = "Needs alert:manage"),
        (status = 404, body = egret_core::http::ErrorBody, description = "No such schedule here"),
        (status = 500, body = egret_core::http::ErrorBody, description = "Internal error"),
    ),
)]
async fn restore(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    ack(st
        .schedules
        .set_trashed(ctx.workspace_id.0, id, false)
        .await)
}

/// `false` from a workspace-scoped write means the id belongs to another
/// workspace (or nothing), which is a 404 — never a 403, which would confirm
/// the id exists.
fn ack(r: Result<bool, AutomationError>) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    match r.map_err(AutomationApiError::from)? {
        true => Ok(Json(egret_core::Ack { ok: true })),
        false => Err(AutomationError::NotFound("schedule".into()).into()),
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct SetFolderBody {
    /// `null` moves the schedule back to Uncategorized.
    folder_id: Option<Uuid>,
}

#[utoipa::path(
    patch, path = "/api/ws/{workspace}/alerts/{id}/folder", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    request_body = SetFolderBody,
    responses(
        (status = 200, description = "Moved", body = egret_core::Ack),
        (status = 401, body = egret_core::http::ErrorBody, description = "Not authenticated"),
        (status = 403, body = egret_core::http::ErrorBody, description = "Needs alert:manage"),
        (status = 404, body = egret_core::http::ErrorBody, description = "No such schedule here"),
        (status = 500, body = egret_core::http::ErrorBody, description = "Internal error"),
    ),
)]
async fn set_folder(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(body): Json<SetFolderBody>,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    ack(st
        .schedules
        .set_folder(ctx.workspace_id.0, id, body.folder_id)
        .await)
}

/// A bookmark is the CALLER's own, so it needs no `alert:manage` — a viewer
/// may star what they can READ, which is now a permission that exists and is
/// therefore checked rather than described.
#[utoipa::path(
    post, path = "/api/ws/{workspace}/alerts/{id}/bookmark", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Bookmarked for the caller", body = egret_core::Ack),
        (status = 401, body = egret_core::http::ErrorBody, description = "Not authenticated"),
        (status = 500, body = egret_core::http::ErrorBody, description = "Internal error"),
    ),
)]
async fn bookmark_add(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, READ)
        .await
        .map_err(AutomationApiError::from)?;
    st.schedules
        .add_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(egret_core::Ack { ok: true }))
}

#[utoipa::path(
    delete, path = "/api/ws/{workspace}/alerts/{id}/bookmark", tag = "alerts",
    params(WorkspacePath, ("id" = Uuid, Path, description = "Schedule id")),
    responses(
        (status = 200, description = "Bookmark removed for the caller", body = egret_core::Ack),
        (status = 401, body = egret_core::http::ErrorBody, description = "Not authenticated"),
        (status = 500, body = egret_core::http::ErrorBody, description = "Internal error"),
    ),
)]
async fn bookmark_remove(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    // Deliberately NOT gated on READ, unlike `bookmark_add`. This deletes a row
    // about the caller and nobody else; if someone loses `alert:read` while a
    // bookmark exists, refusing to let them clear it strands a star they can
    // neither see nor remove. Adding is gated because bookmarking something you
    // cannot read is a (weak) test for whether it exists.
    st.schedules
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(egret_core::Ack { ok: true }))
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
    let targets = st
        .schedules
        .targets_for(id)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(detail_dto(
        &row,
        owners.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        &recipients,
        &targets,
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
            html: None,
            attachments: Vec::new(),
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
