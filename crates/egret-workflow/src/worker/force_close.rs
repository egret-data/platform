//! **Force-close an execution a kill cannot load** -- workflow debt spec
//! `docs/en/specs/2026-10-19-workflow-debt-design.md` §2.2 (decision D4).
//!
//! # When
//!
//! `claim::load` fails with a row-content fault (`WorkerError::is_row_content`:
//! `Unplannable`, `NotALeaf`, `Corrupt`, `UnknownState`) at one of three
//! sites:
//! - the route's `kill_in` (`worker::kill`);
//! - the cascade's `descendants` (`worker::kill`), for one descendant;
//! - `commit_kill_with` (`worker::claim`).
//!
//! Any other error stays transient, as `finish_claimed_kill` already rules.
//!
//! # What it writes
//!
//! [`close`] is plain SQL and never plans the source:
//! 1. It locks the execution row (`FOR NO KEY UPDATE`, as `load`), then its
//!    run rows `FOR UPDATE` in `run_ix` order, as `kill_loaded` does.
//! 2. A live run (`NOT workflow.state_is_terminal(state)`) that a worker holds
//!    is set to `KILLING`, and keeps its lease. "Holds" means `RUNNING`,
//!    `SUBMITTED` or `KILLING` with `claimed_at IS NOT NULL`, and not the
//!    caller's own `own` run. That worker has the handler's input, so it can
//!    cancel; its `commit_kill` then reaches this function again with `own`
//!    set.
//! 3. Every other live run becomes `KILLED`:
//!    - `KILLED` is appended to `history`;
//!    - `ended_at = now()` and `claimed_at = NULL`;
//!    - `message = reason` when no message is set;
//!    - the last attempt's `history` gets `KILLED` when its last state is not
//!      terminal.
//!
//!    An unleased running run gets no cancel: nothing can build its handler
//!    (the fix round 1 rule, `claim::finish_claimed_kill`). The orphan sweep
//!    owns its container, now including a kept one (§2.1).
//! 4. The execution becomes `KILLED`, with `ended_at`, when no run is left
//!    `KILLING`. Otherwise it becomes `KILLING`.
//! 5. A `KILLED` execution reports to its parent through the same calls
//!    `settle` makes (`spawn::on_execution_terminal`,
//!    `subflow::on_child_terminal`). They load the parent, not this
//!    execution, so a waiting loop or `Subflow` run ends.
//! 6. No Flow-trigger outbox event is written, and no `errors:` or `finally:`
//!    runs: there is no plan to run them from. The message says so.
//!
//! **Rule 2 holds a LEAF only** (fix round 1). A `WorkingDirectory` block's
//! claim (`type_id IS NULL`) is never killed (`claim::Watch::Block`), so
//! nobody would cancel it, and a `KILLING` block is claimable only in
//! `FINISHING` (`claim::claim`): one left `KILLING` in `PREPARING` would
//! keep the execution `KILLING` for ever. A leased block is closed `KILLED`
//! with the rest; its worker's commit then fails once (the load) and the
//! row, terminal, is never claimed again; what its claim wrote under `_wd/`
//! is the crash sweep's.
//!
//! # How the timestamps are written
//!
//! Every state write goes through `stamps::stamp_set!`, as every other
//! writer's does (`tests/execution_timestamps.rs`'s lexical guard): a run
//! entering `KILLED` here is a non-terminal-to-terminal transition, so its
//! `ended_at` is `GREATEST(now(), started_at)`, and a run moved to `KILLING`
//! enters nothing.
//!
//! **The attempt's `ended_at` is written as `stamps::stamp_attempt_ends`
//! writes it** (measured for debt Task 1.4): the jsonb key `ended_at` on the
//! attempt object, an integer `Tick` -- milliseconds after the EXECUTION's
//! `created_at`, the clock of the attempt's own `started_at` -- and
//! `max(now, started_at)`. `now` is `load`'s `now_ms` expression,
//! `(EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint`, read off the
//! execution row this function locks. An attempt that already has an end
//! keeps it.
//!
//! The round clock moves as a settle's does: each run closed here gets
//! `ended_seq = seq + 1`, and the execution's `seq` becomes that step.
//!
//! # The parent's report is deferred
//!
//! [`Closed::reported`] holds a `claim::Job::Report`, which the caller runs
//! the way it runs its frames, and which loads the parent only when it runs.
//! In a cascade the parent may be a descendant whose own kill is still a
//! frame waiting to settle: a report that loaded it now would settle a state
//! that frame then overwrites.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::claim::{self, Caller, Job};
use super::stamps::stamp_set;
use super::{from_wire, WorkerError};
use crate::executor::RunState;

