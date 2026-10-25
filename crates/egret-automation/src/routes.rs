//! HTTP surface for Alerts & Reports, mounted under
//! `/api/ws/{workspace}/alerts` — the path the `alerts` feature entitlement
//! claimed in P0 (`egret-workspace`'s catalog), so mounting it anywhere else
//! would silently escape the module gate.

use axum::extract::{FromRef, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{http::StatusCode, Json, Router};
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
    /// The workspace's Slack app — one bot token for every channel it posts to.
    pub slack_config: crate::deliver::slack_config::SlackConfigRepo,
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
    /// `analytics.folders` is shared across chart/dashboard/dataset/alert
    /// folders (`entity_type`), keyed on nothing but its own id — used here
    /// only by `set_folder`, to check a `folder_id` the body supplies belongs
    /// to THIS workspace (and is an 'alert' folder) before it is stored, the
    /// same guard `egret-bi`'s own `require_folder` applies to chart/
    /// dashboard/dataset folders.
    pub folders: egret_bi::folder::FolderRepo,
    pub databases: egret_query::DatabaseRepo,
    pub users: egret_auth::users::UserRepo,
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
    /// The Data API seam as a PORT (fix-round-2), so this crate never names
    /// `egret-dataapi` — same shape `query`/`engine_minter` already use for
    /// their own seams. `require_dataapi_endpoint_read` uses this to resolve
    /// an endpoint id's ACTUAL workspace before ever asking the PDP about it;
    /// without it, a foreign or fictional id was authorized by any
    /// workspace-cascaded grant, because nothing checked which workspace the
    /// id actually belonged to.
    pub dataapi_endpoints: std::sync::Arc<dyn egret_dataapi_api::DataApiEndpointPort>,
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

/// The renderer's two endpoints, kept OUT of `automation_router`.
///
/// Not workspace-scoped and deliberately not under `/ws/{workspace}`: the
/// caller is a headless browser with no session, so there is no workspace to
/// put in front and no cookie to scope. The render token is the whole
/// credential.
///
/// Mounted WITHOUT the CSRF layer, for the reason the layer exists at all.
/// Double-submit CSRF defends requests the browser authenticates AMBIENTLY —
/// it attaches the cookie whether or not the page meant to. Nothing is
/// ambient here: the token travels in an `Authorization` header this code
/// sets, so a cross-site page cannot make an authenticated request in the
/// first place, and requiring a CSRF cookie the renderer can never have would
/// simply make `POST /render/data` impossible.
///
/// Found by running a real report: the integration tests call this router
/// directly, so the layer they never mount rejected every POST in production
/// with `csrf check failed` and the page reported a failure the renderer could
/// only describe as "the print page reported a failure".
pub fn render_router() -> Router<AutomationState> {
    Router::new()
        .route("/render/payload", get(render_payload))
        .route("/render/data", post(render_data))
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
        .route(
            "/ws/{workspace}/notifications/slack",
            get(slack_get).put(slack_put).delete(slack_delete),
        )
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
        slack_get,
        slack_put,
        slack_delete,
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
        egret_automation_api::DataapiMetric,
        crate::deliver::smtp_config::SmtpConfigDto,
        crate::deliver::smtp_config::SmtpConfigBody,
        crate::deliver::smtp_config::TlsMode,
        SmtpTestBody,
        SetActiveBody,
        SetFolderBody,
        crate::deliver::slack_config::SlackConfigDto,
        crate::deliver::slack_config::SlackConfigBody,
        crate::render::payload::RenderPayload,
        crate::render::payload::RenderCardRequest,
        crate::render::payload::RenderCardData,
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
    let mut rows = st
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

    // `ScheduleSummaryDto::last_state`/`last_eval_at` say whether a
    // `dataapi_alert`'s threshold fired — a weaker signal than `runs`'/
    // `detail`'s raw `value`, but still information ABOUT an endpoint the
    // caller may not be allowed to read (Task 4 review, Finding 1). `alert:read`
    // is not that permission, so those two fields are blanked per row rather
    // than the row (or the whole list) being hidden — the schedule's own
    // existence/name/owners are ordinary alert-management visibility,
    // untouched by this gate.
    //
    // Checked per DISTINCT `dataapi_endpoint_id` (`None` is the
    // workspace-wide bucket) and memoized: an object-scoped grant can differ
    // endpoint to endpoint, but every row naming the SAME endpoint gets the
    // SAME answer, so a page naming k distinct endpoints costs k PDP calls,
    // not one per row.
    let mut endpoint_read: std::collections::HashMap<Option<Uuid>, bool> =
        std::collections::HashMap::new();
    for (s, _) in rows.iter_mut() {
        if s.r#type != "dataapi_alert" {
            continue;
        }
        let key = s.dataapi_endpoint_id;
        let allowed = match endpoint_read.get(&key) {
            Some(a) => *a,
            None => {
                let a = require_dataapi_endpoint_read(&st, &ctx, key).await.is_ok();
                endpoint_read.insert(key, a);
                a
            }
        };
        if !allowed {
            s.last_state = None;
            s.last_eval_at = None;
        }
    }

    Ok(Json(
        rows.iter()
            .map(|(s, bookmarked)| summary_dto(s, &owners, can_manage, *bookmarked))
            .collect(),
    ))
}

