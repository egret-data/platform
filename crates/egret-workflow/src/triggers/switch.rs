//! The runtime switch's write (W7e spec §3.2 "Concurrency", §3.3): one
//! trigger switched on or off, inside a transaction the caller opens and
//! commits.
//!
//! # The lock order
//!
//! A save's order: the flow row first (`FOR NO KEY UPDATE`, which a save's
//! `UPDATE workflow.flows` also takes), then the trigger's state row
//! (`workflow.triggers` or `workflow.flow_listeners`, `FOR UPDATE`).
//!
//! - **Against a save or `reconcile`:** both serialise on the flow row, so a
//!   save's `sync` never computes the effective copy from a switch that is
//!   about to change. Without the flow lock a save could read the switch rows
//!   before this transaction commits, wait on the trigger row held here, and
//!   then write the stale value over this one: a lost update
//!   (`tests/runtime.rs`, `a_save_and_a_switch_on_one_flow_end_with_the_switch`).
//! - **Against a pass:** the pass holds the trigger row and takes only `KEY
//!   SHARE` on the flow (its execution insert), which `FOR NO KEY UPDATE`
//!   does not block, so there is no cycle; this waits for an in-flight fire to
//!   commit, and no new fire of the schedule starts after it.
//! - **Against a pass's `resync`:** it reads the flow `FOR SHARE SKIP LOCKED`
//!   and does nothing while the flow is held here (`scheduler/fire.rs`).

use chrono::{DateTime, Utc};
use egret_core::Result;
use sqlx::PgConnection;
use uuid::Uuid;

use super::state::db_err;
use crate::models::{Flow, TriggerKind};
use crate::repo::{flow_cols, FlowRow};
use crate::scheduler::{effective_disabled, never_fires};
use crate::trigger::{FixedClock, RecoverMissed, ScheduleTrigger, TriggerState};

/// A switch that was applied (not yet committed).
#[derive(Debug)]
pub(crate) struct Switched {
    /// The flow row, read under the lock.
    pub flow: FlowRow,
    /// Its source, parsed.
    pub parsed: Flow,
    /// The schedule's new `next_evaluation_date`, when a re-enable moved it.
    pub moved_to: Option<DateTime<Utc>>,
    /// A re-enable during a backfill moved the regular schedule's resume position here (§3.8).
    pub resume_moved_to: Option<DateTime<Utc>>,
    /// The switch row was inserted (off) or deleted (on). `false`: the
    /// trigger already was in the asked state, nothing was written, and no
    /// audit event is due.
    pub changed: bool,
}

/// Why a switch was refused; nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitchRefusal {
    /// No such flow in the workspace, the flow is in the trash, or its source
    /// declares no trigger with that id.
    NotFound,
    /// The flow's source does not parse: the trigger's kind is unknown.
    Unparsable(String),
    /// Switching on a trigger the YAML disables; `flow` when it is the flow's
    /// own `disabled: true`.
    DisabledInSource { flow: bool },
    /// A `Leaf` trigger: egret never fires it, so it has no switch.
    NotSupported,
    /// `recover_missed: true` and the trigger's `recoverMissedSchedules` is
    /// not `ALL`, `NONE` or `LAST` (the raw value, as JSON).
    RecoverUnknown(String),
}

/// A schedule row's `(next_evaluation_date, evaluated_at)`; a listener row
/// reads as `(None, None)`.
type Dates = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// Step 7's outcome for a re-enabled overdue schedule.
enum Target {
    /// Keep the stored dates (`ALL`, or a `LAST` with nothing newer).
    Keep,
    /// Move `next_evaluation_date` here; `None`: the cron has no tick after
    /// now, so the row goes off the queue.
    Date(Option<DateTime<Utc>>),
}

/// What [`set_disabled`] locks on the trigger's side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StateTable {
    Schedule,
    Listener,
    None,
}

