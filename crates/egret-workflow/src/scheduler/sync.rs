//! `sync`: keep `workflow.triggers` in step with a flow's YAML (spec §3.3).

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Schedule, Trigger, TriggerKind};
use crate::repo::{flow_cols, FlowRow};
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
///    editing a task never resets a schedule. `disabled` is written every time
///    and never touches the dates, so re-enabling recovers what was missed.
/// 4. When a row IS (re-)initialised and there is no first date -- `resolve`
///    refused the schedule, or it resolved but has no future tick (`0 0 30 2
///    *`) -- it is stored with `next_evaluation_date = NULL` and the reason in
///    `last_error`.
///
/// Point 4 does not reach a row that keeps its dates. A refusal whose cause is
/// not in `spec_key` (`lateMaximumDelay`, `when`), written onto a row that is
/// already queued under an unchanged key, leaves the old date and the old
/// `last_error` in place; `fire_due` meets the refusal when that date comes
/// due and takes the row off the queue then.
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
            Ok(st) => {
                match TriggerState::initialise(&st, "", "", &FixedClock(now)).next_evaluation_date {
                    Some(next) => (Some(next), None),
                    // Off the queue like a refusal, and for the same reason it
                    // must say so: otherwise the author sees a schedule that
                    // is simply never due (final review I4).
                    None => (
                        None,
                        Some(format!(
                            "the schedule never fires: `{}` has no tick after {now}",
                            s.cron.as_deref().unwrap_or("")
                        )),
                    ),
                }
            }
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

/// What [`reconcile`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Flows `sync` ran for (or that were deleted before their turn came).
    pub synced: usize,
    /// Flows whose `sync` failed; each was logged and skipped.
    pub failed: usize,
}

/// Run [`sync`] once over every non-deleted flow -- the scheduler ticker's
/// first act, before its first pass (final review I1).
///
/// Migration 0156 created `workflow.triggers` empty and only a `FlowRepo`
/// write calls `sync`, so without this a flow saved before 0156 and never
/// saved again would never get its row, and its cron would never fire.
///
/// Idempotent: an unchanged `spec_key` keeps its dates, so running it on every
/// start of every replica costs one transaction per flow and moves nothing
/// that was right. Like a save, it re-queues a row `fire_due` took off the
/// queue, and drops the rows of a flow whose source no longer parses.
///
/// Each flow gets its own transaction, and a flow whose `sync` fails is logged
/// and skipped: one bad row must not keep every other schedule from firing.
/// `Err` only when the flows cannot be listed at all.
///
/// The flow is read `FOR SHARE`, so a concurrent `soft_delete` either commits
/// first -- and the re-checked `deleted_at IS NULL` finds nothing -- or waits
/// for this transaction; rows are never resurrected for a deleted flow. The
/// lock order is the save's (flow, then trigger rows), and a fire's `KEY
/// SHARE` on the flow does not conflict with `FOR SHARE`.
pub async fn reconcile(pool: &PgPool) -> Result<Reconciled> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workflow.flows WHERE deleted_at IS NULL ORDER BY id")
            .fetch_all(pool)
            .await
            .map_err(db_err)?;
    let mut done = Reconciled::default();
    for id in ids {
        match reconcile_one(pool, id).await {
            Ok(()) => done.synced += 1,
            Err(e) => {
                tracing::warn!(flow_id = %id, error = %e, "could not reconcile a flow's schedules; skipped");
                done.failed += 1;
            }
        }
    }
    Ok(done)
}

async fn reconcile_one(pool: &PgPool, id: Uuid) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db_err)?;
    let flow: Option<FlowRow> = sqlx::query_as(concat!(
        "SELECT ",
        flow_cols!(),
        " FROM workflow.flows WHERE id = $1 AND deleted_at IS NULL FOR SHARE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    if let Some(flow) = flow {
        sync(&mut tx, &flow, false).await?;
    }
    tx.commit().await.map_err(db_err)
}

/// `cron|withSeconds|timezone` -- the three fields that decide WHEN. A string,
/// not a digest: nothing needs it short, and a readable key is debuggable.
pub(super) fn spec_key(s: &Schedule) -> String {
    format!(
        "{}|{}|{}",
        s.cron.as_deref().unwrap_or(""),
        s.with_seconds,
        s.timezone.as_deref().unwrap_or("")
    )
}