/// Refuse a `chart_id`, `dashboard_id`, `schedule_targets` entry, or
/// `database_id` that does not belong to `ws` — the write-side half of the
/// report-target scoping fix. Task 1 made the READ safe
/// (`ChartRepo::get`/`DashboardRepo::get` now require a workspace and return
/// `None` for a foreign object); this stops the row from ever being written in
/// the first place, so the normal experience is a 400 at save rather than a
/// schedule that silently starts failing at run time the next time it ticks.
///
/// `schedule_targets.object_id` has no foreign key — `kind` is polymorphic,
/// so the column cannot point at two tables at once — which is exactly why it
/// needs its own check here rather than relying on the database to reject it.
///
/// `database_id` is the sibling of `chart_id`/`dashboard_id`: `to_new` already
/// restricts it to alert schedules, so a report body naturally has none to
/// check here. Unlike the chart/dashboard columns, `query.databases` carries
/// no cross-tenant guard of its own below this — `open_secret` resolves the
/// credential from whatever `workspace_id` the row itself names — so this is
/// the ONLY place stopping a workspace-1 alert from storing workspace-2's
/// connection id (and, at run time, opening workspace-2's vaulted secret).
///
/// `owners` is deliberately NOT checked here — see [`validate_owners_in_workspace`],
/// which runs against the body's own list rather than `new.owners` (which by
/// this point may already be a server-chosen fallback, not anything the
/// caller named).
///
/// A foreign object and a nonexistent one are deliberately answered the SAME
/// way throughout: `ChartRepo::get`/`DashboardRepo::get`/`get_in_workspace`
/// already collapse "exists in another workspace" and "does not exist at all"
/// into one `None`/`NotFound`, and this function just returns that verdict
/// rather than distinguishing the two cases — a distinguishable refusal would
/// let a caller use this endpoint to probe whether an id exists in a
/// workspace it cannot see, the same existence-oracle concern the rest of
/// this codebase already avoids for cross-tenant ids.
async fn validate_targets_in_workspace(
    st: &AutomationState,
    ws: Uuid,
    new: &crate::repo::NewSchedule,
) -> Result<(), AutomationApiError> {
    let bi_err =
        |e: egret_bi::BiError| AutomationApiError::from(AutomationError::Internal(e.to_string()));
    let bad = |field: &str| {
        AutomationApiError::from(AutomationError::BadRequest(format!(
            "{field}: no such object in this workspace"
        )))
    };

    if let Some(id) = new.chart_id {
        if st.charts.get(id, ws).await.map_err(bi_err)?.is_none() {
            return Err(bad("chart_id"));
        }
    }
    if let Some(id) = new.dashboard_id {
        if st.dashboards.get(id, ws).await.map_err(bi_err)?.is_none() {
            return Err(bad("dashboard_id"));
        }
    }
    for (kind, object_id) in &new.targets {
        let found = match kind.as_str() {
            "chart" => st
                .charts
                .get(*object_id, ws)
                .await
                .map_err(bi_err)?
                .is_some(),
            "dashboard" => st
                .dashboards
                .get(*object_id, ws)
                .await
                .map_err(bi_err)?
                .is_some(),
            // Not one of the two kinds `schedule_target_kind_valid` allows.
            // Fail the same way here rather than letting an unrecognised kind
            // reach the database only to trip that CHECK as a 500.
            _ => false,
        };
        if !found {
            return Err(bad("targets"));
        }
    }
    if let Some(id) = new.database_id {
        match st.databases.get_in_workspace(ws, id).await {
            Ok(_) => {}
            Err(egret_query::DatabaseError::NotFound(_)) => return Err(bad("database_id")),
            Err(e) => {
                return Err(AutomationApiError::from(AutomationError::Internal(
                    e.to_string(),
                )))
            }
        }
    }
    Ok(())
}

