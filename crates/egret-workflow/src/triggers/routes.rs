//! The trigger routes: the two reads (W7e spec §3.5), a flow's triggers and
//! the workspace's, and the two switches (§3.3, §3.7), one trigger or a
//! selection; their wire shapes and the coded errors they add.
//!
//! ```text
//! GET /api/ws/{workspace}/workflows/flows/{id}/triggers
//! GET /api/ws/{workspace}/workflows/triggers
//! PUT /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled
//! PUT /api/ws/{workspace}/workflows/triggers/disabled
//! ```

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use egret_core::{Error, WorkspacePath};
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::entry::{self, Detail, FlowCtx};
use super::state::{db_err, last_executions, read_state, Key};
use super::switch::{self, SwitchRefusal};
use crate::namespaces::dependencies::{off_runtime, DraftRow, MAX_FLOWS};
use crate::repo::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::routes::{caller_may_manage, WorkflowState, WF_MANAGE, WF_READ};

/// What a trigger is, as egret runs it: one of the three kinds it fires, or
/// `other` (a `Leaf`: any other type, which egret never fires).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKindDto {
    Schedule,
    Webhook,
    Flow,
    Other,
}

/// Why the UI's switch is disabled, the first that applies: the flow is in
/// the trash, the trigger is not one egret fires, the caller lacks
/// `workflow:manage`, the YAML disables it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToggleRefusal {
    FlowDeleted,
    NotSupported,
    Forbidden,
    YamlDisabled,
}

/// Ticks due and not fired, counted up to 1,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct MissedTicksDto {
    /// How many, at most 1000.
    pub count: u32,
    /// `true`: there are more than `count` (the UI says "1000+").
    pub more: bool,
}

/// A `Schedule` trigger: its YAML and its `workflow.triggers` row.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct ScheduleInfoDto {
    pub cron: Option<String>,
    pub with_seconds: bool,
    /// The YAML's `timezone`, `"UTC"` when absent.
    pub timezone: String,
    /// The next tick the scheduler will fire. `null` with `off_queue`.
    pub next_evaluation_date: Option<DateTime<Utc>>,
    /// The last tick the scheduler processed.
    pub evaluated_at: Option<DateTime<Utc>>,
    /// Why the schedule went off the queue, when it did.
    pub last_error: Option<String>,
    /// `true`: no next date; the schedule does not fire until a save
    /// re-initialises it (`last_error` says why).
    pub off_queue: bool,
    /// On the queue and overdue (`next_evaluation_date <= now`, which a
    /// switched-off schedule becomes, since it keeps its date): the ticks from
    /// `next_evaluation_date` through now. `null` otherwise.
    pub missed_ticks: Option<MissedTicksDto>,
    /// The YAML's `recoverMissedSchedules` as written (`ALL`, `NONE`, `LAST`);
    /// `null` when absent, which a re-enable treats as `ALL`.
    pub recover_missed_schedules: Option<String>,
}

/// One `dependsOn` entry, labelled by its condition key.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct ConditionDto {
    pub namespace: Option<String>,
    pub flow_id: Option<String>,
    pub states: Option<Vec<String>>,
    #[schema(value_type = Option<Object>)]
    pub labels: Option<serde_json::Value>,
    pub when: Option<String>,
    /// The open window holds this condition as met.
    pub met: bool,
}

/// The open `dependsOn` window.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct WindowDto {
    /// `duration`, `sliding`, `daily_window` or `daily_deadline`.
    pub kind: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// A `Flow` trigger's `dependsOn`.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct DependsOnDto {
    /// `ALL`, `ANY` or `AT_LEAST`.
    pub mode: String,
    pub min_satisfied: Option<i32>,
    /// Each entry, in declaration order. `null` in the workspace list (the
    /// drawer reads the per-flow route).
    pub conditions: Option<Vec<ConditionDto>>,
    /// The open window; `null` when none is open (an ended one is none).
    pub window: Option<WindowDto>,
}