/// Switch `trigger_id` of `flow` (in workspace `ws`) on or off, in the
/// caller's transaction. The caller commits on `Ok(Ok(_))` and MUST roll
/// back on a refusal: `RecoverUnknown` is found after the switch row's
/// `DELETE` (it applies only when there was a row to delete), every other
/// refusal before any write.
pub(crate) async fn set_disabled(
    conn: &mut PgConnection,
    ws: Uuid,
    flow: Uuid,
    trigger_id: &str,
    disabled: bool,
    recover_missed: bool,
    by: Uuid,
) -> Result<std::result::Result<Switched, SwitchRefusal>> {
    // 1. The flow row, locked first (see the module doc). The workspace
    //    predicate makes a foreign id a 404, never a 403.
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
        return Ok(Err(SwitchRefusal::NotFound));
    };

    // 2. The trigger, the first declared with this id.
    let parsed = match crate::spec::parse(&row.source) {
        Ok(p) => p,
        Err(e) => return Ok(Err(SwitchRefusal::Unparsable(e.message))),
    };
    let Some(t) = parsed.triggers.iter().find(|t| t.id == trigger_id) else {
        return Ok(Err(SwitchRefusal::NotFound));
    };
    let table = match &t.kind {
        TriggerKind::Schedule(_) => StateTable::Schedule,
        TriggerKind::Flow(_) => StateTable::Listener,
        TriggerKind::Webhook(_) => StateTable::None,
        TriggerKind::Leaf { .. } => return Ok(Err(SwitchRefusal::NotSupported)),
    };

    // 3. Off is always accepted (D4); on is refused while the YAML disables.
    if !disabled && (parsed.disabled || t.disabled) {
        return Ok(Err(SwitchRefusal::DisabledInSource {
            flow: parsed.disabled,
        }));
    }
    // 4. The state row, after the flow.
    let state_row: Option<Dates> = match table {
        StateTable::Schedule => sqlx::query_as(
            "SELECT next_evaluation_date, evaluated_at FROM workflow.triggers \
              WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE",
        )
        .bind(flow)
        .bind(trigger_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?,
        StateTable::Listener => sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM workflow.flow_listeners \
              WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE",
        )
        .bind(flow)
        .bind(trigger_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?
        .map(|_| (None, None)),
        StateTable::None => None,
    };

    // 5. The switch row. `changed`: it was there to delete, or not there to
    //    insert -- the trigger really changes state.
    let changed = if disabled {
        sqlx::query(
            "INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id, disabled_by) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (flow_id, trigger_id) DO NOTHING",
        )
        .bind(flow)
        .bind(trigger_id)
        .bind(ws)
        .bind(by)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?
        .rows_affected()
            == 1
    } else {
        sqlx::query("DELETE FROM workflow.disabled_triggers WHERE flow_id = $1 AND trigger_id = $2")
            .bind(flow)
            .bind(trigger_id)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?
            .rows_affected()
            == 1
    };
    // Re-enable outcomes (step 7) apply only to a trigger that WAS switched
    // off (default decision, 2026-10-15): an already-on overdue schedule is
    // catching up after downtime or replaying an `ALL` backlog one tick per
    // pass, and an enable -- a bulk Enable over a mixed selection included --
    // must not discard that backlog.
    let re_enabled = !disabled && changed;
    // Only where recovery can apply. The DELETE above is this transaction's
    // only write so far, and a refusal is rolled back by the caller, so
    // nothing is written.
    let recover = match &t.kind {
        TriggerKind::Schedule(s) if re_enabled && recover_missed => {
            match RecoverMissed::from_model(s.recover_missed_schedules.as_ref()) {
                Ok(r) => Some(r),
                Err(raw) => return Ok(Err(SwitchRefusal::RecoverUnknown(raw))),
            }
        }
        _ => None,
    };

    // 6. The effective copy, on the row locked above (none: nothing to write).
    let copy = effective_disabled(parsed.disabled, t.disabled, disabled);
    if state_row.is_some() {
        let q = match table {
            StateTable::Schedule => {
                "UPDATE workflow.triggers SET disabled = $3, updated_at = now() \
                  WHERE flow_id = $1 AND trigger_id = $2"
            }
            StateTable::Listener => {
                "UPDATE workflow.flow_listeners SET disabled = $3 \
                  WHERE flow_id = $1 AND trigger_id = $2"
            }
            StateTable::None => unreachable!("no state row is read for a webhook"),
        };
        sqlx::query(q)
            .bind(flow)
            .bind(trigger_id)
            .bind(copy)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }

    // 7. A re-enabled schedule (it had a switch row) on the queue whose next
    //    date has passed: skip or recover the ticks missed while it was off.
    //    `evaluated_at` is never written, nor is `last_error` except as `sync`
    //    writes it for a cron with no tick left; a row off the queue stays off
    //    (the next save re-queues it).
    //
    //    During a backfill (W7f §3.8, D11) the stored date is the backfill's
    //    `current_tick` and is kept for every choice, so the backfill resumes
    //    at the tick it stopped on; the choice moves the regular schedule's
    //    resume position (`previous_next_evaluation_date`) instead.
    let mut moved_to = None;
    let mut resume_moved_to = None;
    if let (true, StateTable::Schedule, Some((next, evaluated_at))) = (re_enabled, table, state_row)
    {
        // The backfill row, after the trigger row (§3.11's lock order).
        let backfill =
            crate::scheduler::backfill::read_for_update(&mut *conn, flow, trigger_id).await?;
        let db_now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
        if let Some(b) = backfill {
            resume_moved_to = skip_backfill_resume(
                &mut *conn,
                flow,
                t,
                recover,
                b.previous_next_evaluation_date,
                db_now,
            )
            .await?;
        } else if next.is_some_and(|n| n <= db_now) {
            // A schedule that no longer resolves: its dates are left alone. Its
            // row is already off the queue, or `fire_due` takes it off (with the
            // reason in `last_error`) when this date comes due.
            if let Ok(st) = ScheduleTrigger::resolve(t) {
                let skip = || {
                    Target::Date(
                        TriggerState::initialise(&st, "", "", &FixedClock(db_now))
                            .next_evaluation_date,
                    )
                };
                let target = match recover {
                    None | Some(RecoverMissed::None) => skip(),
                    Some(RecoverMissed::All) => Target::Keep,
                    Some(RecoverMissed::Last) => match evaluated_at {
                        None => skip(),
                        Some(ev) => match st.last_tick_at_or_before(db_now) {
                            Some(l) if l > ev => Target::Date(Some(l)),
                            _ => Target::Keep,
                        },
                    },
                };
                match target {
                    Target::Keep => {}
                    Target::Date(Some(date)) => {
                        sqlx::query(
                            "UPDATE workflow.triggers SET next_evaluation_date = $3, updated_at = now() \
                              WHERE flow_id = $1 AND trigger_id = $2",
                        )
                        .bind(flow)
                        .bind(trigger_id)
                        .bind(date)
                        .execute(&mut *conn)
                        .await
                        .map_err(db_err)?;
                        moved_to = Some(date);
                    }
                    // A skip with no tick after now (`0 0 30 2 *`): off the
                    // queue with `sync`'s reason, as `sync` stores it.
                    Target::Date(None) => {
                        let cron = match &t.kind {
                            TriggerKind::Schedule(s) => s.cron.as_deref(),
                            _ => None,
                        };
                        sqlx::query(
                            "UPDATE workflow.triggers SET next_evaluation_date = NULL, \
                                    last_error = $3, updated_at = now() \
                              WHERE flow_id = $1 AND trigger_id = $2",
                        )
                        .bind(flow)
                        .bind(trigger_id)
                        .bind(never_fires(cron, db_now))
                        .execute(&mut *conn)
                        .await
                        .map_err(db_err)?;
                    }
                }
            }
        }
    }

    Ok(Ok(Switched {
        flow: row,
        parsed,
        moved_to,
        resume_moved_to,
        changed,
    }))
}

/// Step 7 during a backfill (W7f §3.8): a skip (`recover_missed: false`, or
/// `NONE`) moves the regular schedule's resume position to the first tick
/// after `db_now`, when that position has passed (as W7e's skip moves only an
/// overdue date); `ALL` and `LAST` leave it, and §3.6 applies the policy when
/// the backfill ends. A cron with no tick after now leaves it too: the
/// backfill's end takes the trigger off the queue then (D26). The answer is
/// the date written, if any. The caller holds the backfill row.
async fn skip_backfill_resume(
    conn: &mut PgConnection,
    flow: Uuid,
    t: &crate::models::Trigger,
    recover: Option<RecoverMissed>,
    resume: DateTime<Utc>,
    db_now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    if !matches!(recover, None | Some(RecoverMissed::None)) || resume > db_now {
        return Ok(None);
    }
    let Ok(st) = ScheduleTrigger::resolve(t) else {
        return Ok(None);
    };
    let Some(date) =
        TriggerState::initialise(&st, "", "", &FixedClock(db_now)).next_evaluation_date
    else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $3, updated_at = now() \
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
