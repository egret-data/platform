//! The backfill routes (W7f spec §3.4, §3.5, §3.10): create, pause/resume
//! and delete one schedule backfill, and pause/resume or delete a selection;
//! their wire shapes and the coded errors they add. The writes are
//! [`super::backfill`].
//!
//! ```text
//! POST   /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill
//! PUT    /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill/paused
//! DELETE /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill
//! PUT    /api/ws/{workspace}/workflows/triggers/backfill/paused
//! POST   /api/ws/{workspace}/workflows/triggers/backfill/delete
//! ```
//!
//! Every route checks `workflow:manage` before it reads the body. Each
//! trigger is written in its own transaction, its entry read in that
//! transaction, and an audit event emitted only for a real change.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use egret_core::WorkspacePath;
use egret_workspace::context::WsContext;
use serde::Deserialize;
use uuid::Uuid;

use super::backfill::{self, BackfillRefusal};
use super::routes::{
    body_refusal_saying, entry_in, BulkResultDto, BulkResultsDto, BulkTriggerRef, TriggerEntryDto,
    TriggerRouteError, MAX_BULK_TRIGGERS, TOO_MANY_TRIGGERS, TRIGGER_GONE, TRIGGER_NOT_FOUND,
    TRIGGER_SOURCE_UNPARSABLE,
};
use super::state::db_err;
use crate::routes::{WorkflowState, WF_MANAGE};

/// 400: `start >= end`.
pub const BACKFILL_WINDOW_INVALID: &str = "backfill_window_invalid";
/// 400: `end` is after the server's `now()`.
pub const BACKFILL_END_IN_FUTURE: &str = "backfill_end_in_future";
/// 400: `start` is more than ten years back.
pub const BACKFILL_START_TOO_EARLY: &str = "backfill_start_too_early";
/// 400: the window holds no tick of the schedule.
pub const BACKFILL_NO_TICKS: &str = "backfill_no_ticks";
/// 400: the window holds more than [`MAX_BACKFILL_TICKS`] ticks.
pub const BACKFILL_TOO_MANY_TICKS: &str = "backfill_too_many_ticks";
/// 400: the inputs are not an object, name a `FILE` input, carry template
/// syntax after the merge, or miss a required input.
pub const BACKFILL_INPUTS_INVALID: &str = "backfill_inputs_invalid";
/// 400: a label breaks §3.4's rules.
pub const BACKFILL_LABELS_INVALID: &str = "backfill_labels_invalid";
/// 404: pause, resume or delete with no backfill.
pub const NO_BACKFILL: &str = "no_backfill";
/// 409: the trigger is not a `Schedule`.
pub const BACKFILL_NOT_SUPPORTED: &str = "backfill_not_supported";
/// 409: create on an effective-disabled trigger.
pub const TRIGGER_DISABLED: &str = "trigger_disabled";
/// 409: create on a schedule with no next date.
pub const TRIGGER_OFF_QUEUE: &str = "trigger_off_queue";
/// 409: create while a backfill exists.
pub const BACKFILL_EXISTS: &str = "backfill_exists";
/// The most ticks one backfill's window may hold.
pub const MAX_BACKFILL_TICKS: u32 = 10_000;
/// The body cap of every backfill route that takes a body.
pub const MAX_BACKFILL_BODY_BYTES: usize = 64 * 1024;

/// Create a backfill of a `Schedule` trigger.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateBackfillRequest {
    /// The window's first instant (inclusive): the first tick at or after it
    /// fires first.
    pub start: DateTime<Utc>,
    /// The window's last instant (inclusive); absent: the server's now. Must
    /// not be after the server's now.
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
    /// Overlaid on the schedule's `inputs:` key by key. An object; no key may
    /// name a `FILE` input.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub inputs: Option<serde_json::Value>,
    /// Added to every execution the backfill fires, in this order. A list of
    /// `{key, value}` or a map.
    #[serde(default, deserialize_with = "crate::models::deserialize_labels")]
    #[schema(value_type = Vec<crate::routes::LabelDto>)]
    pub labels: Vec<crate::models::Label>,
}