/// A `Flow` trigger.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct FlowTriggerInfoDto {
    /// The source states it hears, after defaults.
    pub states: Vec<String>,
    /// A `workflow.flow_listeners` row exists.
    pub listening: bool,
    /// Why a trigger without `dependsOn` has no listener row: it does not
    /// resolve.
    pub not_listening_reason: Option<String>,
    pub depends_on: Option<DependsOnDto>,
}

/// A `Webhook` trigger.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct WebhookInfoDto {
    /// `/workflows/v1/{workspace}/executions/webhook/{namespace}/{flow}/{key}`,
    /// each segment percent-encoded, with the key rendered. A path, not a URL:
    /// the server does not know the origin a caller reaches it on. `null` when
    /// the key does not render.
    pub url_path: Option<String>,
    /// Why there is no `url_path`.
    pub key_error: Option<String>,
    /// A declared option egret does not implement; a request is refused.
    pub unsupported: Option<String>,
}

/// The newest execution this trigger started.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct LastExecutionDto {
    pub id: Uuid,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

/// One declared trigger and its state.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct TriggerEntryDto {
    pub trigger_id: String,
    /// The YAML's `type`, in full.
    #[serde(rename = "type")]
    pub r#type: String,
    pub kind: TriggerKindDto,
    /// The flow's or the trigger's YAML says `disabled: true`.
    pub yaml_disabled: bool,
    /// Switched off at runtime (`workflow.disabled_triggers`).
    pub runtime_disabled: bool,
    pub disabled_by: Option<Uuid>,
    /// The switcher's display name.
    pub disabled_by_name: Option<String>,
    pub disabled_at: Option<DateTime<Utc>>,
    /// What the scheduler, the listener and the webhook route honour: the
    /// YAML (flow or trigger), the runtime switch, the flow's `state` being
    /// `disabled`, or the flow being in the trash.
    pub effective_disabled: bool,
    /// Whether the UI's switch is enabled. It is disabled for a trigger the
    /// YAML disables (`toggle_refusal: yaml_disabled`), but the switch route
    /// still accepts switching such a trigger OFF; only switching it ON is
    /// refused there (409 `trigger_disabled_in_source`).
    pub can_toggle: bool,
    /// Why `can_toggle` is false.
    pub toggle_refusal: Option<ToggleRefusal>,
    /// The flow declares this trigger id more than once (a save-time error,
    /// `duplicate_trigger_id`); the state is the id's.
    pub duplicate_id: bool,
    pub schedule: Option<ScheduleInfoDto>,
    pub flow: Option<FlowTriggerInfoDto>,
    /// `null` in the workspace list.
    pub webhook: Option<WebhookInfoDto>,
    pub last_execution: Option<LastExecutionDto>,
}

/// A flow's triggers, in declaration order.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowTriggersDto {
    /// The flow's YAML says `disabled: true`, or its `state` is `disabled`.
    pub flow_disabled: bool,
    /// The flow is in the trash: every trigger is effective-disabled.
    pub flow_deleted: bool,
    /// This deployment runs the schedule loop.
    pub scheduler_enabled: bool,
    /// This deployment runs the worker (and the Flow-trigger processor).
    pub worker_enabled: bool,
    /// This deployment serves webhooks.
    pub webhooks_enabled: bool,
    pub triggers: Vec<TriggerEntryDto>,
}

/// A row of the workspace list: the flow, then the entry's fields.
///
/// The flow's uuid is `flow_uuid`, not `flow`: the entry is flattened in, and
/// its own `flow` (a `Flow` trigger's info) would otherwise be a second key
/// of the same name in one object.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TriggerRowDto {
    /// The flow's uuid (what the per-flow route and the switch take).
    pub flow_uuid: Uuid,
    pub namespace: String,
    /// The flow's YAML `id`.
    pub flow_id: String,
    /// The flow's YAML says `disabled: true`, or its `state` is `disabled`
    /// (the per-flow route's top-level `flow_disabled`, per row): what makes a
    /// trigger `effective_disabled` when neither `yaml_disabled` (which also
    /// covers the flow's YAML) nor `runtime_disabled` does.
    pub flow_disabled: bool,
    #[serde(flatten)]
    pub entry: TriggerEntryDto,
}

