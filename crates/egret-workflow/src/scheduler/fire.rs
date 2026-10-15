//! `fire_due`: fire every due `Schedule` trigger once, one trigger per
//! transaction (spec §4, §5).

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Schedule, TriggerKind};
use crate::repo::{flow_cols, FlowRow};
use crate::trigger::{
    tick, BackfillEnd, FixedClock, RecoverMissed, ScheduleTrigger, ScheduledExecution, TickOutcome,
    TriggerState,
};
use crate::worker::txn::{transaction, Ended, Fetched, Finish, Isolation, Txn};
use crate::worker::{start_execution_in_with, NewExecution};

// The off switches' SQL, the feature key and the start classification are
// shared with the Flow-trigger processor (W13a Task 4); see `start.rs`.
use super::backfill::BackfillRow;
use super::start::{classify, flow_is_on, workflows_feature_default, Started, WORKFLOWS_FEATURE};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Kestra's `Schedule` type name, as stored in `execution_trigger.type` and
/// matched by `workflow_executions_scheduled_date_iff_schedule`.
const SCHEDULE_TYPE: &str = "io.kestra.plugin.core.trigger.Schedule";

/// What one pass did, one count per claimed trigger.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    /// An execution was created and the dates advanced.
    pub fired: usize,
    /// The tick was consumed without an execution: `tick` skipped it, or its
    /// inputs were refused (spec §5), or the unique index showed the tick
    /// already had its execution. The reason is in `last_error`.
    pub skipped: usize,
    /// The trigger could not be fired, reason in `last_error`. Either it was
    /// taken off the queue (`next_evaluation_date = NULL`: the source does not
    /// parse, resolve or plan) or, for any other failure, its dates were left
    /// alone so the next pass retries the same tick.
    pub errored: usize,
    /// Nothing fired and nothing moved. Either the source disagrees with the
    /// row -- the trigger is off, no longer declared, or its `spec_key` is not
    /// the row's -- and `sync` re-ran for the flow (unless a writer held the
    /// flow row, which then puts the copy right itself); or the trigger's
    /// backfill was found paused by `fire_one`'s fresh read (a pause that
    /// committed between the claim's snapshot and its lock; W7f §3.6); or
    /// `tick` declined a row the claim found due (`NotDue`/`NotSchedulable`),
    /// which should not happen and is logged at warn.
    ///
    /// A trigger whose backfill is paused **before** the pass is not claimed
    /// at all (W7f D7) and is counted nowhere.
    pub frozen: usize,
}

/// The claimed trigger and what the claim read with it, all under the row lock.
#[derive(sqlx::FromRow)]
struct Claimed {
    flow_id: Uuid,
    trigger_id: String,
    /// What `sync` last wrote. Compared with the source's own (final review
    /// I2): under `FOR UPDATE OF t` a claim that waited re-reads the NEWEST
    /// `t` but keeps its snapshot's `f`, so the two can disagree.
    spec_key: String,
    next_evaluation_date: Option<DateTime<Utc>>,
    evaluated_at: Option<DateTime<Utc>>,
    workspace_id: Uuid,
    source: String,
    namespace: String,
    flow_key: String,
    db_now: DateTime<Utc>,
}

enum Fate {
    Fired,
    Skipped,
    Errored,
    Frozen,
}

