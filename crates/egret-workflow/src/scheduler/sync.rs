//! `sync`: keep a flow's trigger rows in step with its YAML -- `workflow.triggers`
//! for its `Schedule` triggers (W12 spec §3.3) and `workflow.flow_listeners` for
//! its `Flow` triggers (W13 spec §4.2). A `Webhook` trigger has no row; the
//! route resolves it from the flow's source on every request.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Schedule, State, Trigger, TriggerKind};
use crate::repo::{flow_cols, FlowRow};
use crate::trigger::{
    FixedClock, FlowTriggerError, ScheduleTrigger, TriggerState, WindowFreeFlowTrigger,
};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// The ONE place the effective `disabled` of a trigger is computed (W7e spec
/// §3.2): the flow's YAML `disabled:`, the trigger's own, or a row in
/// `workflow.disabled_triggers` (the runtime switch). `sync` writes it onto
/// `workflow.triggers` and `workflow.flow_listeners`, whose `NOT disabled`
/// the schedule claim, the listener select and the outbox insert already read.
pub(crate) fn effective_disabled(flow_yaml: bool, trigger_yaml: bool, runtime: bool) -> bool {
    flow_yaml || trigger_yaml || runtime
}

/// The `last_error` of a schedule that resolves but has no tick after `now`
/// (`0 0 30 2 *`): written by `sync` and by the switch route's skip, which
/// take the row off the queue for it.
pub(crate) fn never_fires(cron: Option<&str>, now: DateTime<Utc>) -> String {
    format!(
        "the schedule never fires: `{}` has no tick after {now}",
        cron.unwrap_or("")
    )
}

/// Bring `workflow.triggers`, `workflow.flow_listeners` and
/// `workflow.disabled_triggers` in line with `flow.source`. Called inside the
/// transaction that wrote `flow`; `gone = true` for a delete.
///
/// 1. The flow's `Schedule` and `Flow` triggers are collected in one pass --
///    none if `gone` or if the source does not parse. A `Webhook` trigger
///    never gets a row. The flow's switched-off trigger ids
///    (`workflow.disabled_triggers`) are read once, in the same transaction.
/// 2. Rows whose `trigger_id` is no longer declared are deleted.
/// 3. Each declared schedule is upserted. A new row, or one whose `spec_key`
///    changed, is initialised to the **next** cron tick after the
///    transaction's `now()`; an unchanged `spec_key` keeps its dates, so
///    editing a task never resets a schedule. `disabled` is the effective
///    value ([`effective_disabled`]: flow YAML, trigger YAML or the runtime
///    switch); it is written every time and never touches the dates, so
///    re-enabling recovers what was missed.
/// 4. When a row IS (re-)initialised and there is no first date -- `resolve`
///    refused the schedule, or it resolved but has no future tick (`0 0 30 2
///    *`) -- it is stored with `next_evaluation_date = NULL` and the reason in
///    `last_error`.
///
/// 5. The `Flow` triggers go to [`sync_flow_listeners`], in the same
///    transaction: one `workflow.flow_listeners` row per declaration, stale
///    rows deleted, `states`/`depends_on`/`disabled` rewritten every time,
///    `disabled` being the same effective value.
///
/// 6. The switch rows (W7e spec §3.4), after the trigger rows: a soft delete
///    (`gone`) deletes them all, since the FK cascade does not fire on one; a
///    source that parses deletes those whose `trigger_id` it no longer
///    declares, under any kind (a renamed id is a new trigger and starts on;
///    a kind change under the same id keeps its row); a source that does not
///    parse keeps every one, so no switch is lost to a broken source.
///
/// Points 2-4 are about `workflow.triggers`. Point 4 does not reach a row that keeps its dates. A refusal whose cause is
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
    // One pass over `flow.triggers` (spec §4.2): `all_triggers` is built once
    // and both `schedules` and `flow_triggers` filter it, rather than each
    // re-walking `parsed` with its own `flat_map`.
    let all_triggers: Vec<&Trigger> = parsed.iter().flat_map(|f| f.triggers.iter()).collect();
    let schedules: Vec<&Trigger> = all_triggers
        .iter()
        .copied()
        .filter(|t| matches!(t.kind, TriggerKind::Schedule(_)))
        .collect();
    let flow_triggers: Vec<&Trigger> = all_triggers
        .iter()
        .copied()
        .filter(|t| matches!(t.kind, TriggerKind::Flow(_)))
        .collect();
    let flow_disabled = parsed.as_ref().is_some_and(|f| f.disabled);
    let switched_off: Vec<String> =
        sqlx::query_scalar("SELECT trigger_id FROM workflow.disabled_triggers WHERE flow_id = $1")
            .bind(flow.id)
            .fetch_all(&mut *conn)
            .await
            .map_err(db_err)?;
    let runtime_off = |id: &str| switched_off.iter().any(|s| s == id);

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
        let disabled = effective_disabled(flow_disabled, t.disabled, runtime_off(&t.id));
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
                    None => (None, Some(never_fires(s.cron.as_deref(), now))),
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

    sync_flow_listeners(conn, flow, &flow_triggers, flow_disabled, &switched_off).await?;

    // The switch rows (point 6). `parsed` is `None` both for `gone` and for a
    // source that does not parse, so `gone` is tested first.
    if gone {
        sqlx::query("DELETE FROM workflow.disabled_triggers WHERE flow_id = $1")
            .bind(flow.id)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    } else if let Some(f) = &parsed {
        let declared: Vec<String> = f.triggers.iter().map(|t| t.id.clone()).collect();
        sqlx::query(
            "DELETE FROM workflow.disabled_triggers WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))",
        )
        .bind(flow.id)
        .bind(&declared)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    // `parsed == None && !gone`: the source does not parse; every row is kept.

    Ok(())
}