/// One page of the workspace's triggers.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TriggerPageDto {
    pub rows: Vec<TriggerRowDto>,
    /// Every trigger that matches, across pages.
    pub total: i64,
    /// The page size served (clamped).
    pub limit: i64,
    pub offset: i64,
    /// Flows in scope whose source did not parse: their triggers are unknown
    /// and not listed.
    pub unparsable: i64,
    pub scheduler_enabled: bool,
    pub worker_enabled: bool,
    pub webhooks_enabled: bool,
    /// The caller holds `workflow:manage` in this workspace: the switches and
    /// the bulk route are theirs to use. Per caller, not per row, so a page
    /// whose rows are all refused for another reason still says it.
    pub can_manage: bool,
}

/// The `code` of the 422 the workspace list answers over
/// [`MAX_FLOWS`] flows in scope.
pub const TRIGGERS_TOO_LARGE_CODE: &str = "triggers_too_large";

/// The error of the trigger routes: every [`egret_core::Error`], plus a coded
/// answer `egret_core::Error` has no variant for.
#[derive(Debug)]
pub enum TriggerRouteError {
    Core(Error),
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
}

impl From<Error> for TriggerRouteError {
    fn from(e: Error) -> Self {
        TriggerRouteError::Core(e)
    }
}

impl axum::response::IntoResponse for TriggerRouteError {
    fn into_response(self) -> axum::response::Response {
        match self {
            TriggerRouteError::Core(e) => e.into_response(),
            TriggerRouteError::Coded {
                status,
                code,
                message,
            } => (
                status,
                Json(egret_core::http::ErrorBody {
                    error: message,
                    code: Some(code.to_string()),
                }),
            )
                .into_response(),
        }
    }
}

/// A read-only transaction with one snapshot, so the state tables and
/// `now()` are read at one instant.
async fn snapshot(
    st: &WorkflowState,
) -> egret_core::Result<sqlx::Transaction<'static, sqlx::Postgres>> {
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    Ok(tx)
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/flows/{id}/triggers",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
    ),
    responses(
        (status = 200, description = "Every trigger the flow's saved draft declares, in declaration order, with its runtime switch, its schedule or listener state, its webhook path and its last execution. A trashed flow answers too, with `flow_deleted: true`.", body = FlowTriggersDto),
        (status = 400, description = "The flow's saved source does not parse, so its triggers are unknown", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn list_flow_triggers(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((ws_slug, id)): Path<(String, Uuid)>,
) -> Result<Json<FlowTriggersDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let row = st.repo.get(ctx.workspace_id.0, id).await?;
    let parsed = crate::spec::parse(&row.source).map_err(|e| {
        Error::InvalidInput(format!(
            "the flow's saved source does not parse, so its triggers are unknown: {}",
            e.message
        ))
    })?;
    let may_manage = caller_may_manage(&st, &ctx).await?;

    let mut tx = snapshot(&st).await?;
    let state = read_state(&mut tx, &[row.id]).await?;
    let flow = FlowCtx {
        id: row.id,
        ws_slug: &ws_slug,
        namespace: &row.namespace,
        flow_id: &row.flow_id,
        state: &row.state,
        deleted: row.deleted_at.is_some(),
    };
    let mut triggers = entry::entries_for_flow(&flow, &parsed, &state, may_manage, Detail::Full);
    let keys: Vec<Key> = triggers
        .iter()
        .map(|e| (row.id, e.trigger_id.clone()))
        .collect();
    let last = last_executions(&mut tx, &keys).await?;
    tx.commit().await.map_err(db_err)?;
    for e in &mut triggers {
        e.last_execution = last.get(&(row.id, e.trigger_id.clone())).cloned();
    }

    let flags = st.trigger_flags;
    Ok(Json(FlowTriggersDto {
        flow_disabled: parsed.disabled || row.state == "disabled",
        flow_deleted: flow.deleted,
        scheduler_enabled: flags.scheduler_enabled,
        worker_enabled: flags.worker_enabled,
        webhooks_enabled: flags.webhooks_enabled,
        triggers,
    }))
}

/// The workspace list's query.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListTriggersQuery {
    /// This namespace and every namespace beneath it, dot-anchored: `a.b`
    /// matches `a.b` and `a.b.c`, never `a.bc`. Must be a valid namespace
    /// name (400 otherwise).
    pub namespace_prefix: Option<String>,
    /// Only this flow (its uuid).
    pub flow_id: Option<Uuid>,
    /// One or more kinds, comma-separated: `schedule`, `webhook`, `flow`,
    /// `other`. Anything else is a 400. Empty (`kind=`) is no filter.
    pub kind: Option<String>,
    /// `enabled` or `disabled`, on `effective_disabled`. Anything else is a
    /// 400. Empty (`state=`) is no filter.
    pub state: Option<String>,
    /// A case-insensitive substring of the trigger id, the flow id or the
    /// namespace.
    pub q: Option<String>,
    /// How many rows to return. Default 50, maximum 200; a larger value is
    /// clamped, and the clamped value comes back in the response's `limit`.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// How many rows to skip. Negative is clamped to zero.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    DEFAULT_PAGE_LIMIT
}

