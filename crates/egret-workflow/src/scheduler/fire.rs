//! `fire_due`: fire every due `Schedule` trigger once, one trigger per
//! transaction (spec §4, §5).

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use egret_core::{Error, Result};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use crate::models::TriggerKind;
use crate::repo::{flow_cols, FlowRow};
use crate::storage::prefetch::Prefetch;
use crate::trigger::{
    tick, FixedClock, ScheduleTrigger, ScheduledExecution, TickOutcome, TriggerState,
};
use crate::worker::txn::{transaction, Ended, Fetched, Finish, Isolation};
use crate::worker::{prefetch_miss, start_execution_in_with, NewExecution};

// The off switches' SQL, the feature key and the start classification are
// shared with the Flow-trigger processor (W13a Task 4); see `start.rs`.
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
    /// the row's -- and `sync` re-ran for the flow; or `tick` declined a row
    /// the claim found due (`NotDue`/`NotSchedulable`), which should not
    /// happen and is logged at warn.
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
/// whatever it claims). Nothing about the rolled-back attempt is written or
/// counted: no `last_error`, no `errored`, and the trigger joins `seen` only
/// once an attempt commits.
pub async fn fire_due(pool: &PgPool) -> Result<PassReport> {
    let mut report = PassReport::default();
    let mut seen_flows: Vec<Uuid> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    loop {
        let ended = transaction!(
            pool,
            Isolation::Default,
            |tx, pf| { claim_and_fire(tx, pf, &seen_flows, &seen_ids).await },
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
    tx: &mut PgConnection,
    pf: &Prefetch,
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
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    let Some(c) = claimed else {
        return Ok(Finish::Rollback(None));
    };
    // The cache is this trigger's: a retry that claims another one (this
    // one was taken by another replica meanwhile) starts it over.
    pf.bind(&format!("{}/{}", c.flow_id, c.trigger_id));
    let fate = fire_one(tx, pf, &c).await?;
    Ok(Finish::Commit(Some((c.flow_id, c.trigger_id, fate))))
}

/// Everything that happens to one claimed trigger, inside its transaction.
async fn fire_one(conn: &mut PgConnection, pf: &Prefetch, c: &Claimed) -> Result<Fate> {
    let flow = match crate::spec::parse(&c.source) {
        Ok(f) => f,
        Err(e) => {
            take_off_queue(conn, c, &format!("the flow does not parse: {}", e.message)).await?;
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
    // fresh statement, so it reads the committed source.
    let declared = trigger
        .filter(|t| !flow.disabled && !t.disabled)
        .and_then(|t| match &t.kind {
            TriggerKind::Schedule(s) if super::sync::spec_key(s) == c.spec_key => Some((t, s)),
            _ => None,
        });
    let Some((trigger, schedule)) = declared else {
        resync(conn, c.flow_id).await?;
        return Ok(Fate::Frozen);
    };
    let resolved = match ScheduleTrigger::resolve(trigger) {
        Ok(r) => r,
        Err(e) => {
            take_off_queue(conn, c, &e.to_string()).await?;
            return Ok(Fate::Errored);
        }
    };

    let inputs = schedule.inputs.clone().unwrap_or_default();
    let refused = refuse_templated(&inputs);
    let state = TriggerState {
        namespace: c.namespace.clone(),
        flow_id: c.flow_key.clone(),
        trigger_id: c.trigger_id.clone(),
        next_evaluation_date: c.next_evaluation_date,
        evaluated_at: c.evaluated_at,
        backfill: None,
    };
    let t = tick(&resolved, &state, &inputs, &FixedClock(c.db_now));

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
        TickOutcome::Fired(e) => match start(conn, pf, c, &e).await? {
            Started::Yes => (Fate::Fired, None),
            // The unique index says this tick already has its execution: the
            // tick is consumed, not refired and not taken off the queue.
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
            Started::Refused(why) => {
                take_off_queue(conn, c, &why).await?;
                return Ok(Fate::Errored);
            }
            // Anything else may be transient (a timeout, a deadlock, a
            // serialization failure). The dates stay, so the next pass fires
            // this same tick again (spec §4 point 1); only the reason is kept.
            Started::Failed(why) => {
                record_error(conn, c, &why).await?;
                return Ok(Fate::Errored);
            }
        },
    };

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
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(fate)
}

/// Create the execution inside a savepoint, so that whatever goes wrong in it
/// -- including a statement Postgres aborted -- can be rolled back while the
/// claim's transaction lives on to record what happened. The outer `Err` is a
/// failure of the transaction itself.
///
/// W9c Task 9: a start whose first settle recorded a miss is NOT classified
/// -- it is not a failed fire, nor anything else: the outer `Err` ends this
/// attempt, and `fire_due`'s `transaction!` rolls it all back and fetches.
async fn start(
    conn: &mut PgConnection,
    pf: &Prefetch,
    c: &Claimed,
    e: &ScheduledExecution,
) -> Result<Started> {
    let mut sp = conn.begin().await.map_err(db_err)?;
    let started = start_execution_in_with(
        &mut sp,
        pf,
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
        },
    )
    .await;
    if pf.has_pending() {
        sp.rollback().await.map_err(db_err)?;
        return Err(prefetch_miss());
    }
    match started {
        Ok(_) => {
            sp.commit().await.map_err(db_err)?;
            Ok(Started::Yes)
        }
        Err(err) => {
            sp.rollback().await.map_err(db_err)?;
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

async fn resync(conn: &mut PgConnection, flow_id: Uuid) -> Result<()> {
    let flow: FlowRow = sqlx::query_as(concat!(
        "SELECT ",
        flow_cols!(),
        " FROM workflow.flows WHERE id = $1"
    ))
    .bind(flow_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    super::sync(conn, &flow, false).await
}

/// Spec §5: `Schedule.inputs` are passed verbatim, because this seam has no
/// render context. A value carrying template syntax would run with the
/// literal text where Kestra would have rendered it, so it is refused.
fn refuse_templated(inputs: &BTreeMap<String, serde_json::Value>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Clock;

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
}
