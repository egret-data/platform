//! `sync`: keep `workflow.triggers` in step with a flow's YAML (spec §3.3).

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::PgConnection;

use crate::models::{Schedule, Trigger, TriggerKind};
use crate::repo::FlowRow;
use crate::trigger::{FixedClock, ScheduleTrigger, TriggerState};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Bring `workflow.triggers` in line with `flow.source`. Called inside the
/// transaction that wrote `flow`; `gone = true` for a delete.
///
/// 1. The flow's `Schedule` triggers are collected -- none if `gone` or if the
///    source does not parse. Webhook and flow triggers never get a row.
/// 2. Rows whose `trigger_id` is no longer declared are deleted.
/// 3. Each declared schedule is upserted. A new row, or one whose `spec_key`
///    changed, is initialised to the **next** cron tick after the
///    transaction's `now()`; an unchanged `spec_key` keeps its dates, so
///    editing a task never resets a schedule. A schedule `resolve` refuses is
///    stored with `next_evaluation_date = NULL` and the refusal in
///    `last_error`. `disabled` is written every time and never touches the
///    dates, so re-enabling recovers what was missed.
pub async fn sync(conn: &mut PgConnection, flow: &FlowRow, gone: bool) -> Result<()> {
    // The loop's clock, read in the same transaction (spec §4 point 4): a
    // first date is never computed on one clock and compared on another.
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
    let parsed = if gone {
        None
    } else {
        crate::spec::parse(&flow.source).ok()
    };
    let schedules: Vec<&Trigger> = parsed
        .iter()
        .flat_map(|f| f.triggers.iter())
        .filter(|t| matches!(t.kind, TriggerKind::Schedule(_)))
        .collect();
    let flow_disabled = parsed.as_ref().is_some_and(|f| f.disabled);

    let keep: Vec<String> = schedules.iter().map(|t| t.id.clone()).collect();
    sqlx::query("DELETE FROM workflow.triggers WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))")
        .bind(flow.id)
        .bind(&keep)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

    for t in schedules {
        let TriggerKind::Schedule(s) = &t.kind else {
            unreachable!("filtered to schedules above")
        };
        let key = spec_key(s);
        let disabled = flow_disabled || t.disabled;
        // `initialise` reads only the trigger and the clock for the date; the
        // namespace/flow_id strings it copies are read from the flow at fire
        // time instead, so a rename cannot leave them stale.
        let (next, err) = match ScheduleTrigger::resolve(t) {
            Ok(st) => (
                TriggerState::initialise(&st, "", "", &FixedClock(now)).next_evaluation_date,
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        };
        // Dates are kept only when the spec_key is unchanged AND the row is on
        // the queue. A row off the queue (refused, or taken off by `fire_due`
        // for a broken source) is re-initialised by the next save -- spec §4
        // point 6. Without the `IS NOT NULL` term, a refusal `spec_key` cannot
        // see (`lateMaximumDelay`, `when`) would stay off the queue for ever
        // after the author fixed it. The term is keyed on the date, not on
        // `last_error`, because `fire_due` also writes `last_error` for a
        // consumed tick, and re-initialising that row would drop its backlog.
        sqlx::query(
            "INSERT INTO workflow.triggers \
               (flow_id, trigger_id, workspace_id, spec_key, disabled, next_evaluation_date, last_error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (flow_id, trigger_id) DO UPDATE SET \
               disabled = EXCLUDED.disabled, updated_at = now(), \
               spec_key = EXCLUDED.spec_key, \
               next_evaluation_date = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.next_evaluation_date ELSE EXCLUDED.next_evaluation_date END, \
               evaluated_at = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.evaluated_at ELSE NULL END, \
               last_error = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.last_error ELSE EXCLUDED.last_error END",
        )
        .bind(flow.id)
        .bind(&t.id)
        .bind(flow.workspace_id)
        .bind(&key)
        .bind(disabled)
        .bind(next)
        .bind(err)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// `cron|withSeconds|timezone` -- the three fields that decide WHEN. A string,
/// not a digest: nothing needs it short, and a readable key is debuggable.
fn spec_key(s: &Schedule) -> String {
    format!(
        "{}|{}|{}",
        s.cron.as_deref().unwrap_or(""),
        s.with_seconds,
        s.timezone.as_deref().unwrap_or("")
    )
}
