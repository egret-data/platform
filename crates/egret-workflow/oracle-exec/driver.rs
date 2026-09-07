//! **The real driver behind the ratchet.** W3b Task 5.
//!
//! Until this file, `tests/executor.rs` drove `advance()` through a harness of
//! test code: four tiers that fed terminal states out of `golden.json`,
//! rendered expressions themselves from `corpus.json` plus the flow source, and
//! never spawned anything. `oracle-exec/README.md` published what that cost —
//! 39 entries differing on nothing but `spawned`, 227 of 1104 kept ordered
//! pairs never asserted, and one field the oracle was circular for.
//!
//! This module drives the **shipping** worker instead: `worker::start_execution`
//! writes an execution to Postgres, `worker::tick` claims its runs and hands
//! each to the registered handler, `worker::render` fills the render slots
//! inside the same transaction, and `worker::spawn` runs the loop iterations.
//! Nothing here reads `golden.json`.
//!
//! # What this driver still supplies, in full
//!
//! Three things, and they are the whole list. Each is a property of the
//! *environment* a worker runs in, not of the execution's semantics, and none
//! of them is read from the golden.
//!
//! 1. **The inputs.** `corpus.json` records what `harvest.py` POSTed when it
//!    captured each golden entry, and they are posted here for the same flow.
//!    A golden captured with `param: true` is a trace of the `then` branch, so
//!    a driver that could not post them would be comparing a different
//!    execution. This is the same provenance the harness used and is unchanged
//!    by Task 5.
//! 2. **The passage of time.** A `ScheduleRetry` writes a `not_before` and the
//!    worker will not reopen the run until the execution's clock passes it.
//!    Rather than sleep, [`pass_time`] moves `executions.created_at` back — the
//!    origin the `Tick` is measured from — by exactly the shortfall. That is
//!    the harness's tier 4 ("time never moves while anything else can happen")
//!    against a real column, and it is a *smaller* intervention than the
//!    harness's: it changes one timestamp and calls the same `resume` any other
//!    caller would.
//! 3. **The wall-clock cost of `core.flow.Sleep`.** The registry is built with
//!    `Sleep::capped(ZERO)`, which validates the duration exactly as the
//!    product handler does and then does not wait. Four corpus flows sleep for
//!    30 s or more and one for `PT10M`. Nothing the oracle compares can see
//!    this: `normalise.py` drops every clock from the golden, and egret's
//!    happens-before endpoints are the logical `executions.seq`, not instants.
//!
//! **What is no longer supplied is the thing that mattered:** a run's terminal
//! state. It comes from a handler that ran.
//!
//! # A stall is a wrong answer here, not a broken instrument
//!
//! The harness completed every `Runnable` from the golden, so an execution that
//! could not finish meant the state machine was spinning, and it was reported
//! as `Broken` — an unbaselineable failure. With real handlers that is no
//! longer true: a flow whose task type has no handler *stalls*, because
//! `Registry::run` refuses an unknown type rather than inventing a state for
//! it, and stalling is what the shipping worker really does. So this driver
//! returns the **partial trace** when nothing can move, and only a run that
//! exceeds [`STEP_BOUND`] or panics is `Broken`.
//! `every_mismatching_entry_is_attributed_to_the_field_it_differs_on` in
//! `tests/executor.rs` counts and names them, which keeps
//! that population from growing silently.

use std::time::Duration;

use egret_workflow::worker::{self, Handlers};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{step_relation, Run, Trace};

/// How many driver steps one entry may take before the run is declared broken.
///
/// A step is one `tick` that ran something, or one `pass_time` that moved the
/// clock. Each ran-tick finishes exactly one task run, and each clock move
/// strictly increases the execution's own `now` to a distinct `not_before` that
/// a `ScheduleRetry` wrote. So the bound is "task runs the trace will hold,
/// plus retries", and the widest golden entry holds 765 history entries across
/// its execution, its runs, their attempts and its sub-executions
/// (`valids/loop-nested`, measured over the whole golden by
/// `the_step_bound_dominates_the_widest_golden_entry`). Four times that is the
/// same headroom the harness's bound carried, for the same reason: the parts
/// are loose rather than tight.
pub const STEP_BOUND: usize = 4 * 765;

/// **One run per tick, and that is the interleaving the golden was normalised
/// against.**
///
/// The harness's tier 3 completed exactly one task per step — the
/// earliest-created still in flight — so exactly one interleaving was
/// reachable. A batch of `n` here would let two siblings of one `Parallel`
/// finish inside one round and would make which of them created its successor
/// first depend on the batch size. It also bounds the blast radius of a worker
/// error: `Registry::run` refusing an unknown type aborts the tick, and with a
/// batch the runs claimed beside it would keep a lease nothing would ever
/// release.
const BATCH: usize = 1;