/// What [`close`] did.
pub(super) struct Closed {
    /// The execution's state afterwards: `Killing` while a held run is left
    /// to its worker (rule 2), `Killed` otherwise. An execution that was
    /// already terminal under its lock is left alone and answers that
    /// terminal.
    pub state: RunState,
    /// The report to the parent of an execution closed `KILLED` (rule 5),
    /// for the caller to run.
    pub reported: Vec<Job>,
    /// Whether the caller's `own` run, fenced on its lease, was closed
    /// `KILLED` here: `commit_kill`'s answer.
    pub own: bool,
}

/// **The message stored on every run [`close`] closes** (spec §2.2). egret's
/// own: Kestra has no kill of an execution it cannot load.
pub(super) fn message(reason: &str) -> String {
    format!(
        "egret could not load this execution ({reason}), so it was killed without running its \
         flow: no errors:, finally: or Flow trigger ran"
    )
}

/// The `<reason>` of [`message`]: the `WorkerError`'s text, when `e` is one.
pub(super) fn reason(e: &Error) -> String {
    match e {
        Error::Internal(a) => a
            .downcast_ref::<WorkerError>()
            .map_or_else(|| a.to_string(), ToString::to_string),
        other => other.to_string(),
    }
}

/// One run row, read under its lock.
struct Run {
    id: Uuid,
    state: String,
    history: serde_json::Value,
    attempts: serde_json::Value,
    claimed_at: Option<DateTime<Utc>>,
    live: bool,
    /// A leaf (`type_id IS NOT NULL`), not a `WorkingDirectory` block.
    leaf: bool,
}

/// A run's write: `$1` the state, `$2` the history, `$3` the attempts, `$4`
/// whether the lease is released, `$5` the message for a row with none, `$6`
/// the round it ended in, `$7`/`$8` `stamps::Entered`'s booleans, `$9` the
/// run.
const RUN_CLOSE: &str = concat!(
    "UPDATE workflow.task_runs \
        SET state = $1, history = $2, attempts = $3, \
            claimed_at = CASE WHEN $4 THEN NULL ELSE claimed_at END, \
            message = COALESCE(message, $5), \
            ended_seq = COALESCE($6, ended_seq), ",
    stamp_set!("$1", "$2", "$7", "$8"),
    ", updated_at = now() WHERE id = $9",
);

/// **Close `execution` without planning it** (module doc, rules 1-6).
/// `reason` is the load's error text ([`reason`]); `own` is the caller's own
/// run and the lease it holds, `None` for the route. A load nested in the
/// parent's report sits inside `caller`'s frame.
pub(super) async fn close(
    conn: &mut PgConnection,
    execution: Uuid,
    reason: &str,
    own: Option<(Uuid, DateTime<Utc>)>,
    caller: Caller,
) -> Result<Closed> {
    // Rule 1: the execution's lock, then the runs' in `run_ix` order.
    let row = sqlx::query(
        "SELECT state, history, seq, \
                (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint AS now_ms \
           FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE",
    )
    .bind(execution)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("execution {execution}")))?;
    let state: String = row.get("state");
    if let Some(terminal) = from_wire(&state).filter(|s| s.is_terminal()) {
        return Ok(Closed {
            state: terminal,
            reported: Vec::new(),
            own: false,
        });
    }
    let history: serde_json::Value = row.get("history");
    let step: i64 = row.get::<i64, _>("seq") + 1;
    let now_ms: i64 = row.get("now_ms");

    let runs: Vec<Run> = sqlx::query(
        "SELECT id, state, history, attempts, claimed_at, \
                NOT workflow.state_is_terminal(state) AS live, \
                type_id IS NOT NULL AS leaf \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix FOR UPDATE",
    )
    .bind(execution)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(|r| Run {
        id: r.get("id"),
        state: r.get("state"),
        history: r.get("history"),
        attempts: r.get("attempts"),
        claimed_at: r.get("claimed_at"),
        live: r.get("live"),
        leaf: r.get("leaf"),
    })
    .collect();

    let text = message(reason);
    let mut killing = false;
    let mut own_closed = false;
    for run in runs.iter().filter(|r| r.live) {
        let is_own = own.is_some_and(|(id, at)| id == run.id && run.claimed_at == Some(at));
        let held = !is_own
            && run.leaf
            && run.claimed_at.is_some()
            && matches!(run.state.as_str(), "RUNNING" | "SUBMITTED" | "KILLING");
        if held {
            // Rule 2: left to its worker, under its lease.
            killing = true;
            if run.state == "KILLING" {
                continue;
            }
            sqlx::query(RUN_CLOSE)
                .bind(RunState::Killing.wire())
                .bind(appended(&run.history, RunState::Killing))
                .bind(&run.attempts)
                .bind(false)
                .bind(None::<String>)
                .bind(None::<i64>)
                .bind(false)
                .bind(false)
                .bind(run.id)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?;
            continue;
        }
        // Rule 3.
        sqlx::query(RUN_CLOSE)
            .bind(RunState::Killed.wire())
            .bind(appended(&run.history, RunState::Killed))
            .bind(closed_attempts(&run.attempts, now_ms))
            .bind(true)
            .bind(Some(text.as_str()))
            .bind(Some(step))
            .bind(false)
            .bind(true)
            .bind(run.id)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        own_closed |= is_own;
    }

    // Rule 4.
    let mut after = history.as_array().cloned().unwrap_or_default();
    if after.last().and_then(|s| s.as_str()) != Some(RunState::Killing.wire()) {
        after.push(RunState::Killing.wire().into());
    }
    let ended = if killing {
        RunState::Killing
    } else {
        after.push(RunState::Killed.wire().into());
        RunState::Killed
    };
    sqlx::query("UPDATE workflow.executions SET seq = $1 WHERE id = $2")
        .bind(step)
        .bind(execution)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    let after = serde_json::Value::Array(after);
    if after != history {
        sqlx::query(claim::EXECUTION_STATE_UPDATE)
            .bind(ended.wire())
            .bind(&after)
            .bind(execution)
            .bind(false)
            .bind(ended == RunState::Killed)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
    }

    // Rules 5 and 6: the parent hears of a `KILLED`; nothing else runs.
    let reported = if ended == RunState::Killed {
        vec![Job::report(execution, RunState::Killed, caller)]
    } else {
        Vec::new()
    };
    Ok(Closed {
        state: ended,
        reported,
        own: own_closed,
    })
}

