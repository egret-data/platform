//! The backfill routes' writes (W7f spec §3.4, §3.5): create, pause/resume
//! and delete one schedule backfill, each inside a transaction the caller
//! opens and commits (or rolls back on a refusal). No HTTP here; the routes
//! are [`super::backfill_routes`].
//!
//! # The lock order (§3.11)
//!
//! W7e's, extended by one row: the flow row first (`FOR NO KEY UPDATE`, with
//! the workspace predicate), then the trigger's `workflow.triggers` row
//! (`FOR UPDATE`), then the backfill row (`FOR UPDATE`, or the `INSERT`).
//!
//! - **Against a pass:** the pass holds the trigger row from its claim and
//!   takes only `KEY SHARE` on the flow, which `FOR NO KEY UPDATE` does not
//!   block. A write here waits for an in-flight fire to commit, then reads the
//!   rows that fire advanced; after it commits, no tick of the old state fires
//!   (`tests/runtime.rs`,
//!   `a_backfill_delete_that_meets_a_claimed_pass_waits_and_the_pass_fires_first`).
//! - **Against a save:** serialised on the flow row, so `sync` never
//!   re-derives from a backfill being created or deleted.
//! - **Two creates:** serialised on the flow row; the second sees the first's
//!   row and is refused `Exists` (`two_concurrent_backfill_creates_make_one_backfill`).
//!
//! Every row is reached through the workspace-scoped flow: the backfill row
//! is keyed by that flow's id, never by `trigger_backfills.workspace_id`.

use chrono::{DateTime, Months, Utc};
use egret_core::Result;
use sqlx::PgConnection;
use uuid::Uuid;

use super::backfill_routes::{CreateBackfillRequest, MAX_BACKFILL_TICKS};
use super::state::db_err;
use crate::models::{Flow, InputType, Trigger, TriggerKind};
use crate::repo::{flow_cols, FlowRow};
use crate::scheduler::backfill::{backfill_cols, read_for_update, BackfillRow};
use crate::scheduler::{
    backfill_ended, effective_disabled, never_fires, overlay_inputs, refuse_templated,
};
use crate::trigger::{BackfillEnd, ScheduleTrigger};

/// Why a backfill write was refused; nothing was written (the caller rolls
/// back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackfillRefusal {
    /// No such flow in the workspace, the flow is in the trash, or its source
    /// declares no trigger with that id.
    NotFound,
    /// The flow's source does not parse.
    Unparsable(String),
    /// Not a `Schedule`.
    NotSupported,
    /// Create on an effective-disabled trigger (D9).
    TriggerDisabled,
    /// Create on a schedule with no next date: its `last_error`, or why it
    /// does not resolve.
    OffQueue(Option<String>),
    /// `start >= end`.
    WindowInvalid,
    /// `end` is after the transaction's `now()`.
    EndInFuture {
        end: DateTime<Utc>,
        now: DateTime<Utc>,
    },
    /// `start` more than ten years before `now()`.
    StartTooEarly,
    /// The window holds no tick of the schedule.
    NoTicks,
    /// More than [`MAX_BACKFILL_TICKS`] ticks.
    TooManyTicks,
    /// The inputs' sentence, naming the input.
    InputsInvalid(String),
    /// The labels' sentence, naming the label.
    LabelsInvalid(String),
    /// The trigger already has a backfill.
    Exists,
    /// Pause, resume or delete with none.
    NoBackfill,
}

/// A created backfill (not yet committed).
#[derive(Debug)]
pub(crate) struct Created {
    pub flow: FlowRow,
    pub parsed: Flow,
    pub backfill: BackfillRow,
}

/// A pause, resume or delete (not yet committed).
#[derive(Debug)]
pub(crate) struct Changed {
    pub flow: FlowRow,
    pub parsed: Flow,
    /// Something was written. `false`: a pause of a paused backfill or a
    /// resume of a running one; no audit event is due.
    pub changed: bool,
    /// Delete only: the trigger's next date written by the resume (`None`
    /// when no date was written, or the trigger went off the queue).
    pub resumed_at: Option<DateTime<Utc>>,
}