/// The registry the ratchet judges: the four handlers W3b ships, with `Sleep`
/// capped — see the module doc, item 3.
pub fn registry() -> Handlers {
    let mut r = Handlers::new();
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::log::Log::new(),
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::ret::Return,
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::fail::Fail::new(),
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::sleep::Sleep::capped(Duration::ZERO),
    ));
    r
}

/// What one entry's run produced.
pub enum Ran {
    /// The trace the driver read back out of Postgres.
    Trace(Box<Trace>),
    /// `build_plan` refused the flow. The one thing a `refused` golden entry
    /// compares.
    Refused(String),
    /// The instrument failed: the step bound was exceeded, or a database error
    /// nothing about the flow explains.
    Broken(String),
}

/// Start `source` as a real execution and drive it to a standstill.
///
/// `inputs` is what `harvest.py` posted for this flow — see the module doc.
pub async fn run(
    pool: &PgPool,
    workspace: Uuid,
    reg: &Handlers,
    source: &str,
    inputs: serde_json::Value,
) -> Ran {
    // `build_plan` first, and separately, so that a flow egret refuses is
    // reported as a refusal rather than as a driver failure. It is the same
    // call `start_execution` makes; making it here costs one plan build and
    // buys the distinction the `refused` verdict rests on.
    if let Err(e) = egret_workflow::executor::build_plan(source) {
        return Ran::Refused(e.message);
    }
    let id = match worker::start_execution(pool, workspace, None, source, inputs).await {
        Ok(id) => id,
        Err(e) => return Ran::Broken(format!("start_execution: {e}")),
    };

    for step in 0..=STEP_BOUND {
        if step == STEP_BOUND {
            return Ran::Broken(format!(
                "did not settle within {STEP_BOUND} steps; execution {id}"
            ));
        }
        // A worker error is NOT a driver error. `Registry::run` refuses a type
        // it has no handler for, which leaves that one run leased and unmoved —
        // exactly what the shipping worker does — and the driver carries on so
        // that the rest of the execution still produces the trace it produces.
        // With `BATCH == 1` the refusal can only ever concern the run it names.
        //
        // **`WorkerError::Corrupt` is the exception, and it used to be swallowed
        // with everything else.** It means a stored row cannot be read back into
        // what `advance()` reads — a `task_ix` the plan does not hold, an
        // `attempts` entry with no `started_at` — which is the DRIVER's own
        // storage being wrong rather than the flow diverging. That is the class
        // of the one product defect this package found, and under `Err(_) =>
        // continue` it could only ever surface as a stalled execution, i.e. as
        // an ordinary mismatch, and never as `Broken`. `Broken` is the verdict
        // `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`
        // asserts at 0.
        match worker::tick(pool, BATCH, reg).await {
            Ok(0) => {}
            Ok(_) => continue,
            Err(e) => match corrupt(&e) {
                true => return Ran::Broken(format!("tick: {e}")),
                false => continue,
            },
        }
        match pass_time(pool, id).await {
            Ok(true) => continue,
            Ok(false) => break,
            Err(e) => return Ran::Broken(format!("pass_time: {e}")),
        }
    }

    match read_trace(pool, id).await {
        Ok(t) => Ran::Trace(Box::new(t)),
        Err(e) => Ran::Broken(format!("read_trace: {e}")),
    }
}

/// Whether a worker error is the storage being wrong rather than the flow
/// diverging.
///
/// Downcast rather than matched on the message: `WorkerError` reaches here
/// inside `egret_core::Error::Internal(anyhow::Error)`, and `anyhow!` over a
/// `std::error::Error` preserves the concrete type, so the classification
/// cannot drift when somebody rewords the `#[error(...)]` string.
fn corrupt(e: &egret_core::Error) -> bool {
    let egret_core::Error::Internal(a) = e else {
        return false;
    };
    matches!(
        a.downcast_ref::<egret_workflow::worker::WorkerError>(),
        Some(egret_workflow::worker::WorkerError::Corrupt(..))
    )
}