/// Keep `workflow.flow_listeners` in step with the flow's declared `Flow`
/// triggers (spec §4.2), in the same transaction as the schedule rows above.
///
/// - A `dependsOn` declaration gets a row with `depends_on = true` even though
///   [`WindowFreeFlowTrigger::resolve`] refuses it -- the processor's
///   `dependsOn` arm owns such rows (W13b), gating them itself, and `sync`
///   does not wait for validity:
///   `depends_on` is read straight off `spec.depends_on` before `resolve` is
///   ever called.
/// - A declaration that is not `dependsOn` but still fails to resolve (for
///   any reason `resolve` may one day report) gets **no row**, and the reason
///   is logged rather than silently dropped.
async fn sync_flow_listeners(
    conn: &mut PgConnection,
    flow: &FlowRow,
    flow_triggers: &[&Trigger],
    flow_disabled: bool,
    switched_off: &[String],
) -> Result<()> {
    struct ListenerRow<'a> {
        trigger_id: &'a str,
        states: &'a [State],
        depends_on: bool,
        disabled: bool,
    }

    let mut rows: Vec<ListenerRow> = Vec::new();
    for t in flow_triggers {
        let TriggerKind::Flow(spec) = &t.kind else {
            unreachable!("filtered to TriggerKind::Flow above")
        };
        let depends_on = !spec.depends_on.is_empty();
        let runtime_off = switched_off.contains(&t.id);
        let disabled = effective_disabled(flow_disabled, t.disabled, runtime_off);
        if depends_on {
            rows.push(ListenerRow {
                trigger_id: &t.id,
                states: &spec.states,
                depends_on: true,
                disabled,
            });
            continue;
        }
        match WindowFreeFlowTrigger::resolve(t) {
            Ok(_) => rows.push(ListenerRow {
                trigger_id: &t.id,
                states: &spec.states,
                depends_on: false,
                disabled,
            }),
            Err(FlowTriggerError::UnsupportedDependsOn(_)) => {
                unreachable!("depends_on is empty here, checked above")
            }
            Err(e) => {
                tracing::warn!(
                    flow_id = %flow.id,
                    trigger_id = %t.id,
                    error = %e,
                    "flow trigger could not be resolved; no listener row written"
                );
            }
        }
    }

    let keep: Vec<String> = rows.iter().map(|r| r.trigger_id.to_string()).collect();
    sqlx::query(
        "DELETE FROM workflow.flow_listeners WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))",
    )
    .bind(flow.id)
    .bind(&keep)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;

    for r in rows {
        let states: Vec<String> = r.states.iter().map(state_wire_name).collect();
        sqlx::query(
            "INSERT INTO workflow.flow_listeners \
               (flow_id, trigger_id, workspace_id, states, depends_on, disabled) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (flow_id, trigger_id) DO UPDATE SET \
               states = EXCLUDED.states, \
               depends_on = EXCLUDED.depends_on, \
               disabled = EXCLUDED.disabled",
        )
        .bind(flow.id)
        .bind(r.trigger_id)
        .bind(flow.workspace_id)
        .bind(&states)
        .bind(r.depends_on)
        .bind(r.disabled)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// `State`'s Kestra spelling (`"SUCCESS"`, `"PAUSED"`, ...), the same
/// `SCREAMING_SNAKE_CASE` [`State`]'s own `Serialize` produces -- reused
/// rather than hand-written so a future variant added to the enum cannot
/// silently desync the two spellings.
fn state_wire_name(s: &State) -> String {
    match serde_json::to_value(s).expect("State serialises to a JSON string") {
        serde_json::Value::String(name) => name,
        other => unreachable!("State must serialise to a JSON string, got {other:?}"),
    }
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
///
/// **Deliberately does not read `workspaces.status`.** An archived
/// workspace's flows are still synced -- their dates kept current, a fresh
/// row initialised, a stale one dropped -- exactly as an active workspace's
/// are. This is harmless: `sync` only ever writes queue state, never fires
/// anything, and `fire_one`'s claim (`fire.rs`) already excludes a non-active
/// workspace, so a row this leaves due simply sits frozen until the workspace
/// is re-activated. Filtering it out here would buy nothing and cost the one
/// thing catch-up needs -- current dates -- so re-activation would have to
/// re-sync before it could recover anything under `ALL`.
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
