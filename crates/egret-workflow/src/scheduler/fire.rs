//! `fire_due`: fire every due `Schedule` trigger once, one trigger per
//! transaction (spec §4, §5).

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use egret_core::{Error, Result};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use crate::models::TriggerKind;
use crate::repo::{flow_cols, FlowRow};
use crate::trigger::{
    tick, FixedClock, ScheduleTrigger, ScheduledExecution, TickOutcome, TriggerState,
};
use crate::worker::{start_execution_in, NewExecution};

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
    /// The source says the trigger is off although the row said on: nothing
    /// fired, nothing moved, and `sync` re-ran for the flow.
    pub frozen: usize,
}

/// The claimed trigger and what the claim read with it, all under the row lock.
#[derive(sqlx::FromRow)]
struct Claimed {
    flow_id: Uuid,
    trigger_id: String,
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
pub async fn fire_due(pool: &PgPool) -> Result<PassReport> {
    let mut report = PassReport::default();
    let mut seen_flows: Vec<Uuid> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    loop {
        let mut tx = pool.begin().await.map_err(db_err)?;
        let claimed: Option<Claimed> = sqlx::query_as(
            "SELECT t.flow_id, t.trigger_id, t.next_evaluation_date, t.evaluated_at, \
                    f.workspace_id, f.source, f.namespace, f.flow_id AS flow_key, \
                    now() AS db_now \
               FROM workflow.triggers t JOIN workflow.flows f ON f.id = t.flow_id \
              WHERE t.next_evaluation_date <= now() \
                AND NOT t.disabled \
                AND f.deleted_at IS NULL \
                AND f.state <> 'disabled' \
                AND (t.flow_id, t.trigger_id) NOT IN \
                    (SELECT * FROM unnest($1::uuid[], $2::text[])) \
              ORDER BY t.next_evaluation_date \
              LIMIT 1 \
              FOR UPDATE OF t SKIP LOCKED",
        )
        .bind(&seen_flows)
        .bind(&seen_ids)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(c) = claimed else {
            return Ok(report);
        };
        seen_flows.push(c.flow_id);
        seen_ids.push(c.trigger_id.clone());

        match fire_one(&mut tx, &c).await? {
            Fate::Fired => report.fired += 1,
            Fate::Skipped => report.skipped += 1,
            Fate::Errored => report.errored += 1,
            Fate::Frozen => report.frozen += 1,
        }
        tx.commit().await.map_err(db_err)?;
    }
}

/// Everything that happens to one claimed trigger, inside its transaction.
async fn fire_one(conn: &mut PgConnection, c: &Claimed) -> Result<Fate> {
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
    // or no longer declares this schedule at all -- nothing fires, the dates
    // stay, and `sync` puts the copy right.
    let Some(trigger) = trigger.filter(|t| !flow.disabled && !t.disabled) else {
        resync(conn, c.flow_id).await?;
        return Ok(Fate::Frozen);
    };
    let TriggerKind::Schedule(schedule) = &trigger.kind else {
        unreachable!("found as a schedule above")
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
        // disabled trigger returned above, so neither can happen; if one did,
        // the state is unchanged and there is nothing to write.
        TickOutcome::NotDue => return Ok(Fate::Skipped),
        TickOutcome::NotSchedulable(_) => return Ok(Fate::Frozen),
        TickOutcome::Skipped(reason) => (Fate::Skipped, Some(format!("skipped: {reason:?}"))),
        // Refused inputs consume the tick (spec §5): the dates advance
        // exactly as `tick` computed them, and no execution is created.
        TickOutcome::Fired(_) if refused.is_some() => (Fate::Skipped, refused),
        TickOutcome::Fired(e) => match start(conn, c, &e).await? {
            Started::Yes => (Fate::Fired, None),
            // The unique index says this tick already has its execution: the
            // tick is consumed, not refired and not taken off the queue.
            Started::AlreadyFired(why) => (Fate::Skipped, Some(why)),
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

/// The index that makes a second execution for one tick impossible
/// (migration 0156).
const ONE_PER_TICK: &str = "workflow_executions_one_per_schedule_tick";

/// What starting a fired tick's execution came to.
enum Started {
    Yes,
    /// `Error::InvalidInput`: the source does not parse or plan, or an input
    /// is refused. Fixed only by a save, so the trigger leaves the queue.
    Refused(String),
    /// A unique violation on [`ONE_PER_TICK`]: the tick was already fired.
    AlreadyFired(String),
    /// Anything else. Possibly transient, so the tick is retried next pass.
    Failed(String),
}

fn classify(err: &Error) -> Started {
    let why = format!("the execution could not be started: {err}");
    match err {
        Error::InvalidInput(_) => Started::Refused(why),
        Error::Internal(source)
            if source
                .downcast_ref::<sqlx::Error>()
                .and_then(|e| e.as_database_error())
                .and_then(|d| d.constraint())
                == Some(ONE_PER_TICK) =>
        {
            Started::AlreadyFired(format!("this tick already has an execution: {err}"))
        }
        _ => Started::Failed(why),
    }
}

/// Create the execution inside a savepoint, so that whatever goes wrong in it
/// -- including a statement Postgres aborted -- can be rolled back while the
/// claim's transaction lives on to record what happened. The outer `Err` is a
/// failure of the transaction itself.
async fn start(conn: &mut PgConnection, c: &Claimed, e: &ScheduledExecution) -> Result<Started> {
    let mut sp = conn.begin().await.map_err(db_err)?;
    let started = start_execution_in(
        &mut sp,
        NewExecution {
            workspace_id: c.workspace_id,
            flow_id: Some(c.flow_id),
            source: &c.source,
            inputs: serde_json::to_value(&e.inputs).map_err(|e| Error::Internal(e.into()))?,
            trigger: Some(schedule_trigger_record(e)),
            scheduled_date: Some(e.scheduled_date),
        },
    )
    .await;
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

    #[test]
    fn templated_values_are_found_at_any_depth() {
        let mut inputs = BTreeMap::new();
        inputs.insert("a".to_string(), serde_json::json!("plain"));
        assert_eq!(refuse_templated(&inputs), None);
        inputs.insert("b".to_string(), serde_json::json!({"k": ["{% if %}"]}));
        assert!(refuse_templated(&inputs).unwrap().contains("inputs.b"));
    }
}