/// The filters the list applies in memory, after the parse.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowFilter {
    pub kinds: Option<Vec<TriggerKindDto>>,
    /// `Some(true)`: only effective-disabled; `Some(false)`: only enabled.
    pub disabled: Option<bool>,
    /// Lower-cased.
    pub q: Option<String>,
}

impl RowFilter {
    fn parse(q: &ListTriggersQuery) -> egret_core::Result<Self> {
        let kinds = match &q.kind {
            None => None,
            Some(raw) => Some(
                raw.split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(|p| match p {
                        "schedule" => Ok(TriggerKindDto::Schedule),
                        "webhook" => Ok(TriggerKindDto::Webhook),
                        "flow" => Ok(TriggerKindDto::Flow),
                        "other" => Ok(TriggerKindDto::Other),
                        other => Err(Error::InvalidInput(format!(
                            "unknown trigger kind '{other}': send schedule, webhook, flow or other"
                        ))),
                    })
                    .collect::<egret_core::Result<Vec<_>>>()?,
            ),
        }
        // `kind=` (or only commas) names no kind: no filter, as if absent.
        .filter(|k| !k.is_empty());
        let disabled = match q.state.as_deref().map(str::trim) {
            None | Some("") => None,
            Some("enabled") => Some(false),
            Some("disabled") => Some(true),
            Some(other) => {
                return Err(Error::InvalidInput(format!(
                    "unknown state '{other}': send enabled or disabled"
                )))
            }
        };
        let q =
            q.q.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase);
        Ok(Self { kinds, disabled, q })
    }

    fn keeps(&self, d: &DraftRow, e: &TriggerEntryDto) -> bool {
        if let Some(kinds) = &self.kinds {
            if !kinds.contains(&e.kind) {
                return false;
            }
        }
        if let Some(disabled) = self.disabled {
            if e.effective_disabled != disabled {
                return false;
            }
        }
        if let Some(q) = &self.q {
            let hit = |s: &str| s.to_lowercase().contains(q.as_str());
            if !(hit(&e.trigger_id) || hit(&d.flow_id) || hit(&d.namespace)) {
                return false;
            }
        }
        true
    }
}

/// What [`build_rows`] answers: one page of rows, the size of the match and
/// the drafts that did not parse.
#[derive(Debug)]
pub(crate) struct BuiltRows {
    pub rows: Vec<TriggerRowDto>,
    pub total: i64,
    pub unparsable: i64,
}