/// One pass: every due trigger evaluated at most once. Returns when none is
/// left.
///
/// Each trigger is claimed, fired and advanced in **its own** transaction, so
/// the execution and the advance commit together or not at all (spec §4
/// point 1). `SKIP LOCKED` partitions the due set between replicas; `seen`
/// makes "at most once per pass" a property of the query, so a trigger that is
/// days behind -- always the oldest due row -- cannot take the whole pass.
///
/// A trigger whose transaction fails returns `Err` and ends the pass; its
/// rollback leaves the row as it was, so the next pass retries it.
///
/// # A miss is not a failed fire (W9c Task 9)
///
/// The fire's first settle renders through the transaction's cache
/// (`storage::prefetch`). A render that needs a read rolls the WHOLE
/// trigger transaction back -- the trigger row's lock with it -- in
/// `worker::txn::transaction!`, which fetches the read with nothing held and
/// claims again; the trigger is still the oldest due row, so it is claimed
/// again (another replica may take it in between, and then this pass fires
/// whatever it claims). The rolled-back attempt went on with a placeholder
/// for every read it missed (the user's ruling of 2026-10-08, "gather"), so
/// it may have classified a start that read a placeholder -- a fate, an
/// error recorded on the row -- and all of it is discarded with the
/// rollback: no `last_error`, no `errored`, and the trigger joins `seen`
/// only once an attempt commits.
pub async fn fire_due(pool: &PgPool) -> Result<PassReport> {
    let mut report = PassReport::default();
    let mut seen_flows: Vec<Uuid> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    loop {
        let ended = transaction!(
            pool,
            Isolation::Default,
            |at| { claim_and_fire(at, &seen_flows, &seen_ids).await },
            |pf| {
                pf.fetch_pending().await?;
                Ok(Fetched::Again)
            }
        )?;
        let Ended::Done(fired) = ended else {
            return Err(Error::Internal(anyhow::anyhow!(
                "a schedule fire's plain fetch cannot give up"
            )));
        };
        let Some((flow_id, trigger_id, fate)) = fired else {
            return Ok(report);
        };
        seen_flows.push(flow_id);
        seen_ids.push(trigger_id);
        match fate {
            Fate::Fired => report.fired += 1,
            Fate::Skipped => report.skipped += 1,
            Fate::Errored => report.errored += 1,
            Fate::Frozen => report.frozen += 1,
        }
    }
}

/// One attempt of [`fire_due`]'s transaction: claim the oldest due trigger
/// not yet seen, and fire it. `Rollback(None)` when nothing is due.
async fn claim_and_fire(
    at: &mut Txn,
    seen_flows: &[Uuid],
    seen_ids: &[String],
) -> Result<Finish<Option<(Uuid, String, Fate)>>> {
    let claimed: Option<Claimed> = sqlx::query_as(concat!(
        "SELECT t.flow_id, t.trigger_id, t.spec_key, t.next_evaluation_date, t.evaluated_at, \
                    f.workspace_id, f.source, f.namespace, f.flow_id AS flow_key, \
                    now() AS db_now \
               FROM workflow.triggers t JOIN workflow.flows f ON f.id = t.flow_id \
              WHERE t.next_evaluation_date <= now() \
                AND NOT t.disabled \
                AND NOT EXISTS (SELECT 1 FROM workflow.trigger_backfills b \
                                 WHERE b.flow_id = t.flow_id AND b.trigger_id = t.trigger_id \
                                   AND b.paused) \
                AND",
        flow_is_on!("$3", "$4"),
        "AND (t.flow_id, t.trigger_id) NOT IN \
                    (SELECT * FROM unnest($1::uuid[], $2::text[])) \
              ORDER BY t.next_evaluation_date \
              LIMIT 1 \
              FOR UPDATE OF t SKIP LOCKED",
    ))
    .bind(seen_flows)
    .bind(seen_ids)
    .bind(WORKFLOWS_FEATURE)
    .bind(workflows_feature_default())
    .fetch_optional(at.conn())
    .await
    .map_err(db_err)?;
    let Some(c) = claimed else {
        return Ok(Finish::Rollback(None));
    };
    // The cache is this trigger's: a retry that claims another one (this
    // one was taken by another replica meanwhile) starts it over.
    at.gate()
        .prefetch()
        .bind(&format!("{}/{}", c.flow_id, c.trigger_id));
    let fate = fire_one(at, &c).await?;
    Ok(Finish::Commit(Some((c.flow_id, c.trigger_id, fate))))
}