type Refused<T> = Result<std::result::Result<T, BackfillRefusal>>;

/// Steps 1-2, shared: the flow row locked with the workspace predicate, its
/// source parsed, and the first trigger with this id.
async fn flow_and_trigger(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
) -> Refused<(FlowRow, Flow, usize)> {
    let row: Option<FlowRow> = sqlx::query_as(concat!(
        "SELECT ",
        flow_cols!(),
        " FROM workflow.flows WHERE id = $1 AND workspace_id = $2 FOR NO KEY UPDATE"
    ))
    .bind(flow)
    .bind(ws)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let Some(row) = row.filter(|r| r.deleted_at.is_none()) else {
        return Ok(Err(BackfillRefusal::NotFound));
    };
    let parsed = match crate::spec::parse(&row.source) {
        Ok(p) => p,
        Err(e) => return Ok(Err(BackfillRefusal::Unparsable(e.message))),
    };
    let Some(at) = parsed.triggers.iter().position(|t| t.id == trigger_id) else {
        return Ok(Err(BackfillRefusal::NotFound));
    };
    if !matches!(parsed.triggers[at].kind, TriggerKind::Schedule(_)) {
        return Ok(Err(BackfillRefusal::NotSupported));
    }
    Ok(Ok((row, parsed, at)))
}

async fn db_now(conn: &mut PgConnection) -> Result<DateTime<Utc>> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)
}

/// §3.4: create a backfill of `trigger_id` over `req`'s window, in the
/// caller's transaction. Every refusal comes before the first write.
pub(crate) async fn create(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
    req: &CreateBackfillRequest,
    by: Uuid,
) -> Refused<Created> {
    // 1-2.
    let (row, parsed, at) = match flow_and_trigger(conn, ws, flow, trigger_id).await? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let t = &parsed.triggers[at];
    let TriggerKind::Schedule(schedule) = &t.kind else {
        unreachable!("flow_and_trigger refuses every other kind");
    };

    // 3. Effective-disabled, as the entry computes it.
    let switched: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workflow.disabled_triggers \
                         WHERE flow_id = $1 AND trigger_id = $2)",
    )
    .bind(flow)
    .bind(trigger_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    if effective_disabled(parsed.disabled, t.disabled, switched) || row.state == "disabled" {
        return Ok(Err(BackfillRefusal::TriggerDisabled));
    }

    // 4. The trigger row, after the flow.
    let state: Option<(Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT next_evaluation_date, last_error FROM workflow.triggers \
          WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE",
    )
    .bind(flow)
    .bind(trigger_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let previous = match state {
        Some((Some(next), _)) => next,
        Some((None, last_error)) => return Ok(Err(BackfillRefusal::OffQueue(last_error))),
        None => return Ok(Err(BackfillRefusal::OffQueue(None))),
    };
    let st = match ScheduleTrigger::resolve(t) {
        Ok(st) => st,
        Err(e) => return Ok(Err(BackfillRefusal::OffQueue(Some(e.to_string())))),
    };

    // 5. Validate, with the transaction's `now()`.
    let now = db_now(conn).await?;
    let (start, end) = (req.start, req.end.unwrap_or(now));
    if start >= end {
        return Ok(Err(BackfillRefusal::WindowInvalid));
    }
    if end > now {
        return Ok(Err(BackfillRefusal::EndInFuture { end, now }));
    }
    let earliest = now
        .checked_sub_months(Months::new(120))
        .unwrap_or(DateTime::<Utc>::MIN_UTC);
    if start < earliest {
        return Ok(Err(BackfillRefusal::StartTooEarly));
    }
    let (n, more) = st.ticks_through(start, end, MAX_BACKFILL_TICKS);
    if more {
        return Ok(Err(BackfillRefusal::TooManyTicks));
    }
    // `n > 0` proves a tick in `[start, end]`, so this is `Some` and `<= end`.
    let Some(current_tick) = st.first_tick_at_or_after(start).filter(|_| n > 0) else {
        return Ok(Err(BackfillRefusal::NoTicks));
    };
    let inputs = match inputs_refusal(&parsed, schedule, req.inputs.as_ref()) {
        Ok(i) => i,
        Err(why) => return Ok(Err(BackfillRefusal::InputsInvalid(why))),
    };
    if let Some(why) = crate::labels::backfill_labels_refusal(&req.labels) {
        return Ok(Err(BackfillRefusal::LabelsInvalid(why)));
    }
    let labels = serde_json::Value::Array(
        req.labels
            .iter()
            .map(|l| serde_json::json!({"key": l.key, "value": l.value}))
            .collect(),
    );

    // 6. The backfill row, after the trigger row. A row already there (a
    //    create that committed first) inserts nothing.
    let backfill: Option<BackfillRow> = sqlx::query_as(concat!(
        "INSERT INTO workflow.trigger_backfills \
           (flow_id, trigger_id, workspace_id, window_start, window_end, current_tick, \
            inputs, labels, previous_next_evaluation_date, total_ticks, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         ON CONFLICT (flow_id, trigger_id) DO NOTHING RETURNING ",
        backfill_cols!()
    ))
    .bind(flow)
    .bind(trigger_id)
    .bind(ws)
    .bind(start)
    .bind(end)
    .bind(current_tick)
    .bind(serde_json::Value::Object(inputs))
    .bind(labels)
    .bind(previous)
    .bind(n as i32)
    .bind(by)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let Some(backfill) = backfill else {
        return Ok(Err(BackfillRefusal::Exists));
    };

    // 7. Attach: the stream points at the window's first tick, which is past
    //    and so due on the next pass. `evaluated_at` and `last_error` stay.
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = $3, updated_at = now() \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trigger_id)
    .bind(current_tick)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;

    Ok(Ok(Created {
        flow: row,
        parsed,
        backfill,
    }))
}