/// The harness's tier 4, against a real clock.
///
/// Nothing is claimable and nothing rendered, so the only thing that can still
/// move the execution is a deadline: a run whose `not_before_ms` is ahead of
/// the execution's own `now`. `Tick` is milliseconds since
/// `executions.created_at`, so moving that column **back** by the shortfall is
/// exactly "time passed", and it is the whole intervention — the state, the
/// histories and the retry policy are untouched, and `worker::resume` then does
/// the same work it would have done had the driver waited.
///
/// Time moves to the **earliest** deadline still ahead, over the whole tree,
/// and never further: a `timeout` that fired early would be a different
/// execution.
///
/// Returns whether anything moved.
async fn pass_time(pool: &PgPool, root: Uuid) -> Result<bool, sqlx::Error> {
    let due: Option<(Uuid, i64)> = sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
             SELECT id, created_at FROM workflow.executions WHERE id = $1 \
             UNION ALL \
             SELECT e.id, e.created_at FROM workflow.executions e \
               JOIN tree t ON e.parent_execution_id = t.id \
         ) \
         SELECT t.id, MIN(r.not_before_ms) - \
                MIN((EXTRACT(EPOCH FROM (now() - t.created_at)) * 1000)::bigint) AS shortfall \
           FROM tree t JOIN workflow.task_runs r ON r.execution_id = t.id \
          WHERE r.not_before_ms IS NOT NULL \
            AND r.not_before_ms > (EXTRACT(EPOCH FROM (now() - t.created_at)) * 1000)::bigint \
          GROUP BY t.id \
          ORDER BY shortfall \
          LIMIT 1",
    )
    .bind(root)
    .fetch_optional(pool)
    .await?;

    let Some((execution, shortfall)) = due else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE workflow.executions \
            SET created_at = created_at - make_interval(secs => $1::double precision / 1000.0) \
          WHERE id = $2",
    )
    // One millisecond past the deadline rather than exactly on it: `Tick` is
    // integral milliseconds and `reopen_due`'s predicate is `not_before <= now`,
    // so landing exactly on the boundary would depend on a rounding this driver
    // does not control.
    .bind(shortfall + 1)
    .bind(execution)
    .execute(pool)
    .await?;
    worker::resume(pool, execution)
        .await
        .map_err(|e| sqlx::Error::Protocol(format!("resume: {e}")))?;
    Ok(true)
}

/// Read one execution and everything under it back out of Postgres, in the
/// shape the golden is compared in.
///
/// # `spawned` is ordered the way `normalise.py` orders it
///
/// `(parent run ordinal, loop index)` — Rule 5's "by the parent run", with the
/// loop index as the tiebreak, because the creation instant carries no
/// information at all: every sub-execution of one batch is written in one
/// transaction and shares `now()`, and on Kestra's side all 670 loop
/// sub-executions carry their parent execution's `startDate`.
async fn read_trace(pool: &PgPool, id: Uuid) -> Result<Trace, sqlx::Error> {
    Box::pin(read_trace_inner(pool, id, None, None)).await
}

async fn read_trace_inner(
    pool: &PgPool,
    id: Uuid,
    parent_run: Option<usize>,
    value: Option<String>,
) -> Result<Trace, sqlx::Error> {
    let history: serde_json::Value =
        sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;

    let rows = sqlx::query(
        "SELECT task_id, value, parent_ix, history, attempts, created_seq, ended_seq \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let mut runs = Vec::with_capacity(rows.len());
    let mut created: Vec<Option<usize>> = Vec::with_capacity(rows.len());
    let mut ended: Vec<Option<usize>> = Vec::with_capacity(rows.len());
    for r in &rows {
        runs.push(Run {
            task: r.get("task_id"),
            value: r.get("value"),
            parent: r.get::<Option<i32>, _>("parent_ix").map(|p| p as usize),
            state: strings(&r.get::<serde_json::Value, _>("history")),
            attempts: r
                .get::<serde_json::Value, _>("attempts")
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| strings(x.get("history").unwrap_or(&serde_json::Value::Null)))
                        .collect()
                })
                .unwrap_or_default(),
        });
        created.push(r.get::<Option<i64>, _>("created_seq").map(|v| v as usize));
        ended.push(r.get::<Option<i64>, _>("ended_seq").map(|v| v as usize));
    }

    let mut order = Vec::new();
    for i in 0..runs.len() {
        for j in (i + 1)..runs.len() {
            if let Some(rel) = step_relation(created[i], ended[i], created[j], ended[j]) {
                order.push((i, j, rel.to_string()));
            }
        }
    }

    let children: Vec<(Uuid, i32, String)> = sqlx::query_as(
        "SELECT id, parent_run_ix, loop_value FROM workflow.executions \
          WHERE parent_execution_id = $1 ORDER BY parent_run_ix, loop_index",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let mut spawned = Vec::with_capacity(children.len());
    for (child, run_ix, loop_value) in children {
        spawned.push(
            Box::pin(read_trace_inner(
                pool,
                child,
                Some(run_ix.max(0) as usize),
                Some(loop_value),
            ))
            .await?,
        );
    }

    Ok(Trace {
        parent_run,
        value,
        execution: strings(&history),
        runs,
        order,
        spawned,
    })
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
