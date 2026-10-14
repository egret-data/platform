//! The I/O half of reading triggers: the state tables, one statement each,
//! for the flow ids in scope, and §3.6's last execution per trigger.
//!
//! Every statement here is keyed by flow id (`= ANY($1)`), and the ids come
//! from a read already scoped to the caller's workspace; no statement here
//! reads a workspace column, so none can widen the scope it is handed.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

use super::routes::LastExecutionDto;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// §3.6's statement: the newest execution each `(flow_id, trigger id)` pair
/// started, matched on `execution_trigger->>'id'`, which every firing path
/// writes as the trigger's declared id (a schedule's `fire`, the webhook
/// route's `ExecutionTrigger`, the Flow-trigger processor). Executions started
/// from the API or by replay carry no `execution_trigger` and are never one.
///
/// `pub` (re-exported at the crate root) so `tests/disabled_triggers.rs`
/// `EXPLAIN`s exactly this text: the route and the plan the test pins cannot
/// drift apart. Served by `workflow_executions_last_by_trigger` (migration
/// 0189), an Index Scan per probe -- not index-only: `id` and `state` are
/// read from the heap.
pub const LAST_EXECUTION_SQL: &str = "\
SELECT k.flow_id, k.trigger_id, e.id, e.state, e.created_at
  FROM unnest($1::uuid[], $2::text[]) AS k(flow_id, trigger_id)
  CROSS JOIN LATERAL (
    SELECT id, state, created_at FROM workflow.executions e
     WHERE e.flow_id = k.flow_id
       AND e.execution_trigger IS NOT NULL
       AND e.execution_trigger->>'id' = k.trigger_id
     ORDER BY e.created_at DESC LIMIT 1) e";

/// A row of `workflow.disabled_triggers`: the trigger is switched off at
/// runtime. `disabled_by_name` is the switcher's display name, read in the
/// same statement (the browser cannot resolve a user id: the user listings
/// are admin-gated).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct Switch {
    pub flow_id: Uuid,
    pub trigger_id: String,
    pub disabled_by: Option<Uuid>,
    pub disabled_by_name: Option<String>,
    pub disabled_at: DateTime<Utc>,
}

/// A row of `workflow.triggers` (a `Schedule`).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct ScheduleRow {
    pub flow_id: Uuid,
    pub trigger_id: String,
    pub next_evaluation_date: Option<DateTime<Utc>>,
    pub evaluated_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// A row of `workflow.flow_listeners` (a `Flow` trigger that listens).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct ListenerRow {
    pub flow_id: Uuid,
    pub trigger_id: String,
}

/// A row of `workflow.flow_trigger_windows` (a `dependsOn` window).
/// `results` is `{condition key -> true}`; only `true` is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct WindowRow {
    pub flow_id: Uuid,
    pub trigger_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub results: serde_json::Value,
}

/// `(flow id, trigger id)`: the key of every trigger table.
pub(crate) type Key = (Uuid, String);

/// The state of the flows in scope, keyed by `(flow id, trigger id)`.
/// Named `StateRows`, not `TriggerState`: `crate::trigger::TriggerState`
/// already exists.
#[derive(Debug, Clone)]
pub(crate) struct StateRows {
    pub switches: HashMap<Key, Switch>,
    pub schedules: HashMap<Key, ScheduleRow>,
    pub listeners: HashMap<Key, ListenerRow>,
    pub windows: HashMap<Key, WindowRow>,
    /// The database's `now()`, the clock the scheduler compares against.
    pub db_now: DateTime<Utc>,
}

impl StateRows {
    /// No rows, at `db_now` (the pure tests' starting point).
    #[cfg(test)]
    pub(crate) fn empty(db_now: DateTime<Utc>) -> Self {
        Self {
            switches: HashMap::new(),
            schedules: HashMap::new(),
            listeners: HashMap::new(),
            windows: HashMap::new(),
            db_now,
        }
    }
}

fn keyed<T>(rows: Vec<T>, key: impl Fn(&T) -> Key) -> HashMap<Key, T> {
    rows.into_iter().map(|r| (key(&r), r)).collect()
}

/// The four state tables for `flows`, one statement each, and `now()`. The
/// caller runs it in one transaction, so `now()` is the transaction's.
pub(crate) async fn read_state(conn: &mut PgConnection, flows: &[Uuid]) -> Result<StateRows> {
    let db_now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
    let switches: Vec<Switch> = sqlx::query_as(
        "SELECT d.flow_id, d.trigger_id, d.disabled_by, \
                COALESCE(NULLIF(u.name, ''), u.email::text) AS disabled_by_name, d.disabled_at \
           FROM workflow.disabled_triggers d LEFT JOIN users u ON u.id = d.disabled_by \
          WHERE d.flow_id = ANY($1)",
    )
    .bind(flows)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let schedules: Vec<ScheduleRow> = sqlx::query_as(
        "SELECT flow_id, trigger_id, next_evaluation_date, evaluated_at, last_error \
           FROM workflow.triggers WHERE flow_id = ANY($1)",
    )
    .bind(flows)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let listeners: Vec<ListenerRow> = sqlx::query_as(
        "SELECT flow_id, trigger_id FROM workflow.flow_listeners WHERE flow_id = ANY($1)",
    )
    .bind(flows)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let windows: Vec<WindowRow> = sqlx::query_as(
        "SELECT flow_id, trigger_id, window_start, window_end, results \
           FROM workflow.flow_trigger_windows WHERE flow_id = ANY($1)",
    )
    .bind(flows)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(StateRows {
        switches: keyed(switches, |r| (r.flow_id, r.trigger_id.clone())),
        schedules: keyed(schedules, |r| (r.flow_id, r.trigger_id.clone())),
        listeners: keyed(listeners, |r| (r.flow_id, r.trigger_id.clone())),
        windows: keyed(windows, |r| (r.flow_id, r.trigger_id.clone())),
        db_now,
    })
}

/// [`LAST_EXECUTION_SQL`] for `keys`: one probe per pair. A pair with no
/// triggered execution is absent from the map.
pub(crate) async fn last_executions(
    conn: &mut PgConnection,
    keys: &[Key],
) -> Result<HashMap<Key, LastExecutionDto>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let flows: Vec<Uuid> = keys.iter().map(|(f, _)| *f).collect();
    let ids: Vec<String> = keys.iter().map(|(_, t)| t.clone()).collect();
    let rows: Vec<(Uuid, String, Uuid, String, DateTime<Utc>)> = sqlx::query_as(LAST_EXECUTION_SQL)
        .bind(&flows)
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|(flow, trigger, id, state, created_at)| {
            (
                (flow, trigger),
                LastExecutionDto {
                    id,
                    state,
                    created_at,
                },
            )
        })
        .collect())
}