/// Refuse an owner the caller NAMED who is not a member of `ws`.
///
/// Takes the body's own `owners: &[Uuid]` — never `NewSchedule::owners` —
/// because by the time a `NewSchedule` exists, that field may already be a
/// server-chosen fallback rather than anything the caller wrote:
///
/// - On `create`, an empty body falls back to the creator (`to_new`'s
///   `creator` parameter). Checking THAT id would mean a `platform_admin`
///   creating a schedule in a workspace they belong to only via the
///   platform-admin `WsContext` bypass (`egret-workspace/src/context.rs:108-113`
///   — no `access_grants` row) gets refused for failing to be a member of a
///   workspace they were just admitted to. A fallback the server chose for
///   itself is not a naming, so it must never be checked.
/// - On `update`, an empty body means "keep the existing owners" — `update`
///   (below) replaces `new.owners` with the STORED owners afterwards. Running
///   this check on `new.owners` at that point would validate the fallback
///   (the creator, again) instead of what actually gets written, AND would
///   spuriously refuse an update that touches nothing about `owners` on a
///   schedule whose creator has since left the workspace or had their grant
///   expire.
///
/// So this runs BEFORE either fallback substitution, directly over the slice
/// the request body carried, and is a no-op when that slice is empty — the
/// caller named nobody, so there is nothing to check. `Authorizer::workspace_roles_for`
/// is the same direct-or-group role lookup `WsContext` itself is built from;
/// an empty result means "not a member", whether the id names a real user
/// elsewhere or nothing at all, so there is no existence oracle here either.
async fn validate_owners_in_workspace(
    st: &AutomationState,
    ws: Uuid,
    owners: &[Uuid],
) -> Result<(), AutomationApiError> {
    for owner in owners {
        let roles = st
            .authz
            .workspace_roles_for(egret_core::UserId(*owner), egret_core::WorkspaceId(ws))
            .await
            .map_err(|e| AutomationApiError::from(AutomationError::Internal(e.to_string())))?;
        if roles.is_empty() {
            return Err(AutomationApiError::from(AutomationError::BadRequest(
                "owners: not a member of this workspace".into(),
            )));
        }
    }
    Ok(())
}

