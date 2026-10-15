//! `workflow.trigger_backfills`: the row a schedule backfill lives in (W7f
//! spec §3.1), and its reads.
//!
//! **Lock order** (§3.11): every writer of a backfill row holds the flow row
//! (or, for a pass, its claim on the trigger), then the trigger's
//! `workflow.triggers` row, then the backfill row. A pass reads the row with
//! a plain statement after its claim and writes it in the same transaction,
//! so it takes the backfill row's lock after the trigger row's, as every
//! other writer does.
//!
//! Rows are read by `(flow_id, trigger_id)`, the primary key: the flow id
//! comes from a row already scoped by its flow (the claim's join, a route's
//! flow lookup), never from `trigger_backfills.workspace_id`, which is a
//! copy for filtering and not an authority.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

/// A backfill in progress, as the scheduler reads it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct BackfillRow {
    pub id: Uuid,
    pub flow_id: Uuid,
    pub trigger_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub current_tick: DateTime<Utc>,
    pub paused: bool,
    pub inputs: serde_json::Value,
    pub labels: serde_json::Value,
    pub previous_next_evaluation_date: DateTime<Utc>,
    pub total_ticks: i32,
    pub ticks_done: i32,
}

/// The columns of [`BackfillRow`], as a literal so `concat!` can build
/// static SQL from it (`repo::flow_cols!`'s pattern; sqlx 0.9 takes
/// `&'static str`).
macro_rules! backfill_cols {
    () => {
        "id, flow_id, trigger_id, window_start, window_end, current_tick, \
         paused, inputs, labels, previous_next_evaluation_date, total_ticks, ticks_done"
    };
}

// Read by Tasks 5-9 (sync and the routes select the same columns).
#[allow(dead_code)]
pub(crate) const BACKFILL_COLS: &str = backfill_cols!();

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// A plain read (a fresh statement: under READ COMMITTED, the newest
/// committed row).
pub(crate) async fn read(
    conn: &mut PgConnection,
    flow: Uuid,
    trigger: &str,
) -> Result<Option<BackfillRow>> {
    sqlx::query_as(concat!(
        "SELECT ",
        backfill_cols!(),
        " FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = $2"
    ))
    .bind(flow)
    .bind(trigger)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)
}

/// The same, `FOR UPDATE` (the routes and `sync`, after the trigger row).
// Its callers land in Tasks 5-7.
#[allow(dead_code)]
pub(crate) async fn read_for_update(
    conn: &mut PgConnection,
    flow: Uuid,
    trigger: &str,
) -> Result<Option<BackfillRow>> {
    sqlx::query_as(concat!(
        "SELECT ",
        backfill_cols!(),
        " FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE"
    ))
    .bind(flow)
    .bind(trigger)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)
}

/// Every backfill row of `flow`, `FOR UPDATE`, in `trigger_id` order
/// (`sync`, after the flow's trigger rows: the lock order of §3.11).
pub(crate) async fn read_flow_for_update(
    conn: &mut PgConnection,
    flow: Uuid,
) -> Result<Vec<BackfillRow>> {
    sqlx::query_as(concat!(
        "SELECT ",
        backfill_cols!(),
        " FROM workflow.trigger_backfills WHERE flow_id = $1 ORDER BY trigger_id FOR UPDATE"
    ))
    .bind(flow)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

impl BackfillRow {
    /// The pure model's window; `paused: false` (a paused row is never
    /// ticked: the claim and `fire_one`'s fresh read exclude it).
    pub fn window(&self) -> crate::trigger::Backfill {
        crate::trigger::Backfill {
            start: self.window_start,
            end: self.window_end,
            current_date: self.current_tick,
            paused: false,
        }
    }
}