/// A stored history with `state` appended. A history that is not a list (a
/// `Corrupt` row) is replaced by `[state]`: the row is being closed, and a
/// list is what every reader expects.
fn appended(history: &serde_json::Value, state: RunState) -> serde_json::Value {
    let mut h = history.as_array().cloned().unwrap_or_default();
    h.push(state.wire().into());
    serde_json::Value::Array(h)
}

/// The stored attempts with the last one closed `KILLED` at `now_ms`, when
/// its last state is not terminal (module doc, "How the timestamps are
/// written"). Anything that is not the expected shape is left as it is.
fn closed_attempts(attempts: &serde_json::Value, now_ms: i64) -> serde_json::Value {
    let mut attempts = attempts.clone();
    let Some(last) = attempts
        .as_array_mut()
        .and_then(|a| a.last_mut())
        .and_then(|a| a.as_object_mut())
    else {
        return attempts;
    };
    let Some(history) = last.get_mut("history").and_then(|h| h.as_array_mut()) else {
        return attempts;
    };
    let ended = history
        .last()
        .and_then(|s| s.as_str())
        .and_then(from_wire)
        .is_some_and(|s| s.is_terminal());
    if ended {
        return attempts;
    }
    history.push(RunState::Killed.wire().into());
    if last.get("ended_at").is_none_or(|e| e.is_null()) {
        let started = last
            .get("started_at")
            .and_then(|t| t.as_i64())
            .unwrap_or(now_ms);
        last.insert("ended_at".into(), now_ms.max(started).into());
    }
    attempts
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The attempt rule, beside each control.
    #[test]
    fn the_last_open_attempt_is_closed_killed_on_the_attempt_clock() {
        // An open attempt: closed, and stamped at max(now, start).
        assert_eq!(
            closed_attempts(
                &json!([{"history": ["CREATED", "RUNNING"], "started_at": 10}]),
                50
            ),
            json!([{"history": ["CREATED", "RUNNING", "KILLED"], "started_at": 10, "ended_at": 50}])
        );
        assert_eq!(
            closed_attempts(&json!([{"history": ["RUNNING"], "started_at": 90}]), 50),
            json!([{"history": ["RUNNING", "KILLED"], "started_at": 90, "ended_at": 90}])
        );
        // A terminal last attempt, and no attempt: unchanged.
        let done = json!([{"history": ["RUNNING", "FAILED"], "started_at": 1, "ended_at": 2}]);
        assert_eq!(closed_attempts(&done, 50), done);
        assert_eq!(closed_attempts(&json!([]), 50), json!([]));
    }

    #[test]
    fn the_message_spells_the_spec() {
        assert_eq!(
            message("x"),
            "egret could not load this execution (x), so it was killed without running its \
             flow: no errors:, finally: or Flow trigger ran"
        );
    }
}