/// Enforce that a `dataapi_alert` names (or, when unscoped, watches) only an
/// endpoint the caller can already read (Task 4, Z2).
///
/// `endpoint_id: None` is a WORKSPACE-scoped `dataapi_alert` — it watches
/// every endpoint in the workspace, so the bar is workspace-level
/// `data_api_endpoint:read` rather than an object-scoped grant on one row.
/// `Some(id)` is object-scoped, mirroring `egret-dataapi`'s own `EP_READ`
/// gate on a single endpoint (`egret-dataapi/src/routes.rs`).
async fn require_dataapi_endpoint_read(
    st: &AutomationState,
    ctx: &egret_core::WorkspaceContext,
    endpoint_id: Option<Uuid>,
) -> Result<(), AutomationApiError> {
    match endpoint_id {
        Some(id) => {
            // Fix-round-2, Critical 2: `dataapi_endpoint_id` is a plain FK —
            // existence only, no workspace predicate (migration 0128) — and
            // `ObjectRef.workspace_id` below is the CALLER's workspace, never
            // the object's: Cedar resolves `resource in workspace` from that
            // stamped ancestry and never loads the row, so a
            // workspace-cascaded grant (e.g. `ws_admin`) authorized ANY uuid,
            // foreign or fictional, before this check existed. Resolved
            // through the `DataApiEndpointPort` seam — mirrors
            // `egret-dataapi::EndpointStore::scoped`'s own "confirm id names
            // a row in workspace, or NotFound" precedent — rather than a raw
            // SQL query against `dataapi.endpoints` from this crate, which
            // this task's own brief says not to invent.
            //
            // A mismatch is refused as `NotFound` (404), not `Forbidden`
            // (403) — the same status `egret-dataapi` itself answers a
            // foreign id with (`EndpointStore::get`/`scoped`, both
            // `Error::NotFound`). A 403 here would be an ORACLE: it would
            // tell this workspace that the uuid names a LIVE endpoint
            // somewhere, just not here. `None` (no such row at all) takes the
            // identical path below, so the two cases are indistinguishable
            // from the response — which is the whole point.
            let owner_ws = st
                .dataapi_endpoints
                .workspace_of(id)
                .await
                .map_err(AutomationApiError::from)?;
            if owner_ws != Some(ctx.workspace_id.0) {
                return Err(AutomationApiError::from(AutomationError::NotFound(
                    "endpoint".into(),
                )));
            }
            egret_core::authz::require_object(
                &*st.pdp,
                ctx,
                egret_dataapi_api::EP_READ,
                egret_core::authz::ObjectRef {
                    kind: egret_dataapi_api::OBJECT_KIND.to_string(),
                    id,
                    // A folder-scoped grant does not cascade to this check,
                    // matching `egret-dataapi::invoke::can_invoke`'s own
                    // precedent (see that function's doc comment): conservative
                    // rather than widening access nobody asked for.
                    folder_id: None,
                    workspace_id: ctx.workspace_id,
                },
            )
            .await
            .map_err(AutomationApiError::from)
        }
        None => egret_core::authz::require(&*st.pdp, ctx, egret_dataapi_api::EP_READ)
            .await
            .map_err(AutomationApiError::from),
    }
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

    // Task 5's carry-forward from Task 4: `CreateSchedule` can now express a
    // `dataapi_alert` (`ScheduleType::DataapiAlert` + `dataapi_endpoint_id`),
    // so `alert:manage` alone is no longer enough — the same reasoning
    // `update` below documents. Checked against the BODY's
    // `dataapi_endpoint_id` (there is no stored row yet to read one from),
    // and before `to_new` validates the rest of the shape: a caller who
    // cannot read the endpoint must be refused before any other field of the
    // body is even considered.
    if body.ty == ScheduleType::DataapiAlert {
        require_dataapi_endpoint_read(&st, &ctx, body.dataapi_endpoint_id).await?;
    }

    let new = crate::model::to_new(&WriteBody::from(&body), Some(ctx.principal.user_id.0))
        .map_err(AutomationApiError::from)?;
    validate_targets_in_workspace(&st, ctx.workspace_id.0, &new).await?;
    // Against the body's OWN `owners`, not `new.owners` — an empty body falls
    // back to the creator inside `to_new`, and that fallback must never be
    // checked (see `validate_owners_in_workspace`'s doc comment).
    validate_owners_in_workspace(&st, ctx.workspace_id.0, &body.owners).await?;
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

    // `ScheduleDetailDto::last_value` IS a `dataapi_alert`'s measured metric
    // (Task 4 review, Finding 1) — the same leak `runs` carries, on the
    // single-row view. A fresh fetch, not `load_detail`'s (which runs after
    // this, on the same id): a caller who fails this gate must receive
    // nothing of the detail DTO, only the 403. A missing row falls through to
    // `load_detail`'s own 404 unchanged.
    if let Some(stored) = st
        .schedules
        .get(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
    {
        if stored.r#type == "dataapi_alert" {
            require_dataapi_endpoint_read(&st, &ctx, stored.dataapi_endpoint_id).await?;
        }
    }
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

    // `alert:manage` alone is not enough to touch a `dataapi_alert`: the
    // schedule names (or, unscoped, watches) a Data API endpoint, and its
    // error rate/traffic are information ABOUT that endpoint — without this,
    // `alert:manage` would be a side channel for observing an endpoint the
    // caller cannot otherwise see. Checked against the raw STORED `type`, not
    // the `ScheduleType::parse` below: a `dataapi_alert` row does not parse
    // there yet (`ScheduleType` gains that variant in Task 5), and this
    // authorization gate must not depend on that landing first.
    //
    // TWO checks, not one — this is the fix-round-1 correction. `type` is
    // immutable, so once `stored.r#type` is `dataapi_alert` the body ALWAYS
    // carries a `dataapi_endpoint_id` too (Task 5 gave `UpdateSchedule` that
    // field), and checking only the STORED endpoint left the target endpoint
    // this update would SET unauthorized entirely: a caller who could read
    // endpoint A could repoint an A-scoped alert at endpoint B (proven live,
    // fix-round-1 review) — or, by simply omitting the field, widen it to
    // `dataapi_endpoint_id = NULL`, this codebase's workspace-wide "watch
    // every endpoint" (proven live too, no malice required, just an omitted
    // field). Both are now covered by checking `body.dataapi_endpoint_id`
    // through the SAME `require_dataapi_endpoint_read` used for `create()`:
    // `None` demands workspace-level `data_api_endpoint:read`, exactly the
    // bar an intentional "watch the whole workspace" update should clear.
    // This is a full REPLACE (`PUT`), like every other field `to_new`
    // validates below — an omitted `dataapi_endpoint_id` is not "leave it
    // alone" (that reading exists ONLY for `owners`, and is documented,
    // below, as a deliberate, narrow exception for a different reason). It
    // is "the caller is asking for `NULL`", and is authorized as such rather
    // than specially preserved — a client that always resubmits the current
    // value (the Task 6 editor's own plan: an endpoint picker pre-populated
    // from `ScheduleDetailDto`) is unaffected either way, and a client that
    // does not is refused rather than granted a silent widening.
    if stored.r#type == "dataapi_alert" {
        require_dataapi_endpoint_read(&st, &ctx, stored.dataapi_endpoint_id).await?;
        require_dataapi_endpoint_read(&st, &ctx, body.dataapi_endpoint_id).await?;
    }

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
    validate_targets_in_workspace(&st, ctx.workspace_id.0, &new).await?;
    // Against the body's OWN `owners`, BEFORE the "empty means keep the
    // existing ones" substitution below — an empty body here means `new.owners`
    // is currently `stored.created_by` (another fallback, not a naming), and
    // is about to be overwritten with the STORED owners regardless. Checking
    // it now would validate a value that never gets written, AND would 400 an
    // update that touches nothing about `owners` on a schedule whose creator
    // has since left the workspace (see `validate_owners_in_workspace`'s doc
    // comment). The stored owners it falls back to were already validated
    // when they were written.
    validate_owners_in_workspace(&st, ctx.workspace_id.0, &body.owners).await?;
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
    let schedule = st
        .schedules
        .get(ctx.workspace_id.0, id)
        .await
        .map_err(AutomationApiError::from)?
        .ok_or_else(|| AutomationApiError::from(AutomationError::NotFound("schedule".into())))?;

    // `RunDto::value` IS a `dataapi_alert`'s measured metric (Task 4 review,
    // Finding 1) — `alert:read` says nothing about the ENDPOINT the value
    // describes, only about the schedule.
    if schedule.r#type == "dataapi_alert" {
        require_dataapi_endpoint_read(&st, &ctx, schedule.dataapi_endpoint_id).await?;
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

    // Worse than a read (Task 4 review, Finding 1): this does not just show
    // the LAST measurement, it takes a FRESH one —
    // `evaluate_and_deliver_dataapi_alert` re-measures `dataapi.access_log`
    // synchronously — and hands the result straight back in the response.
    // `alert:manage` lets a caller author the schedule; it says nothing about
    // the endpoint whose traffic this button would let them re-measure on
    // demand. Checked BEFORE `runner.run_now`, not merely before the
    // response is built, so a caller who fails this gate cannot even trigger
    // the measurement.
    if schedule.r#type == "dataapi_alert" {
        require_dataapi_endpoint_read(&st, &ctx, schedule.dataapi_endpoint_id).await?;
    }

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

    let mut row = st
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

    // Same leak `list` redacts (Task 4 review, Finding 4): `ScheduleSummaryDto`
    // built from the `RETURNING` row carries `last_state`/`last_eval_at`, and
    // `alert:manage` says nothing about the ENDPOINT those fields describe for
    // a `dataapi_alert`. A caller who can flip `active` but cannot read the
    // endpoint must still not learn whether it last fired.
    if row.r#type == "dataapi_alert"
        && require_dataapi_endpoint_read(&st, &ctx, row.dataapi_endpoint_id)
            .await
            .is_err()
    {
        row.last_state = None;
        row.last_eval_at = None;
    }

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
    // The schedule itself is scoped by `set_folder`'s own `WHERE id=$1 AND
    // workspace_id=$2`, but `folder_id` was going straight into the `SET`
    // clause unchecked: a foreign folder stored with a 200, and a nonexistent
    // one tripped `folders_id_fkey` as a 500 — two different status CLASSES,
    // which made this an existence oracle over every `analytics.folders` row
    // in the installation (charts, dashboards and datasets included), one
    // call per guess, open to any `ws_editor`. Both wrongs are refused HERE,
    // identically, before the write: `st.folders.meta` collapses "belongs to
    // another workspace", "belongs to a different entity_type", and "does not
    // exist" into the same `_ =>` arm, so a foreign and a nonexistent id are
    // no longer distinguishable at all — matching `validate_targets_in_workspace`'s
    // rule for chart/dashboard/database ids above.
    if let Some(folder_id) = body.folder_id {
        let meta = st
            .folders
            .meta(folder_id)
            .await
            .map_err(|e| AutomationApiError::from(AutomationError::Internal(e.to_string())))?;
        match meta {
            Some((w, t)) if w == ctx.workspace_id.0 && t == "alert" => {}
            _ => {
                return Err(AutomationApiError::from(AutomationError::BadRequest(
                    "folder_id: no such object in this workspace".into(),
                )))
            }
        }
    }
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
        (status = 204, description = "Not configured"),
    ),
)]
async fn smtp_get(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
) -> Result<Response, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    let cfg = st
        .smtp
        .get(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?;
    // Not configured is every workspace's starting state, not a missing
    // resource: 204, not 404, which the browser logs as an error on every visit
    // of the settings page (KI-212). `dto` has no password field at all — see
    // its docs.
    Ok(match cfg {
        Some(cfg) => Json(smtp_config::dto(&cfg)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
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

/// The workspace's Slack app.
///
/// Gated on `SMTP_MANAGE` — the same permission as the relay, and for the same
/// reason: a bot token is workspace administration, not alert authoring. An
/// editor who may write a schedule still may not swap the credential every
/// schedule posts with.
#[utoipa::path(
    get, path = "/api/ws/{workspace}/notifications/slack", tag = "alerts",
    params(WorkspacePath),
    responses(
        (status = 200, description = "The workspace's Slack app", body = crate::deliver::slack_config::SlackConfigDto),
        (status = 204, description = "Not configured"),
    ),
)]
async fn slack_get(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
) -> Result<Response, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    let cfg = st
        .slack_config
        .get(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?;
    // 204 when not configured, as SMTP's (KI-212). The DTO has no token field
    // at all — see its docs.
    Ok(match cfg {
        Some(cfg) => Json(crate::deliver::slack_config::to_dto(&cfg)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[utoipa::path(
    put, path = "/api/ws/{workspace}/notifications/slack", tag = "alerts",
    params(WorkspacePath), request_body = crate::deliver::slack_config::SlackConfigBody,
    responses((status = 200, description = "Saved", body = crate::deliver::slack_config::SlackConfigDto)),
)]
async fn slack_put(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
    Json(body): Json<crate::deliver::slack_config::SlackConfigBody>,
) -> Result<Json<crate::deliver::slack_config::SlackConfigDto>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    let cfg = st
        .slack_config
        .upsert(ctx.workspace_id.0, &body, Some(ctx.principal.user_id.0))
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(crate::deliver::slack_config::to_dto(&cfg)))
}

#[utoipa::path(
    delete, path = "/api/ws/{workspace}/notifications/slack", tag = "alerts",
    params(WorkspacePath),
    responses((status = 200, description = "Removed", body = egret_core::Ack)),
)]
async fn slack_delete(
    State(st): State<AutomationState>,
    WsContext(ctx): WsContext,
) -> Result<Json<egret_core::Ack>, AutomationApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SMTP_MANAGE)
        .await
        .map_err(AutomationApiError::from)?;
    st.slack_config
        .delete(ctx.workspace_id.0)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(egret_core::Ack { ok: true }))
}