/// §3.4's input rules; the backfill's own map (what is stored), or the
/// sentence naming the input.
fn inputs_refusal(
    flow: &Flow,
    schedule: &crate::models::Schedule,
    given: Option<&serde_json::Value>,
) -> std::result::Result<serde_json::Map<String, serde_json::Value>, String> {
    let own = match given {
        None | Some(serde_json::Value::Null) => serde_json::Map::new(),
        Some(serde_json::Value::Object(o)) => o.clone(),
        Some(_) => return Err("inputs must be a JSON object".to_string()),
    };
    if let Some(file) = flow
        .inputs
        .iter()
        .find(|i| i.r#type == InputType::File && own.contains_key(&i.id))
    {
        return Err(format!(
            "{} is a FILE input: a backfill has no upload",
            file.id
        ));
    }
    let merged = overlay_inputs(&schedule.inputs.clone().unwrap_or_default(), &own);
    if let Some(why) = refuse_templated(&merged) {
        return Err(why);
    }
    // A JSON `null` is no value, as an absent key.
    if let Some(missing) = flow.inputs.iter().find(|i| {
        i.required
            && i.defaults.is_none()
            && merged.get(&i.id).is_none_or(serde_json::Value::is_null)
    }) {
        return Err(format!("{} is required", missing.id));
    }
    Ok(own)
}

/// Steps 1-2, the trigger row `FOR UPDATE` (absent is fine) and the backfill
/// row `FOR UPDATE`; shared by pause, resume and delete. A disabled trigger
/// is not refused: a frozen backfill can be paused or deleted.
async fn locked_backfill(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
) -> Refused<(
    FlowRow,
    Flow,
    usize,
    Option<Option<DateTime<Utc>>>,
    BackfillRow,
)> {
    let (row, parsed, at) = match flow_and_trigger(conn, ws, flow, trigger_id).await? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let next: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "SELECT next_evaluation_date FROM workflow.triggers \
          WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE",
    )
    .bind(flow)
    .bind(trigger_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let Some(b) = read_for_update(&mut *conn, flow, trigger_id).await? else {
        return Ok(Err(BackfillRefusal::NoBackfill));
    };
    Ok(Ok((row, parsed, at, next, b)))
}

/// §3.5: pause or resume. Writes the backfill row's pause columns and
/// nothing else (D5, D7): the trigger's date stays `current_tick`, so a
/// resume needs no other write.
pub(crate) async fn set_paused(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
    paused: bool,
    by: Uuid,
) -> Refused<Changed> {
    let (row, parsed, _, _, b) = match locked_backfill(conn, ws, flow, trigger_id).await? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let changed = b.paused != paused;
    if changed {
        sqlx::query(
            "UPDATE workflow.trigger_backfills \
                SET paused = $3, \
                    paused_by = CASE WHEN $3 THEN $4::uuid END, \
                    paused_at = CASE WHEN $3 THEN now() END, \
                    updated_at = now() \
              WHERE flow_id = $1 AND trigger_id = $2",
        )
        .bind(flow)
        .bind(trigger_id)
        .bind(paused)
        .bind(by)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    Ok(Ok(Changed {
        flow: row,
        parsed,
        changed,
        resumed_at: None,
    }))
}

/// §3.5: delete, and resume the regular schedule as a fire's refused start
/// does (§3.6, D26: `fire::backfill_ended`, from `max(previous, current_tick)`
/// under the trigger's `recoverMissedSchedules`). A trigger off the queue
/// stays off (the next save re-queues it, §3.7); a disabled trigger's dates
/// are written too, since they are what it resumes from. A schedule that no
/// longer resolves keeps its date: `fire_due` takes it off the queue with the
/// reason when the date comes due.
pub(crate) async fn delete(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
) -> Refused<Changed> {
    let (row, parsed, at, next, b) = match locked_backfill(conn, ws, flow, trigger_id).await? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    sqlx::query("DELETE FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = $2")
        .bind(flow)
        .bind(trigger_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

    let mut resumed_at = None;
    if let Some(Some(_)) = next {
        resumed_at = resume(conn, flow, &parsed.triggers[at], &b).await?;
    }
    Ok(Ok(Changed {
        flow: row,
        parsed,
        changed: true,
        resumed_at,
    }))
}

/// The delete's resume, written to the trigger row the caller holds.
async fn resume(
    conn: &mut PgConnection,
    flow: Uuid,
    t: &Trigger,
    b: &BackfillRow,
) -> Result<Option<DateTime<Utc>>> {
    let TriggerKind::Schedule(schedule) = &t.kind else {
        return Ok(None);
    };
    let Ok(st) = ScheduleTrigger::resolve(t) else {
        return Ok(None);
    };
    let now = db_now(conn).await?;
    let end = BackfillEnd::Deleted {
        current_tick: b.current_tick,
    };
    match backfill_ended(&st, schedule, b, end, now, None) {
        (Some(date), _) => {
            sqlx::query(
                "UPDATE workflow.triggers SET next_evaluation_date = $3, updated_at = now() \
                  WHERE flow_id = $1 AND trigger_id = $2",
            )
            .bind(flow)
            .bind(&t.id)
            .bind(date)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
            Ok(Some(date))
        }
        // No tick left: off the queue with `sync`'s reason, as the switch
        // route stores it.
        (None, why) => {
            sqlx::query(
                "UPDATE workflow.triggers SET next_evaluation_date = NULL, \
                        last_error = $3, updated_at = now() \
                  WHERE flow_id = $1 AND trigger_id = $2",
            )
            .bind(flow)
            .bind(&t.id)
            .bind(why.unwrap_or_else(|| never_fires(schedule.cron.as_deref(), now)))
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
            Ok(None)
        }
    }
}