/// Pause or resume one backfill.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetBackfillPausedRequest {
    pub paused: bool,
}

/// Pause or resume up to 100 backfills, each on its own.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetBackfillsPausedRequest {
    /// At most 100.
    pub triggers: Vec<BulkTriggerRef>,
    pub paused: bool,
}

/// Delete up to 100 backfills, each on its own.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeleteBackfillsRequest {
    /// At most 100.
    pub triggers: Vec<BulkTriggerRef>,
}

/// A refusal's status, code and sentence.
fn refusal_parts(r: &BackfillRefusal) -> (StatusCode, &'static str, String) {
    use BackfillRefusal as R;
    let bad = StatusCode::BAD_REQUEST;
    let conflict = StatusCode::CONFLICT;
    match r {
        R::NotFound => (
            StatusCode::NOT_FOUND,
            TRIGGER_NOT_FOUND,
            TRIGGER_GONE.to_string(),
        ),
        R::Unparsable(m) => (
            bad,
            TRIGGER_SOURCE_UNPARSABLE,
            format!("the flow's saved source does not parse, so its triggers are unknown: {m}"),
        ),
        R::NotSupported => (
            conflict,
            BACKFILL_NOT_SUPPORTED,
            "Only a Schedule trigger can be backfilled".to_string(),
        ),
        R::TriggerDisabled => (
            conflict,
            TRIGGER_DISABLED,
            "Enable the trigger to backfill it".to_string(),
        ),
        R::OffQueue(why) => (
            conflict,
            TRIGGER_OFF_QUEUE,
            format!(
                "The schedule is off the queue: {}; fix the flow and save it",
                why.as_deref().unwrap_or("it has no next date")
            ),
        ),
        R::WindowInvalid => (
            bad,
            BACKFILL_WINDOW_INVALID,
            "The start must be before the end; pick an earlier start".to_string(),
        ),
        R::EndInFuture { end, now } => (
            bad,
            BACKFILL_END_IN_FUTURE,
            format!(
                "The end {} is after the server's time {}; pick an end in the past or leave it empty",
                end.to_rfc3339(),
                now.to_rfc3339()
            ),
        ),
        R::StartTooEarly => (
            bad,
            BACKFILL_START_TOO_EARLY,
            "The start is more than ten years back; check the year".to_string(),
        ),
        R::NoTicks => (
            bad,
            BACKFILL_NO_TICKS,
            "The schedule has no tick in this window; widen it".to_string(),
        ),
        R::TooManyTicks => (
            bad,
            BACKFILL_TOO_MANY_TICKS,
            format!(
                "The window holds more than {MAX_BACKFILL_TICKS} ticks of this schedule; narrow it"
            ),
        ),
        R::InputsInvalid(why) => (bad, BACKFILL_INPUTS_INVALID, why.clone()),
        R::LabelsInvalid(why) => (bad, BACKFILL_LABELS_INVALID, why.clone()),
        R::Exists => (
            conflict,
            BACKFILL_EXISTS,
            "This trigger already has a backfill; pause or delete it first".to_string(),
        ),
        R::NoBackfill => (
            StatusCode::NOT_FOUND,
            NO_BACKFILL,
            "This trigger has no backfill".to_string(),
        ),
    }
}

impl From<BackfillRefusal> for TriggerRouteError {
    fn from(r: BackfillRefusal) -> Self {
        let (status, code, message) = refusal_parts(&r);
        TriggerRouteError::Coded {
            status,
            code,
            message,
        }
    }
}

/// A pause, resume or delete.
#[derive(Debug, Clone, Copy)]
enum Op {
    Pause(bool),
    Delete,
}