/// The CPU half of the list: parse every draft, build the entries (rows
/// detail), filter, count, cut the page, and count the missed ticks of the
/// page's schedules only. Pure; the handler runs it off the async workers.
pub(crate) fn build_rows(
    drafts: &[DraftRow],
    state: &super::state::StateRows,
    may_manage: bool,
    filter: &RowFilter,
    limit: i64,
    offset: i64,
) -> BuiltRows {
    let mut unparsable = 0i64;
    let mut total = 0i64;
    let mut rows = Vec::new();
    let (start, end) = (offset, offset.saturating_add(limit));
    for d in drafts {
        let Ok(parsed) = crate::spec::parse(&d.source) else {
            unparsable += 1;
            continue;
        };
        let flow = FlowCtx {
            id: d.id,
            // Rows carry no webhook path, so the slug is never rendered.
            ws_slug: "",
            namespace: &d.namespace,
            flow_id: &d.flow_id,
            state: &d.state,
            deleted: false,
        };
        let entries = entry::entries_for_flow(&flow, &parsed, state, may_manage, Detail::Row);
        for (t, mut e) in parsed.triggers.iter().zip(entries) {
            if !filter.keeps(d, &e) {
                continue;
            }
            if total >= start && total < end {
                if let Some(s) = e.schedule.as_mut() {
                    s.missed_ticks = entry::missed_ticks(t, s.next_evaluation_date, state.db_now);
                }
                rows.push(TriggerRowDto {
                    flow_uuid: d.id,
                    namespace: d.namespace.clone(),
                    flow_id: d.flow_id.clone(),
                    flow_disabled: parsed.disabled || d.state == "disabled",
                    entry: e,
                });
            }
            total += 1;
        }
    }
    BuiltRows {
        rows,
        total,
        unparsable,
    }
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/triggers",
    tag = "workflow",
    params(WorkspacePath, ListTriggersQuery),
    responses(
        (status = 200, description = "One page of the triggers of the workspace's flows not in the trash, by namespace, flow id and declaration order. Rows carry no `depends_on.conditions` and no `webhook` (read the per-flow route). `unparsable` counts the flows in scope whose source did not parse.", body = TriggerPageDto),
        (status = 400, description = "`namespace_prefix` is not a valid namespace name, or `kind`/`state` holds an unknown value, or the query string does not decode", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 422, description = "More than 2000 flows match the namespace and flow filters (`code: triggers_too_large`); narrow by namespace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn list_triggers(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListTriggersQuery>,
) -> Result<Json<TriggerPageDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let limit = q.limit.clamp(1, MAX_PAGE_LIMIT);
    let offset = q.offset.max(0);
    if let Some(p) = &q.namespace_prefix {
        if !crate::spec::validate::is_valid_namespace(p) {
            return Err(Error::InvalidInput(format!(
                "namespace_prefix '{p}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long"
            ))
            .into());
        }
    }
    let filter = RowFilter::parse(&q)?;
    let drafts = st
        .repo
        .trigger_drafts(
            ctx.workspace_id.0,
            q.namespace_prefix.as_deref(),
            q.flow_id,
            MAX_FLOWS as i64 + 1,
        )
        .await?;
    if drafts.len() > MAX_FLOWS {
        return Err(TriggerRouteError::Coded {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: TRIGGERS_TOO_LARGE_CODE,
            message: format!("more than {MAX_FLOWS} flows match; narrow by namespace"),
        });
    }
    let may_manage = caller_may_manage(&st, &ctx).await?;

    let mut tx = snapshot(&st).await?;
    let ids: Vec<Uuid> = drafts.iter().map(|d| d.id).collect();
    let state = read_state(&mut tx, &ids).await?;
    // Closed before the parse: no snapshot or connection is held through it.
    tx.commit().await.map_err(db_err)?;
    // Parsing up to MAX_FLOWS drafts is seconds of CPU: never on this worker.
    let built =
        off_runtime(move || build_rows(&drafts, &state, may_manage, &filter, limit, offset))
            .await?;
    let keys: Vec<Key> = built
        .rows
        .iter()
        .map(|r| (r.flow_uuid, r.entry.trigger_id.clone()))
        .collect();
    let mut conn = st.db.pool.acquire().await.map_err(db_err)?;
    let last = last_executions(&mut conn, &keys).await?;
    let mut rows = built.rows;
    for r in &mut rows {
        r.entry.last_execution = last
            .get(&(r.flow_uuid, r.entry.trigger_id.clone()))
            .cloned();
    }

    let flags = st.trigger_flags;
    Ok(Json(TriggerPageDto {
        rows,
        total: built.total,
        limit,
        offset,
        unparsable: built.unparsable,
        scheduler_enabled: flags.scheduler_enabled,
        worker_enabled: flags.worker_enabled,
        webhooks_enabled: flags.webhooks_enabled,
        can_manage: may_manage,
    }))
}

// ── The switches (§3.3, §3.7) ────────────────────────────────────────────────

/// 409: switching on a trigger the flow's or the trigger's YAML disables (D4).
pub const TRIGGER_DISABLED_IN_SOURCE: &str = "trigger_disabled_in_source";
/// 409: a `Leaf` trigger, which egret never fires, has no switch.
pub const TRIGGER_NOT_SUPPORTED: &str = "trigger_not_supported";
/// 409: `recover_missed: true` and the trigger's `recoverMissedSchedules` is
/// not `ALL`, `NONE` or `LAST`.
pub const RECOVER_MISSED_UNKNOWN: &str = "recover_missed_unknown";
/// 404: no such flow in the workspace (or in the trash), or no such trigger.
pub const TRIGGER_NOT_FOUND: &str = "not_found";
/// 400: the flow's saved source does not parse.
pub const TRIGGER_SOURCE_UNPARSABLE: &str = "source_does_not_parse";

/// The most entries one bulk request switches.
pub const MAX_BULK_TRIGGERS: usize = 100;
/// The bulk route's body cap (`DefaultBodyLimit`): 100 entries fit many
/// times over.
pub const MAX_BULK_TRIGGERS_BODY_BYTES: usize = 64 * 1024;

/// Switch one trigger on or off.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetTriggerDisabledRequest {
    /// `true`: off; `false`: on.
    pub disabled: bool,
    /// On a re-enable of a switched-off, overdue `Schedule` only (an already-on
    /// trigger keeps its dates): `false` (the default)
    /// skips the missed ticks (the next date becomes the first tick after
    /// now); `true` follows the trigger's `recoverMissedSchedules` (absent or
    /// `ALL`: every missed tick runs; `NONE`: skip; `LAST`: the latest one
    /// runs). Ignored otherwise.
    #[serde(default)]
    pub recover_missed: bool,
}

