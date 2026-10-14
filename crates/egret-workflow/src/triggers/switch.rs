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
use crate::scheduler::effective_disabled;
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

/// What [`set_disabled`] locks on the trigger's side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StateTable {
    Schedule,
    Listener,
    None,
}

/// Switch `trigger_id` of `flow` (in workspace `ws`) on or off, in the
/// caller's transaction. The caller commits on `Ok(Ok(_))`; on a refusal
/// nothing was written and the caller rolls back.
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
    // Checked before any write, for every re-enable of a schedule that asks
    // to recover, overdue or not: the answer does not depend on the clock.
    let recover = match &t.kind {
        TriggerKind::Schedule(s) if !disabled && recover_missed => {
            match RecoverMissed::from_model(s.recover_missed_schedules.as_ref()) {
                Ok(r) => Some(r),
                Err(raw) => return Ok(Err(SwitchRefusal::RecoverUnknown(raw))),
            }
        }
        _ => None,
    };

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

    // 5. The switch row.
    if disabled {
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
        .map_err(db_err)?;
    } else {
        sqlx::query(
            "DELETE FROM workflow.disabled_triggers WHERE flow_id = $1 AND trigger_id = $2",
        )
        .bind(flow)
        .bind(trigger_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }

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

    // 7. A re-enabled schedule on the queue whose next date has passed: skip
    //    or recover the ticks missed while it was off. `evaluated_at` and
    //    `last_error` are never written; a row off the queue stays off (the
    //    next save re-queues it).
    let mut moved_to = None;
    if let (false, StateTable::Schedule, Some((Some(next), evaluated_at))) =
        (disabled, table, state_row)
    {
        let db_now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
        // A schedule that no longer resolves: its dates are left alone. Its
        // row is already off the queue, or `fire_due` takes it off (with the
        // reason in `last_error`) when this date comes due.
        if next <= db_now {
            if let Ok(st) = ScheduleTrigger::resolve(t) {
                let skip = || {
                    TriggerState::initialise(&st, "", "", &FixedClock(db_now)).next_evaluation_date
                };
                let target = match recover {
                    None | Some(RecoverMissed::None) => skip(),
                    Some(RecoverMissed::All) => None,
                    Some(RecoverMissed::Last) => match evaluated_at {
                        None => skip(),
                        Some(ev) => st.last_tick_at_or_before(db_now).filter(|l| *l > ev),
                    },
                };
                // `None`: keep the dates (`ALL`, a `LAST` with nothing newer,
                // or a cron with no tick after now, which `fire_due` reports).
                if let Some(target) = target {
                    sqlx::query(
                        "UPDATE workflow.triggers SET next_evaluation_date = $3, updated_at = now() \
                          WHERE flow_id = $1 AND trigger_id = $2",
                    )
                    .bind(flow)
                    .bind(trigger_id)
                    .bind(target)
                    .execute(&mut *conn)
                    .await
                    .map_err(db_err)?;
                    moved_to = Some(target);
                }
            }
        }
    }

    Ok(Ok(Switched {
        flow: row,
        parsed,
        moved_to,
    }))
}