/// One create in its own transaction: [`backfill::create`], the entry read
/// in it, the commit, the audit event. The caller holds `workflow:manage`.
async fn create_one(
    st: &WorkflowState,
    ctx: &egret_core::WorkspaceContext,
    ws_slug: &str,
    flow: Uuid,
    trigger_id: &str,
    req: &CreateBackfillRequest,
) -> egret_core::Result<std::result::Result<TriggerEntryDto, BackfillRefusal>> {
    let ws = ctx.workspace_id.0;
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    let c = match backfill::create(&mut tx, ws, flow, trigger_id, req, ctx.principal.user_id.0)
        .await?
    {
        Ok(c) => c,
        Err(refusal) => {
            tx.rollback().await.map_err(db_err)?;
            return Ok(Err(refusal));
        }
    };
    let e = entry_in(&mut tx, ws_slug, &c.flow, &c.parsed, trigger_id).await?;
    tx.commit().await.map_err(db_err)?;
    // Keys, never values: an input or a label may carry what the audit log's
    // readers must not see.
    let mut input_keys: Vec<&String> = c
        .backfill
        .inputs
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    input_keys.sort();
    let label_keys: Vec<&str> = req.labels.iter().map(|l| l.key.as_str()).collect();
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ws),
        "workflow_trigger:backfill_create",
        "workflow_trigger",
        Some(format!("{flow}/{trigger_id}")),
        serde_json::json!({
            "start": c.backfill.window_start,
            "end": c.backfill.window_end,
            "total_ticks": c.backfill.total_ticks,
            "input_keys": input_keys,
            "label_keys": label_keys,
        }),
    ));
    Ok(Ok(e))
}

/// One pause, resume or delete in its own transaction, as [`create_one`].
async fn change_one(
    st: &WorkflowState,
    ctx: &egret_core::WorkspaceContext,
    ws_slug: &str,
    flow: Uuid,
    trigger_id: &str,
    op: Op,
) -> egret_core::Result<std::result::Result<TriggerEntryDto, BackfillRefusal>> {
    let ws = ctx.workspace_id.0;
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    let outcome = match op {
        Op::Pause(paused) => {
            backfill::set_paused(
                &mut tx,
                ws,
                flow,
                trigger_id,
                paused,
                ctx.principal.user_id.0,
            )
            .await?
        }
        Op::Delete => backfill::delete(&mut tx, ws, flow, trigger_id).await?,
    };
    let c = match outcome {
        Ok(c) => c,
        Err(refusal) => {
            tx.rollback().await.map_err(db_err)?;
            return Ok(Err(refusal));
        }
    };
    let e = entry_in(&mut tx, ws_slug, &c.flow, &c.parsed, trigger_id).await?;
    tx.commit().await.map_err(db_err)?;
    if c.changed {
        let (action, detail) = match op {
            Op::Pause(true) => ("workflow_trigger:backfill_pause", serde_json::json!({})),
            Op::Pause(false) => ("workflow_trigger:backfill_resume", serde_json::json!({})),
            Op::Delete => (
                "workflow_trigger:backfill_delete",
                serde_json::json!({ "resumed_at": c.resumed_at }),
            ),
        };
        st.audit.emit(egret_audit::mutation_for(
            &ctx.principal,
            Some(ws),
            action,
            "workflow_trigger",
            Some(format!("{flow}/{trigger_id}")),
            detail,
        ));
    }
    Ok(Ok(e))
}