/// One trigger of a bulk switch.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkTriggerRef {
    /// The flow's uuid.
    pub flow_id: Uuid,
    pub trigger_id: String,
}

/// Switch up to 100 triggers on or off, each on its own.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetTriggersDisabledRequest {
    /// At most 100.
    pub triggers: Vec<BulkTriggerRef>,
    pub disabled: bool,
    /// As [`SetTriggerDisabledRequest::recover_missed`], for every entry.
    #[serde(default)]
    pub recover_missed: bool,
}

/// One entry's outcome.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkResultDto {
    pub flow_id: Uuid,
    pub trigger_id: String,
    /// Switched (and committed).
    pub ok: bool,
    /// Why not: `not_found`, `source_does_not_parse`,
    /// `trigger_disabled_in_source`, `trigger_not_supported`,
    /// `recover_missed_unknown`.
    pub code: Option<String>,
    /// The refusal's sentence.
    pub message: Option<String>,
    /// The trigger's entry after the switch, when `ok`.
    pub trigger: Option<TriggerEntryDto>,
}

/// A result per entry, in the order given.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkResultsDto {
    pub results: Vec<BulkResultDto>,
}

/// 400: the body is not the route's request (bad JSON, a missing or mistyped
/// field, no JSON content type).
pub const TRIGGER_BODY_INVALID: &str = "invalid_body";
/// 413: the body is over the route's cap.
pub const TRIGGER_BODY_TOO_LARGE: &str = "body_too_large";

/// A body the extractor refused, after the permission check (as the KV bulk
/// delete does): over the cap is a 413, anything else a 400, each with its
/// code and `ErrorBody`.
fn body_refusal(r: JsonRejection, shape: &str) -> TriggerRouteError {
    match r.status() {
        StatusCode::PAYLOAD_TOO_LARGE => TriggerRouteError::Coded {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: TRIGGER_BODY_TOO_LARGE,
            message: "the body is over this route's size cap; nothing was switched".to_string(),
        },
        _ => TriggerRouteError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: TRIGGER_BODY_INVALID,
            message: format!("the body is not `{shape}`: {}", r.body_text()),
        },
    }
}

const TRIGGER_GONE: &str = "This trigger no longer exists in the flow; reload";