/// Everything that happens to one claimed trigger, inside its transaction.
///
/// # A backfill (W7f §3.6)
///
/// After the claim, the trigger's backfill row is read with a fresh
/// statement. Every writer of that row holds the trigger row first (§3.11),
/// and this transaction holds it from the claim, so the read is current; it
/// does not lean on the claim's snapshot, whose other tables may be older
/// than the `t` an EvalPlanQual re-check gave it. A backfill paused there
/// writes nothing and is `Frozen`; so `tick` is never handed a paused window
/// and its `BackfillPaused` skip is unreachable here (the claim's `NOT
/// EXISTS` excludes the rest).
///
/// The tick's outcome is handled as for a live tick, and then, in the same
/// transaction and after the trigger row (the lock order of §3.11), the
/// backfill row is advanced to the window's next tick or, when the next tick
/// passes `window_end`, deleted, with the trigger's next date replaced by the
/// regular schedule's resume (§3.6). A refused start of a backfill tick ends
/// the backfill the same way, resuming as a delete does (D25). The early
/// returns (`take_off_queue`, `record_error`) leave the backfill row alone.
async fn fire_one(at: &mut Txn, c: &Claimed) -> Result<Fate> {
    let flow = match crate::spec::parse(&c.source) {
        Ok(f) => f,
        Err(e) => {
            take_off_queue(
                at.conn(),
                c,
                &format!("the flow does not parse: {}", e.message),
            )
            .await?;
            return Ok(Fate::Errored);
        }
    };
    let trigger = flow
        .triggers
        .iter()
        .find(|t| t.id == c.trigger_id && matches!(t.kind, TriggerKind::Schedule(_)));
    // The source read under the lock is the authority and the row is a copy
    // (spec §4 point 5). When the copy says "on" and the source says "off" --
    // or no longer declares this schedule at all, or declares it with a cron
    // the copy's `spec_key` does not describe -- nothing fires, the dates
    // stay, and `sync` puts the copy right. The last case is the stale-`f`
    // race (final review I2): firing would evaluate the old cron against the
    // new row and store its dates under the new key for good. `resync` is a
    // fresh statement, so it reads the committed source; it skips the `sync`
    // when a writer holds the flow row (see `resync`).
    let declared = trigger
        .filter(|t| !flow.disabled && !t.disabled)
        .and_then(|t| match &t.kind {
            TriggerKind::Schedule(s) if super::sync::spec_key(s) == c.spec_key => Some((t, s)),
            _ => None,
        });
    let Some((trigger, schedule)) = declared else {
        resync(at.conn(), c.flow_id).await?;
        return Ok(Fate::Frozen);
    };
    let resolved = match ScheduleTrigger::resolve(trigger) {
        Ok(r) => r,
        Err(e) => {
            take_off_queue(at.conn(), c, &e.to_string()).await?;
            return Ok(Fate::Errored);
        }
    };

    // §3.6: a fresh statement, after the claim's lock; every writer of the
    // backfill row holds the trigger row first (§3.11), so this read is
    // current. A pause that committed between the claim's snapshot and its
    // lock is seen here.
    #[cfg(any(test, feature = "test-util"))]
    hold::after_claim(c.flow_id).await;
    let backfill = super::backfill::read(at.conn(), c.flow_id, &c.trigger_id).await?;
    if backfill.as_ref().is_some_and(|b| b.paused) {
        return Ok(Fate::Frozen);
    }

    // The schedule's `inputs:` overlaid by the backfill's, key by key; the
    // template refusal applies to the merged result (§3.4).
    let mut inputs = schedule.inputs.clone().unwrap_or_default();
    if let Some(over) = backfill.as_ref().and_then(|b| b.inputs.as_object()) {
        inputs = overlay_inputs(&inputs, over);
    }
    let refused = refuse_templated(&inputs);
    let state = TriggerState {
        namespace: c.namespace.clone(),
        flow_id: c.flow_key.clone(),
        trigger_id: c.trigger_id.clone(),
        next_evaluation_date: c.next_evaluation_date,
        evaluated_at: c.evaluated_at,
        backfill: backfill.as_ref().map(BackfillRow::window),
    };
    let mut t = tick(&resolved, &state, &inputs, &FixedClock(c.db_now));

    let mut refused_backfill = false;
    let (fate, last_error) = match t.outcome {
        // The claim's `WHERE` and `tick` read the same `now()`, and a
        // disabled trigger returned above, so neither can happen. If one did,
        // nothing is written: the row stays due, is claimed again next pass,
        // and says so here every time rather than passing as a quiet skip.
        declined @ (TickOutcome::NotDue | TickOutcome::NotSchedulable(_)) => {
            let outcome = format!("{declined:?}");
            // Loud in a debug build, so a test that ever reaches this fails.
            if cfg!(debug_assertions) {
                panic!("tick declined a claimed trigger: {outcome}");
            }
            tracing::warn!(
                flow_id = %c.flow_id,
                trigger_id = %c.trigger_id,
                outcome,
                "tick declined a trigger the claim found due; nothing written"
            );
            return Ok(Fate::Frozen);
        }
        TickOutcome::Skipped(reason) => (Fate::Skipped, Some(format!("skipped: {reason:?}"))),
        // Refused inputs consume the tick (spec §5): the dates advance
        // exactly as `tick` computed them, and no execution is created.
        TickOutcome::Fired(_) if refused.is_some() => (Fate::Skipped, refused),
        TickOutcome::Fired(e) => match start(at, c, &e, backfill.as_ref()).await? {
            Started::Yes => (Fate::Fired, None),
            // A unique index (the per-tick one, or the per-backfill-tick one
            // for a backfill's fire) says this tick already has its
            // execution: the tick is consumed, not refired and not taken off
            // the queue.
            // Under statement 1 of `scheduler`'s safety section this never
            // happens, so it is a warning, not a routine skip.
            Started::AlreadyFired(why) => {
                tracing::warn!(
                    flow_id = %c.flow_id,
                    trigger_id = %c.trigger_id,
                    scheduled_date = %e.scheduled_date,
                    "a schedule tick that already has an execution was claimed again; consumed"
                );
                (Fate::Skipped, Some(why))
            }
            // Deterministic in the source: only the author's next save helps.
            // A backfill tick's refusal ends the backfill instead (below): a
            // save would re-derive the same tick with the same inputs, so
            // taking the trigger off the queue would leave it stuck until the
            // backfill was deleted (W7f §3.6, default decision 2026-10-15).
            Started::Refused(why) if e.from_backfill && backfill.is_some() => {
                refused_backfill = true;
                (Fate::Errored, Some(format!("backfill ended: {why}")))
            }
            Started::Refused(why) => {
                take_off_queue(at.conn(), c, &why).await?;
                return Ok(Fate::Errored);
            }
            // Anything else may be transient (a timeout, a deadlock, a
            // serialization failure). The dates stay, so the next pass fires
            // this same tick again (spec §4 point 1); only the reason is kept.
            Started::Failed(why) => {
                record_error(at.conn(), c, &why).await?;
                return Ok(Fate::Errored);
            }
        },
    };

    let mut last_error = last_error;
    if let Some(b) = &backfill {
        let ended = match (&t.state.backfill, refused_backfill) {
            // A refused start ends the backfill as a delete does: the unfired
            // `current_tick` and the rest of the window are dropped.
            (_, true) => Some(BackfillEnd::Deleted {
                current_tick: b.current_tick,
            }),
            // The window goes on: the row follows the trigger's next date.
            // `LEAST` (D21): `total_ticks` and this walk use the same cron
            // sequence, so the clamp should never bite; an unclamped write
            // that broke the table's CHECK would fail this transaction and
            // end every pass.
            (Some(next), false) => {
                sqlx::query(
                    "UPDATE workflow.trigger_backfills \
                        SET current_tick = $3, ticks_done = LEAST(ticks_done + 1, total_ticks), \
                            updated_at = now() \
                      WHERE flow_id = $1 AND trigger_id = $2",
                )
                .bind(c.flow_id)
                .bind(&c.trigger_id)
                .bind(next.current_date)
                .execute(at.conn())
                .await
                .map_err(db_err)?;
                None
            }
            // The next tick passed `window_end`: the backfill is complete.
            (None, false) => Some(BackfillEnd::Completed {
                window_end: b.window_end,
            }),
        };
        // Ended: the row goes, and the trigger's next date is the regular
        // schedule's resume, not `tick`'s first tick after now.
        if let Some(end) = ended {
            sqlx::query(
                "DELETE FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = $2",
            )
            .bind(c.flow_id)
            .bind(&c.trigger_id)
            .execute(at.conn())
            .await
            .map_err(db_err)?;
            (t.state.next_evaluation_date, last_error) =
                backfill_ended(&resolved, schedule, b, end, c.db_now, last_error);
        }
    }

    sqlx::query(
        "UPDATE workflow.triggers \
            SET next_evaluation_date = $3, evaluated_at = $4, last_error = $5, updated_at = now() \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(c.flow_id)
    .bind(&c.trigger_id)
    .bind(t.state.next_evaluation_date)
    .bind(t.state.evaluated_at)
    .bind(last_error)
    .execute(at.conn())
    .await
    .map_err(db_err)?;
    Ok(fate)
}

/// Where the regular schedule resumes when backfill `b` ends (W7f §3.6), and
/// the `last_error` to store with it.
///
/// `policy` is the trigger's `recoverMissedSchedules`; a value `from_model`
/// does not know is `ALL`, egret's behaviour wherever it does not apply the
/// property. No tick at all: off the queue (`None`) with `never_fires`.
///
/// The stored `previous_next_evaluation_date` is first snapped to the
/// current cron (its first tick at or after it; default decision
/// 2026-10-15): a cron changed since the backfill began must not queue the
/// regular schedule on a date that is not one of its ticks. No tick to snap
/// to: off the queue with `never_fires` (D20).
///
/// Shared with `sync`, which ends a backfill the same way on a cron change
/// (§3.7: complete, or over the cap).
pub(super) fn backfill_ended(
    resolved: &ScheduleTrigger,
    schedule: &Schedule,
    b: &BackfillRow,
    end: BackfillEnd,
    now: DateTime<Utc>,
    last_error: Option<String>,
) -> (Option<DateTime<Utc>>, Option<String>) {
    let policy = RecoverMissed::from_model(schedule.recover_missed_schedules.as_ref())
        .unwrap_or(RecoverMissed::All);
    let next = resolved
        .first_tick_at_or_after(b.previous_next_evaluation_date)
        .and_then(|prev| {
            let from = TriggerState::resume_from(resolved, prev, end);
            TriggerState::resume_after_backfill(resolved, from, policy, now)
        });
    let last_error = match (next, last_error) {
        // Both kept, `never_fires` first: it is the state the row stays in.
        (None, Some(why)) => Some(format!(
            "{}; {why}",
            super::never_fires(schedule.cron.as_deref(), now)
        )),
        (None, None) => Some(super::never_fires(schedule.cron.as_deref(), now)),
        (Some(_), why) => why,
    };
    (next, last_error)
}

/// Create the execution inside a savepoint, so that whatever goes wrong in it
/// -- including a statement Postgres aborted -- can be rolled back while the
/// claim's transaction lives on to record what happened. The outer `Err` is a
/// failure of the transaction itself.
///
/// W9c Task 9: the savepoint is the attempt's ([`Txn::savepoint`]), so
/// the first settle renders under `fire_due`'s gate. A start whose render
/// missed is classified like any other -- with a placeholder it usually
/// starts -- and the gate rolls the whole attempt back, its fate with it.
///
/// A backfill's fire (`e.from_backfill`, with the row `fire_one` read)
/// carries the backfill's id and labels (NULL for an empty list); the
/// `execution_trigger` record is a live fire's (§3.2).
async fn start(
    at: &mut Txn,
    c: &Claimed,
    e: &ScheduledExecution,
    backfill: Option<&BackfillRow>,
) -> Result<Started> {
    let backfill = backfill.filter(|_| e.from_backfill);
    let labels = backfill
        .map(|b| b.labels.clone())
        .filter(|l| l.as_array().is_some_and(|a| !a.is_empty()));
    let mut sp = at.savepoint().await?;
    let started = start_execution_in_with(
        &mut sp,
        NewExecution {
            workspace_id: c.workspace_id,
            flow_id: Some(c.flow_id),
            source: &c.source,
            inputs: serde_json::to_value(&e.inputs).map_err(|e| Error::Internal(e.into()))?,
            trigger: Some(schedule_trigger_record(e)),
            scheduled_date: Some(e.scheduled_date),
            // A `Schedule` fire has no source execution and starts a chain.
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels,
            backfill_id: backfill.map(|b| b.id),
        },
    )
    .await;
    match started {
        Ok(_) => {
            sp.release().await?;
            Ok(Started::Yes)
        }
        Err(err) => {
            sp.rollback().await?;
            Ok(classify(&err))
        }
    }
}

/// Keep the reason, touch nothing else: the dates stay where the claim found
/// them, so the next pass retries the same tick.
async fn record_error(conn: &mut PgConnection, c: &Claimed, why: &str) -> Result<()> {
    sqlx::query(
        "UPDATE workflow.triggers SET last_error = $3, updated_at = now() \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(c.flow_id)
    .bind(&c.trigger_id)
    .bind(why)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Spec §4 point 6: a trigger that cannot be evaluated leaves the queue
/// rather than costing a failed transaction every pass. The next save of the
/// flow (`sync`) puts it back.
async fn take_off_queue(conn: &mut PgConnection, c: &Claimed, why: &str) -> Result<()> {
    sqlx::query(
        "UPDATE workflow.triggers \
            SET next_evaluation_date = NULL, last_error = $3, updated_at = now() \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(c.flow_id)
    .bind(&c.trigger_id)
    .bind(why)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Run `sync` for the claimed trigger's flow, inside the pass's transaction --
/// **only if the flow row is free**, read `FOR SHARE SKIP LOCKED`.
///
/// Every other `sync` caller holds the flow row (a save `FOR NO KEY UPDATE`
/// or `FOR UPDATE`, `reconcile` `FOR SHARE`), and so does the W7e switch route
/// while it writes `workflow.disabled_triggers` and a trigger's copy. Without
/// the lock, `sync` could read the switch rows before a concurrent route
/// commits, wait on the trigger row the route holds, and then write the stale
/// `disabled` over the route's value -- a lost update that leaves a
/// switched-off trigger firing (W7e review I1, pinned by
/// `tests/disabled_triggers.rs`'s
/// `a_resync_does_not_undo_a_concurrent_switch_on_another_trigger`).
///
/// `SKIP LOCKED`, not a waiting `FOR SHARE`: the pass already holds the
/// claimed trigger's row, and a writer holding the flow may want that same
/// row next (the route switching the claimed trigger, a save upserting it),
/// so waiting would be a deadlock. Not `NOWAIT` either: its error would abort
/// the pass's transaction and count as a failure. When the flow is held, this
/// does nothing: the trigger stays frozen (nothing fires, its dates stay), the
/// holder's own write -- a save's `sync`, or the route's copy -- runs after
/// this transaction commits, and a later pass that finds the row still
/// disagreeing resyncs it then. `FOR SHARE` does not conflict with the `KEY
/// SHARE` a fire's execution insert takes, nor with `reconcile`'s `FOR SHARE`.
async fn resync(conn: &mut PgConnection, flow_id: Uuid) -> Result<()> {
    let flow: Option<FlowRow> = sqlx::query_as(concat!(
        "SELECT ",
        flow_cols!(),
        " FROM workflow.flows WHERE id = $1 FOR SHARE SKIP LOCKED"
    ))
    .bind(flow_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    match flow {
        Some(flow) => super::sync(conn, &flow, false).await,
        None => {
            tracing::debug!(
                flow_id = %flow_id,
                "flow row held by a writer; resync left to it or to a later pass"
            );
            Ok(())
        }
    }
}

/// Spec §5: `Schedule.inputs` are passed verbatim, because this seam has no
/// render context. A value carrying template syntax would run with the
/// literal text where Kestra would have rendered it, so it is refused.
pub(crate) fn refuse_templated(inputs: &BTreeMap<String, serde_json::Value>) -> Option<String> {
    fn templated(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::String(s) => s.contains("{{") || s.contains("{%"),
            serde_json::Value::Array(a) => a.iter().any(templated),
            serde_json::Value::Object(o) => o.values().any(templated),
            _ => false,
        }
    }
    inputs.iter().find(|(_, v)| templated(v)).map(|(k, _)| {
        format!("inputs.{k} contains template syntax, which a scheduled execution cannot render")
    })
}

/// Kestra's `putAll`: `schedule` overlaid by `backfill`, key by key.
pub(crate) fn overlay_inputs(
    schedule: &BTreeMap<String, serde_json::Value>,
    backfill: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    let mut merged = schedule.clone();
    merged.extend(backfill.iter().map(|(k, v)| (k.clone(), v.clone())));
    merged
}

/// `execution.trigger` for a fired schedule, in the shape Task 1 measured on
/// the pinned artifact (`oracle-exec/w12-schedule-trigger.json`): `{id, type,
/// variables: {date, next, previous}}`, each date RFC 3339 in the trigger's
/// zone, with that zone's offset and six fractional digits.
///
/// A zero offset is written `Z`, which is what Java's `XXX` offset pattern
/// does; that case was **not** measured (the probe used `+07:00`).
pub(crate) fn schedule_trigger_record(e: &ScheduledExecution) -> serde_json::Value {
    let fmt = |at: DateTime<Utc>| {
        at.with_timezone(&e.timezone)
            .to_rfc3339_opts(SecondsFormat::Micros, true)
    };
    serde_json::json!({
        "id": e.trigger_id,
        "type": SCHEDULE_TYPE,
        "variables": {
            "date": fmt(e.scheduled_date),
            "next": e.next_date.map(fmt),
            "previous": e.previous_date.map(fmt),
        }
    })
}

/// Test-only (`test-util`; W7f D22, spec §6): park a pass right after its
/// claim, before `fire_one` reads the backfill row. The window between the
/// claim's `SELECT … FOR UPDATE OF t SKIP LOCKED` and the next statement
/// writes nothing and the claim never waits, so no lock or table trigger can
/// hold a pass there; this can.
///
/// Armed **for one flow**, and one-shot: the passes of other tests in the
/// same process (`tests/runtime.rs` runs its tests concurrently, each on its
/// own database) and every later pass of this flow go straight through. A
/// release build has none of it.
#[cfg(any(test, feature = "test-util"))]
pub mod hold {
    use std::sync::{Arc, Mutex};

    use tokio::sync::Notify;
    use uuid::Uuid;

    type Armed = Option<(Uuid, Arc<Notify>, Arc<Notify>)>;
    static ARMED: Mutex<Armed> = Mutex::new(None);

    /// A pass parked by [`arm`].
    pub struct Hold {
        reached: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl Hold {
        /// Returns once a pass that claimed a trigger of the armed flow is
        /// parked.
        pub async fn reached(&self) {
            self.reached.notified().await;
        }

        /// Lets the parked pass go on (or one that has yet to arrive).
        pub fn release(&self) {
            self.release.notify_one();
        }
    }

    /// Park the next pass that claims a trigger of `flow`.
    pub fn arm(flow: Uuid) -> Hold {
        let reached = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut armed = ARMED.lock().expect("the hold's lock");
        debug_assert!(
            armed.is_none(),
            "the hold is armed twice: the first arm's pass would never be parked"
        );
        *armed = Some((flow, Arc::clone(&reached), Arc::clone(&release)));
        Hold { reached, release }
    }

    pub(super) async fn after_claim(flow: Uuid) {
        let armed = {
            let mut armed = ARMED.lock().expect("the hold's lock");
            match armed.as_ref() {
                Some((f, _, _)) if *f == flow => armed.take(),
                _ => None,
            }
        };
        if let Some((_, reached, release)) = armed {
            reached.notify_one();
            release.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Clock;
    use serde_json::json;

    /// W7f review follow-up 2: a completed backfill whose regular schedule has
    /// no tick left goes off the queue with `never_fires`, and says so even
    /// when the last tick's outcome had already set a `last_error` (both are
    /// kept, `never_fires` first: it is the state the row stays in).
    ///
    /// Pure, because no cron that ticks inside a window can run out of ticks
    /// within `SEARCH_DAYS` of now through the database: `0 0 30 2 *` (no
    /// tick ever) stands in for it.
    #[test]
    fn a_completion_with_no_tick_left_reports_never_fires() {
        let src = "id: f\nnamespace: n\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 0 30 2 *\"\n    recoverMissedSchedules: NONE\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
        let flow = crate::spec::parse(src).unwrap();
        let TriggerKind::Schedule(schedule) = &flow.triggers[0].kind else {
            panic!("a schedule")
        };
        let resolved = ScheduleTrigger::resolve(&flow.triggers[0]).unwrap();
        let now: DateTime<Utc> = "2026-10-15T00:00:00Z".parse().unwrap();
        let b = BackfillRow {
            id: Uuid::nil(),
            flow_id: Uuid::nil(),
            trigger_id: "s".into(),
            window_start: now - chrono::Duration::days(2),
            window_end: now - chrono::Duration::days(1),
            current_tick: now - chrono::Duration::days(1),
            paused: false,
            inputs: json!({}),
            labels: json!([]),
            previous_next_evaluation_date: now - chrono::Duration::days(3),
            total_ticks: 1,
            ticks_done: 0,
        };
        let end = BackfillEnd::Completed {
            window_end: b.window_end,
        };
        let never = crate::scheduler::never_fires(Some("0 0 30 2 *"), now);

        let (next, err) = backfill_ended(&resolved, schedule, &b, end, now, None);
        assert_eq!(next, None, "off the queue");
        assert_eq!(err.as_deref(), Some(never.as_str()));

        let (next, err) = backfill_ended(
            &resolved,
            schedule,
            &b,
            end,
            now,
            Some("skipped: the last tick".into()),
        );
        assert_eq!(next, None);
        assert_eq!(
            err.as_deref(),
            Some(format!("{never}; skipped: the last tick").as_str())
        );
    }

    /// W7f review follow-up 4: arming the hold twice is a test bug (the
    /// first arm's pass would never be parked).
    #[test]
    #[should_panic(expected = "armed twice")]
    fn arming_the_hold_twice_is_a_bug() {
        let flow = Uuid::new_v4();
        let _first = hold::arm(flow);
        let _second = hold::arm(flow);
    }

    /// Each captured execution is reproduced from the cron alone: the state
    /// is initialised a minute before the capture, ticked at the captured
    /// instant, and the record of what fired must equal what Kestra stored.
    #[test]
    fn schedule_trigger_record_matches_the_captured_execution() {
        let oracle: serde_json::Value =
            serde_json::from_str(include_str!("../../oracle-exec/w12-schedule-trigger.json"))
                .unwrap();
        let t = &oracle["flow"]["trigger"];
        let src = format!(
            "id: {}\nnamespace: {}\ntriggers:\n  - id: {}\n    type: {}\n    cron: \"{}\"\n    timezone: {}\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
            oracle["flow"]["id"].as_str().unwrap(),
            oracle["flow"]["namespace"].as_str().unwrap(),
            t["id"].as_str().unwrap(),
            t["type"].as_str().unwrap(),
            t["cron"].as_str().unwrap(),
            t["timezone"].as_str().unwrap(),
        );
        let flow = crate::spec::parse(&src).unwrap();
        let resolved = ScheduleTrigger::resolve(&flow.triggers[0]).unwrap();

        let executions = oracle["executions"].as_array().unwrap();
        assert_eq!(executions.len(), 2);
        for captured in executions {
            let at: DateTime<Utc> = captured["scheduleDate"].as_str().unwrap().parse().unwrap();
            let state = TriggerState::initialise(
                &resolved,
                &flow.namespace,
                &flow.id,
                &FixedClock(at - chrono::Duration::minutes(1)),
            );
            let clock = FixedClock(at);
            let TickOutcome::Fired(e) = tick(&resolved, &state, &BTreeMap::new(), &clock).outcome
            else {
                panic!("the captured instant {} must fire", clock.now());
            };
            assert_eq!(schedule_trigger_record(&e), captured["trigger"]);
        }
    }

    #[test]
    fn a_zero_offset_is_written_z() {
        let at: DateTime<Utc> = "2026-09-24T07:42:00Z".parse().unwrap();
        let e = ScheduledExecution {
            namespace: "n".into(),
            flow_id: "f".into(),
            trigger_id: "s".into(),
            scheduled_date: at,
            next_date: None,
            previous_date: None,
            timezone: chrono_tz::Tz::UTC,
            inputs: BTreeMap::new(),
            from_backfill: false,
        };
        assert_eq!(
            schedule_trigger_record(&e)["variables"]["date"],
            "2026-09-24T07:42:00.000000Z"
        );
    }

    /// The claim's feature predicate reads the key the route gate gates on.
    #[test]
    fn the_feature_key_is_the_one_the_route_gate_uses() {
        assert!(egret_workspace::features::is_known(WORKFLOWS_FEATURE));
        assert_eq!(
            egret_workspace::features::gated_feature("workflows/flows", "GET"),
            Some(WORKFLOWS_FEATURE)
        );
    }

    #[test]
    fn templated_values_are_found_at_any_depth() {
        let mut inputs = BTreeMap::new();
        inputs.insert("a".to_string(), serde_json::json!("plain"));
        assert_eq!(refuse_templated(&inputs), None);
        inputs.insert("b".to_string(), serde_json::json!({"k": ["{% if %}"]}));
        assert!(refuse_templated(&inputs).unwrap().contains("inputs.b"));
    }

    #[test]
    fn a_backfill_input_overlays_the_schedules_key_by_key() {
        let schedule: BTreeMap<String, serde_json::Value> =
            [("a".into(), json!("{{ x }}")), ("b".into(), json!(1))].into();
        let over = json!({"a": "plain"}).as_object().unwrap().clone();
        let merged = overlay_inputs(&schedule, &over);
        assert_eq!(
            merged,
            BTreeMap::from([("a".into(), json!("plain")), ("b".into(), json!(1))])
        );
        assert_eq!(refuse_templated(&merged), None);

        let extra = json!({"c": 2}).as_object().unwrap().clone();
        let merged = overlay_inputs(&schedule, &extra);
        assert_eq!(merged.len(), 3);
        assert!(refuse_templated(&merged).unwrap().contains("inputs.a"));
    }
}