// ── Report rendering (P4) ────────────────────────────────────────────────────

/// The print page's ONE request.
///
/// Deliberately token-only: there is no `WsContext` extractor here, so an
/// ordinary session cookie authenticates nothing on this route — widening it
/// to sessions would make it a second, unaudited read path for any chart or
/// dashboard.
#[utoipa::path(
    get, path = "/api/render/payload", tag = "alerts",
    responses(
        (status = 200, description = "What is being rendered: the object's title, each card's saved spec and param mappings, and the dashboard's parameters. No rows — see POST /render/data", body = crate::render::payload::RenderPayload),
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
    // `peek`, not `redeem`: this step hands out titles and saved specs so the
    // page can resolve the dashboard's parameters. The ROWS are what costs the
    // token — see `render_data` below.
    let grant = st
        .render_tokens
        .peek(token)
        .await
        .map_err(AutomationApiError::from)?;
    let payload = crate::render::payload::build(&st, &grant)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(payload))
}

/// Run the render's cards with the filters the page resolved.
///
/// Step two, and the one that SPENDS the token: `redeem` deletes the row in the
/// same statement that reads it, so the rows are handed over exactly once.
///
/// The body carries FILTERS, never a query. They are merged onto each card's
/// saved spec by `payload::run_cards`, exactly as `db_card_run` does for the
/// interactive dashboard, and every column is checked against the dataset by
/// `compile_chart_sql`. The chart ids are checked against the object the token
/// names, so a token for one dashboard cannot run another's chart.
#[utoipa::path(
    post, path = "/render/data", tag = "alerts",
    request_body = Vec<crate::render::payload::RenderCardRequest>,
    responses(
        (status = 200, description = "The rows for each requested card", body = Vec<crate::render::payload::RenderCardData>),
        (status = 400, body = egret_core::http::ErrorBody, description = "A card outside this render, or an invalid filter"),
        (status = 401, body = egret_core::http::ErrorBody, description = "Missing or spent render token"),
    ),
)]
async fn render_data(
    State(st): State<AutomationState>,
    headers: axum::http::HeaderMap,
    Json(reqs): Json<Vec<crate::render::payload::RenderCardRequest>>,
) -> Result<Json<Vec<crate::render::payload::RenderCardData>>, AutomationApiError> {
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
    let cards = crate::render::payload::run_cards(&st, &grant, &reqs)
        .await
        .map_err(AutomationApiError::from)?;
    Ok(Json(cards))
}