/// A refusal's status, code and sentence.
fn refusal_parts(r: &SwitchRefusal) -> (StatusCode, &'static str, String) {
    match r {
        SwitchRefusal::NotFound => (
            StatusCode::NOT_FOUND,
            TRIGGER_NOT_FOUND,
            TRIGGER_GONE.to_string(),
        ),
        SwitchRefusal::Unparsable(m) => (
            StatusCode::BAD_REQUEST,
            TRIGGER_SOURCE_UNPARSABLE,
            format!("the flow's saved source does not parse, so its triggers are unknown: {m}"),
        ),
        SwitchRefusal::DisabledInSource { flow: true } => (
            StatusCode::CONFLICT,
            TRIGGER_DISABLED_IN_SOURCE,
            "This flow is disabled in its YAML; remove `disabled: true` there".to_string(),
        ),
        SwitchRefusal::DisabledInSource { flow: false } => (
            StatusCode::CONFLICT,
            TRIGGER_DISABLED_IN_SOURCE,
            "This trigger is disabled in the flow's YAML; remove `disabled: true` there"
                .to_string(),
        ),
        SwitchRefusal::NotSupported => (
            StatusCode::CONFLICT,
            TRIGGER_NOT_SUPPORTED,
            "egret does not run this trigger type, so it has no switch".to_string(),
        ),
        SwitchRefusal::RecoverUnknown(v) => (
            StatusCode::CONFLICT,
            RECOVER_MISSED_UNKNOWN,
            format!("recoverMissedSchedules is {v}; egret knows ALL, NONE and LAST"),
        ),
    }
}

impl From<SwitchRefusal> for TriggerRouteError {
    fn from(r: SwitchRefusal) -> Self {
        let (status, code, message) = refusal_parts(&r);
        TriggerRouteError::Coded {
            status,
            code,
            message,
        }
    }
}