/// The bulk routes' loop: over 100 is refused before any write; then one
/// transaction per entry, a refusal recorded and the next entry run.
async fn change_many(
    st: &WorkflowState,
    ctx: &egret_core::WorkspaceContext,
    ws_slug: &str,
    triggers: Vec<BulkTriggerRef>,
    op: Op,
) -> Result<Json<BulkResultsDto>, TriggerRouteError> {
    if triggers.len() > MAX_BULK_TRIGGERS {
        return Err(TriggerRouteError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: TOO_MANY_TRIGGERS,
            message: format!(
                "{} triggers: a bulk backfill change takes at most {MAX_BULK_TRIGGERS}; nothing was changed",
                triggers.len()
            ),
        });
    }
    let mut results = Vec::with_capacity(triggers.len());
    for r in triggers {
        let outcome = change_one(st, ctx, ws_slug, r.flow_id, &r.trigger_id, op).await?;
        results.push(match outcome {
            Ok(e) => BulkResultDto {
                flow_id: r.flow_id,
                trigger_id: r.trigger_id,
                ok: true,
                code: None,
                message: None,
                trigger: Some(e),
            },
            Err(refusal) => {
                let (_, code, message) = refusal_parts(&refusal);
                BulkResultDto {
                    flow_id: r.flow_id,
                    trigger_id: r.trigger_id,
                    ok: false,
                    code: Some(code.to_string()),
                    message: Some(message),
                    trigger: None,
                }
            }
        });
    }
    Ok(Json(BulkResultsDto { results }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("trigger_id" = String, Path, description = "The trigger's `id` in the flow's YAML"),
    ),
    request_body = CreateBackfillRequest,
    responses(
        (status = 201, description = "Created. The window `[start, end]` is inclusive at both ends; the trigger's next date moves to the window's first tick, so the scheduler fires the window one tick per pass, then resumes the regular schedule from where it stood under its `recoverMissedSchedules`. The answer is the trigger's entry as `GET .../triggers` returns it.", body = TriggerEntryDto),
        (status = 400, description = "`code: source_does_not_parse`, `invalid_body`, `backfill_window_invalid` (start not before end), `backfill_end_in_future` (the message gives the server's time), `backfill_start_too_early` (more than ten years back), `backfill_no_ticks`, `backfill_too_many_ticks` (over 10000), `backfill_inputs_invalid` (not an object, a FILE input, template syntax after the merge with the schedule's inputs, a required input missing; the message names the input) or `backfill_labels_invalid`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace, the flow is in the trash, or its source declares no trigger with this id (`code: not_found`)", body = egret_core::http::ErrorBody),
        (status = 409, description = "`code: backfill_not_supported` (not a Schedule), `trigger_disabled` (the trigger is effective-disabled), `trigger_off_queue` (the schedule has no next date; the message gives why) or `backfill_exists`", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 64 KiB (`code: body_too_large`)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn create_trigger_backfill(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((ws_slug, id, trigger_id)): Path<(String, Uuid, String)>,
    body: Result<Json<CreateBackfillRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<TriggerEntryDto>), TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) = body.map_err(|r| {
        body_refusal_saying(
            r,
            "{\"start\", \"end\"?, \"inputs\"?: {}, \"labels\"?: [{\"key\", \"value\"}]}",
            "no backfill was created",
        )
    })?;
    let e = create_one(&st, &ctx, &ws_slug, id, &trigger_id, &body).await??;
    Ok((StatusCode::CREATED, Json(e)))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill/paused",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("trigger_id" = String, Path, description = "The trigger's `id` in the flow's YAML"),
    ),
    request_body = SetBackfillPausedRequest,
    responses(
        (status = 200, description = "Paused or resumed (idempotent: already in that state is a 200 with no write and no audit event). Only the backfill row is written: a paused backfill is not fired, and a resumed one fires its next tick on the next pass. A switched-off trigger's backfill can be paused and resumed. The answer is the trigger's entry.", body = TriggerEntryDto),
        (status = 400, description = "`code: source_does_not_parse` or `invalid_body`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "`code: not_found` (no such flow or trigger) or `no_backfill`", body = egret_core::http::ErrorBody),
        (status = 409, description = "`code: backfill_not_supported` (not a Schedule)", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 64 KiB (`code: body_too_large`)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn set_trigger_backfill_paused(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((ws_slug, id, trigger_id)): Path<(String, Uuid, String)>,
    body: Result<Json<SetBackfillPausedRequest>, JsonRejection>,
) -> Result<Json<TriggerEntryDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) =
        body.map_err(|r| body_refusal_saying(r, "{\"paused\": bool}", "nothing was changed"))?;
    let e = change_one(&st, &ctx, &ws_slug, id, &trigger_id, Op::Pause(body.paused)).await??;
    Ok(Json(e))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("trigger_id" = String, Path, description = "The trigger's `id` in the flow's YAML"),
    ),
    responses(
        (status = 200, description = "Deleted. The ticks not yet fired are dropped; the executions already fired stay. The regular schedule resumes from the later of where it stood at create and the backfill's next tick, under its `recoverMissedSchedules` (absent: ALL); a schedule off the queue stays off until the flow is saved. A fire already claimed commits first, and no window tick fires after the answer. The answer is the trigger's entry.", body = TriggerEntryDto),
        (status = 400, description = "`code: source_does_not_parse`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "`code: not_found` (no such flow or trigger) or `no_backfill`", body = egret_core::http::ErrorBody),
        (status = 409, description = "`code: backfill_not_supported` (not a Schedule)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn delete_trigger_backfill(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((ws_slug, id, trigger_id)): Path<(String, Uuid, String)>,
) -> Result<Json<TriggerEntryDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let e = change_one(&st, &ctx, &ws_slug, id, &trigger_id, Op::Delete).await??;
    Ok(Json(e))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/triggers/backfill/paused",
    tag = "workflow",
    params(WorkspacePath),
    request_body = SetBackfillsPausedRequest,
    responses(
        (status = 200, description = "A result per entry, in the order given. Each entry is paused or resumed in its own transaction, as the per-trigger route does, and a refusal (`ok: false` with its `code`: `no_backfill`, `not_found` for a flow outside this workspace, never a 403) does not stop the next.", body = BulkResultsDto),
        (status = 400, description = "More than 100 entries (`code: too_many_triggers`; nothing is changed), or `invalid_body`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 64 KiB (`code: body_too_large`); nothing is changed", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error; entries before the failing one may be changed", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn set_triggers_backfill_paused(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path(ws_slug): Path<String>,
    body: Result<Json<SetBackfillsPausedRequest>, JsonRejection>,
) -> Result<Json<BulkResultsDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) = body.map_err(|r| {
        body_refusal_saying(
            r,
            "{\"triggers\": [{\"flow_id\", \"trigger_id\"}], \"paused\": bool}",
            "nothing was changed",
        )
    })?;
    change_many(&st, &ctx, &ws_slug, body.triggers, Op::Pause(body.paused)).await
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/triggers/backfill/delete",
    tag = "workflow",
    params(WorkspacePath),
    request_body = DeleteBackfillsRequest,
    responses(
        (status = 200, description = "A result per entry, in the order given. Each backfill is deleted in its own transaction, as the per-trigger route does, and a refusal (`ok: false` with its `code`: `no_backfill`, `not_found` for a flow outside this workspace, never a 403) does not stop the next. A `POST`, because a `DELETE` with a body is not portable.", body = BulkResultsDto),
        (status = 400, description = "More than 100 entries (`code: too_many_triggers`; nothing is deleted), or `invalid_body`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 64 KiB (`code: body_too_large`); nothing is deleted", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error; entries before the failing one may be deleted", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn delete_triggers_backfill(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path(ws_slug): Path<String>,
    body: Result<Json<DeleteBackfillsRequest>, JsonRejection>,
) -> Result<Json<BulkResultsDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) = body.map_err(|r| {
        body_refusal_saying(
            r,
            "{\"triggers\": [{\"flow_id\", \"trigger_id\"}]}",
            "nothing was deleted",
        )
    })?;
    change_many(&st, &ctx, &ws_slug, body.triggers, Op::Delete).await
}