/// One switch in its own transaction: [`switch::set_disabled`], then the
/// trigger's entry read **in that transaction** (so the answer is what was
/// committed), the commit, and the audit event. The caller has passed
/// `workflow:manage`.
async fn switch_one(
    st: &WorkflowState,
    ctx: &egret_core::WorkspaceContext,
    ws_slug: &str,
    flow: Uuid,
    trigger_id: &str,
    disabled: bool,
    recover_missed: bool,
) -> egret_core::Result<std::result::Result<TriggerEntryDto, SwitchRefusal>> {
    let ws = ctx.workspace_id.0;
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    let sw = match switch::set_disabled(
        &mut tx,
        ws,
        flow,
        trigger_id,
        disabled,
        recover_missed,
        ctx.principal.user_id.0,
    )
    .await?
    {
        Ok(sw) => sw,
        Err(refusal) => {
            tx.rollback().await.map_err(db_err)?;
            return Ok(Err(refusal));
        }
    };
    let state = read_state(&mut tx, &[flow]).await?;
    let fctx = FlowCtx {
        id: sw.flow.id,
        ws_slug,
        namespace: &sw.flow.namespace,
        flow_id: &sw.flow.flow_id,
        state: &sw.flow.state,
        deleted: false,
    };
    // The first declaration with this id: the one the switch resolved.
    let Some(at) = sw.parsed.triggers.iter().position(|t| t.id == trigger_id) else {
        return Err(egret_core::Error::Internal(anyhow::anyhow!(
            "the switched trigger {trigger_id} vanished from its parsed flow"
        )));
    };
    let mut e =
        entry::entries_for_flow(&fctx, &sw.parsed, &state, true, Detail::Full).swap_remove(at);
    let key: Key = (flow, trigger_id.to_string());
    e.last_execution = last_executions(&mut tx, std::slice::from_ref(&key))
        .await?
        .remove(&key);
    tx.commit().await.map_err(db_err)?;
    // Only a real change is audited: off when already off, or on when
    // already on, wrote nothing.
    if sw.changed {
        st.audit.emit(egret_audit::mutation_for(
            &ctx.principal,
            Some(ws),
            if disabled {
                "workflow_trigger:disable"
            } else {
                "workflow_trigger:enable"
            },
            "workflow_trigger",
            Some(format!("{flow}/{trigger_id}")),
            serde_json::json!({
                "recover_missed": recover_missed,
                "next_evaluation_date": sw.moved_to,
            }),
        ));
    }
    Ok(Ok(e))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Flow id"),
        ("trigger_id" = String, Path, description = "The trigger's `id` in the flow's YAML"),
    ),
    request_body = SetTriggerDisabledRequest,
    responses(
        (status = 200, description = "Switched (idempotent: off twice, or on with no switch, is a 200). The answer is the trigger's entry as `GET .../triggers` returns it. Off stops the trigger's schedule claim, Flow-trigger listener and webhook; a fire already claimed commits first, and no new schedule fire starts after the answer. A Flow-trigger pass or a webhook request that read the switch before it changed may still fire once more after the answer. A re-enabled `Schedule` (one that was switched off) whose next date has passed skips the missed ticks, or with `recover_missed: true` follows its `recoverMissedSchedules`; enabling a trigger that is already on changes nothing and is not audited.", body = TriggerEntryDto),
        (status = 400, description = "The flow's saved source does not parse (`code: source_does_not_parse`), or the body is not a switch request: bad JSON, a missing or mistyped field, or no JSON content type (`code: invalid_body`)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such flow in this workspace, the flow is in the trash, or its source declares no trigger with this id (`code: not_found`)", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 2 MiB (`code: body_too_large`)", body = egret_core::http::ErrorBody),
        (status = 409, description = "`code: trigger_disabled_in_source` (switching on a trigger whose flow or trigger YAML says `disabled: true`; switching it off is accepted), `trigger_not_supported` (a trigger type egret does not run), or `recover_missed_unknown` (re-enabling a switched-off `Schedule` with `recover_missed: true` and its `recoverMissedSchedules` is not ALL, NONE or LAST)", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn set_trigger_disabled(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((ws_slug, id, trigger_id)): Path<(String, Uuid, String)>,
    body: Result<Json<SetTriggerDisabledRequest>, JsonRejection>,
) -> Result<Json<TriggerEntryDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) =
        body.map_err(|r| body_refusal(r, "{\"disabled\": bool, \"recover_missed\"?: bool}"))?;
    let e = switch_one(
        &st,
        &ctx,
        &ws_slug,
        id,
        &trigger_id,
        body.disabled,
        body.recover_missed,
    )
    .await??;
    Ok(Json(e))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/triggers/disabled",
    tag = "workflow",
    params(WorkspacePath),
    request_body = SetTriggersDisabledRequest,
    responses(
        (status = 200, description = "A result per entry, in the order given. Each entry is switched in its own transaction, as the per-trigger route does, and a refusal (`ok: false` with its `code`: a flow outside this workspace is `not_found`, never a 403) does not stop the next. A Flow-trigger pass or a webhook request that read a switch before it changed may still fire once more after the answer.", body = BulkResultsDto),
        (status = 400, description = "More than 100 entries (nothing is switched), or the body is not a bulk switch request: bad JSON, a missing or mistyped field, or no JSON content type (`code: invalid_body`)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body is over 64 KiB (`code: body_too_large`); nothing is switched", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error; entries before the failing one may be switched", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn set_triggers_disabled(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path(ws_slug): Path<String>,
    body: Result<Json<SetTriggersDisabledRequest>, JsonRejection>,
) -> Result<Json<BulkResultsDto>, TriggerRouteError> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
    let Json(body) = body.map_err(|r| {
        body_refusal(
            r,
            "{\"triggers\": [{\"flow_id\", \"trigger_id\"}], \"disabled\": bool, \"recover_missed\"?: bool}",
        )
    })?;
    if body.triggers.len() > MAX_BULK_TRIGGERS {
        return Err(Error::InvalidInput(format!(
            "{} triggers: a bulk switch takes at most {MAX_BULK_TRIGGERS}; nothing was switched",
            body.triggers.len()
        ))
        .into());
    }
    let mut results = Vec::with_capacity(body.triggers.len());
    for r in body.triggers {
        let outcome = switch_one(
            &st,
            &ctx,
            &ws_slug,
            r.flow_id,
            &r.trigger_id,
            body.disabled,
            body.recover_missed,
        )
        .await?;
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
