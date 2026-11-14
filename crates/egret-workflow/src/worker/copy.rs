//! **Replay into a new execution** -- workflow package 7b (spec
//! `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md` D1-D10;
//! Kestra `ExecutionService.replay`, `ES:393-512`, and `.restart` with a
//! revision, `ES:281-373`).
//!
//! Three kinds ([`ReplayKind`]):
//!
//! - **Whole**: a fresh execution of the chosen source, started as any start
//!   is (`claim::insert_execution`, then its first settle), with the
//!   `system.replay` label and the old `attempt_number + 1` (D10).
//! - **From a task run** and **restart on a source**: a copy. The executor
//!   says which old runs are kept and which of those are reset
//!   (`executor::replay_from`, `executor::restart_copy`) and maps them onto
//!   the chosen source's plan (`executor::copy_onto`). The new row is
//!   inserted, the kept runs are copied into it ([`insert_runs`]), and the
//!   reset is run as 7a's restart job on the new execution
//!   (`restart::copy_job`), which adds the label and the attempt.
//!
//! One transaction (`worker::txn`), the old execution locked first, then the
//! new one. **Every refusal comes before the first write**: `transaction!`
//! commits an `Ok`. A deadlock's victim is run again, as 7a's restart is.

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::claim::{self, NewExecution, Stack};
use super::kill::{is_deadlock, DEADLOCK_RETRIES};
use super::restart::{self, CopyOp, REPLAY_LABEL_KEY, REPLAY_LABEL_VALUE};
use super::txn::{transaction, Gate, InTxn};
use crate::executor::{CopiedRuns, CopyRefusal, CopySelection, MatchFault, Plan, TaskKind};

/// What [`replay_execution`] is asked to do. The route has scoped `from` to
/// the caller's workspace and chosen `source` (D6, S6).
pub struct ReplayRequest<'a> {
    pub workspace_id: Uuid,
    /// The execution replayed or restarted; the route scoped it.
    pub from: Uuid,
    /// The source the new execution runs and freezes, chosen by the route (D6, S6).
    pub source: &'a str,
    pub kind: ReplayKind,
    pub inputs: ReplayInputs,
    /// The person (D8: `check_start`'s door 2 judges the caller).
    pub started_by: Uuid,
    /// Chosen first: a multipart body's files are stored under it.
    pub id: Uuid,
    pub input_files: Vec<crate::storage::StoredFile>,
}

/// Which replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayKind {
    /// The whole execution, from the start (today's replay, plus D10).
    Whole,
    /// From this task run of the replayed execution (D2).
    FromTaskRun(Uuid),
    /// 7a's restart, into a new execution on the chosen source (D3).
    RestartOnSource,
}

/// The new execution's inputs (D7).
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayInputs {
    /// The replayed execution's own, SECRET values revealed (KI-112).
    Reuse,
    /// New values, checked against the chosen source's `inputs:`.
    Replace(serde_json::Value),
}

/// Why a replay wrote nothing. The route maps each to its coded answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayRefused {
    /// The execution is an iteration of a `Loop`; replay its root.
    LoopIteration {
        root: Uuid,
    },
    /// Replay: the execution has not ended; its state.
    NotTerminal(String),
    /// Restart on a source: 7a's refusals.
    NotRestartable(String),
    NothingToRestart,
    FilesPruned,
    AfterExecutionRunning,
    /// No run of this execution has that id, or the run with it was emptied.
    TaskRunNotFound,
    /// The chosen run has a `WorkingDirectory` ancestor (D9).
    InsideWorkingDirectory,
    /// A copied run's task does not fit the chosen source (D5, S3).
    TaskNotInSource {
        task_id: String,
        why: MatchFault,
    },
}

/// **Spec D1-D10.** Unscoped by workspace, like every worker entry point. A
/// refusal writes nothing. A deadlock's victim is run again (7a's retry).
pub async fn replay_execution(
    pool: &PgPool,
    req: &ReplayRequest<'_>,
) -> Result<std::result::Result<Uuid, ReplayRefused>> {
    let mut retries = 0;
    loop {
        let replayed = transaction!(pool, |at| {
            let (conn, gate) = at.parts();
            replay_locked(conn, gate, req).await
        });
        match replayed {
            Err(e) if retries < DEADLOCK_RETRIES && is_deadlock(&e) => {
                retries += 1;
                tracing::info!(execution = %req.from, "a replay was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

/// The replayed execution's row, read under its lock.
struct Old {
    loop_root: Option<Uuid>,
    files_pruned: bool,
    idle: bool,
    labels: Option<serde_json::Value>,
    attempt_number: i32,
    seq: i64,
    inputs: serde_json::Value,
    secret_inputs: Option<Vec<u8>>,
    variables: Option<serde_json::Value>,
    flow_id: Option<Uuid>,
}

/// One row: the facts the checks read (the files of the whole replay chain,
/// S7) and what the new row copies.
const OLD_ROW: &str = "\
WITH RECURSIVE chain(id) AS ( \
  SELECT $1::uuid \
  UNION \
  SELECT e.replayed_from_execution_id FROM workflow.executions e JOIN chain c ON e.id = c.id \
   WHERE e.replayed_from_execution_id IS NOT NULL) \
SELECT CASE WHEN x.loop_index IS NOT NULL THEN x.parent_execution_id END AS loop_root, \
       EXISTS (SELECT 1 FROM workflow.executions p JOIN chain c ON p.id = c.id \
                WHERE p.files_pruned_at IS NOT NULL) AS files_pruned, \
       COALESCE(workflow.execution_is_idle(x.id), false) AS idle, \
       x.labels, x.attempt_number, x.seq, x.inputs, x.secret_inputs, x.variables, x.flow_id \
  FROM workflow.executions x WHERE x.id = $1";

async fn replay_locked(
    conn: &mut PgConnection,
    gate: &Gate,
    req: &ReplayRequest<'_>,
) -> Result<std::result::Result<Uuid, ReplayRefused>> {
    // 1. The old execution and its runs, locked (`restart::lock`'s order).
    let (old_ctx, state, _) = claim::load(conn, req.from, Stack::Fresh(gate.clone())).await?;
    sqlx::query(
        "SELECT 1 FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix FOR UPDATE",
    )
    .bind(req.from)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    let row = sqlx::query(OLD_ROW)
        .bind(req.from)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
    let old = Old {
        loop_root: row.get("loop_root"),
        files_pruned: row.get("files_pruned"),
        idle: row.get("idle"),
        labels: row.get("labels"),
        attempt_number: row.get("attempt_number"),
        seq: row.get("seq"),
        inputs: row.get("inputs"),
        secret_inputs: row.get("secret_inputs"),
        variables: row.get("variables"),
        flow_id: row.get("flow_id"),
    };

    // 2. A loop iteration is replayed through its root.
    if let Some(root) = old.loop_root {
        return Ok(Err(ReplayRefused::LoopIteration { root }));
    }
    let last = state
        .history
        .last()
        .copied()
        .unwrap_or(crate::executor::RunState::Created);
    let terminal = last.is_terminal();
    // 3. A replay needs an ended execution.
    if matches!(req.kind, ReplayKind::Whole | ReplayKind::FromTaskRun(_)) && !terminal {
        return Ok(Err(ReplayRefused::NotTerminal(last.wire().to_string())));
    }
    // 4. S7: a copy carries the old runs' files and must not copy live runs.
    if req.kind != ReplayKind::Whole {
        if old.files_pruned {
            return Ok(Err(ReplayRefused::FilesPruned));
        }
        if terminal && !old.idle {
            return Ok(Err(ReplayRefused::AfterExecutionRunning));
        }
    }

    // 5. The selection.
    let selection = match select(conn, req, &old_ctx.plan, &state).await? {
        Ok(selection) => selection,
        Err(refused) => return Ok(Err(refused)),
    };
    // 6. The copy onto the chosen source's plan (D5, S3).
    let copied = match selection {
        None => None,
        Some(sel) => {
            let target = crate::executor::build_plan(req.source)
                .map_err(|e| Error::InvalidInput(format!("the flow does not plan: {e}")))?;
            match crate::executor::copy_onto(&old_ctx.plan, &target, &state, &sel) {
                Ok(copied) => Some((target, copied)),
                Err(refusal) => return Ok(Err(refused(refusal))),
            }
        }
    };

    // 7. Inputs (D7).
    let inputs = match &req.inputs {
        // KI-112: the stored SECRET values, not the mask the row shows; the
        // insert masks and seals them again for the new row.
        ReplayInputs::Reuse => {
            crate::secret_inputs::reveal(old.inputs.clone(), old.secret_inputs.as_deref())?.0
        }
        // The start's check against the CHOSEN source. The insert skips its
        // own check, since `replayed_from_execution_id` is set.
        ReplayInputs::Replace(v) => {
            if !v.is_object() {
                return Err(Error::InvalidInput("inputs must be a JSON object".into()));
            }
            let flow = crate::spec::parse(req.source).map_err(|e| {
                Error::InvalidInput(format!("the flow does not parse: {}", e.message))
            })?;
            claim::checked_inputs(&flow, v).map_err(|e| Error::InvalidInput(e.message))?
        }
    };

    // 8. Labels: a whole replay is labelled here; a copy by the restart job.
    let labels = match req.kind {
        ReplayKind::Whole => Some(restart::with_label(
            old.labels.as_ref(),
            REPLAY_LABEL_KEY,
            REPLAY_LABEL_VALUE,
        )),
        _ => old.labels.clone(),
    };

    // 9. The row: `check_start` judges the chosen source with the caller
    // (D8), and FILE inputs are checked.
    let ctx = claim::insert_execution(
        conn,
        gate,
        NewExecution {
            workspace_id: req.workspace_id,
            flow_id: old.flow_id,
            source: req.source,
            inputs,
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: Some(req.from),
            id: Some(req.id),
            input_files: req.input_files.clone(),
            labels,
            backfill_id: None,
            variables: old.variables.clone(),
            started_by: Some(req.started_by),
        },
    )
    .await?;

    let Some((target, mut copied)) = copied else {
        // 10. Whole: the old attempt plus one (D10), then the first settle.
        sqlx::query("UPDATE workflow.executions SET attempt_number = $2 WHERE id = $1")
            .bind(ctx.id)
            .bind(old.attempt_number + 1)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        claim::settle_new(conn, &ctx).await?;
        return Ok(Ok(ctx.id));
    };

    // 11. A copy. The old attempt and clock: the restart job's side writes
    // add 1 to the attempt, and the copied `created_seq`/`ended_seq` stay
    // below the clock.
    sqlx::query("UPDATE workflow.executions SET attempt_number = $2, seq = $3 WHERE id = $1")
        .bind(ctx.id)
        .bind(old.attempt_number)
        .bind(old.seq)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    // Ticks are measured from an execution's `created_at`: move them so a
    // kept attempt keeps its wall clock under the new row.
    let by: i64 = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM (o.created_at - n.created_at)) * 1000)::bigint \
           FROM workflow.executions o, workflow.executions n WHERE o.id = $1 AND n.id = $2",
    )
    .bind(req.from)
    .bind(ctx.id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    crate::executor::shift_ticks(&mut copied.runs, by);
    insert_runs(conn, ctx.id, req.from, &target, &copied).await?;
    let op = match req.kind {
        ReplayKind::RestartOnSource => CopyOp::Restart,
        _ => CopyOp::Replay,
    };
    let job =
        restart::copy_job(conn, ctx.id, Stack::Fresh(gate.clone()), &copied.reset, op).await?;
    claim::run_job(conn, job).await?;
    Ok(Ok(ctx.id))
}

/// The old runs a copy keeps and resets, or `None` for a whole replay; or
/// why the copy is refused.
async fn select(
    conn: &mut PgConnection,
    req: &ReplayRequest<'_>,
    plan: &Plan,
    state: &crate::executor::ExecutionState,
) -> Result<std::result::Result<Option<CopySelection>, ReplayRefused>> {
    let selection = match req.kind {
        ReplayKind::Whole => return Ok(Ok(None)),
        ReplayKind::FromTaskRun(task_run) => {
            // This execution's run only. `load` refuses a gap in `run_ix`,
            // so a run's `run_ix` is its index in `state.runs`.
            let run_ix: Option<i32> = sqlx::query_scalar(
                "SELECT run_ix FROM workflow.task_runs WHERE execution_id = $1 AND id = $2",
            )
            .bind(req.from)
            .bind(task_run)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?;
            let Some(ix) = run_ix.and_then(|ix| usize::try_from(ix).ok()) else {
                return Ok(Err(ReplayRefused::TaskRunNotFound));
            };
            crate::executor::replay_from(plan, state, ix)
        }
        ReplayKind::RestartOnSource => crate::executor::restart_copy(plan, state),
    };
    Ok(selection.map(Some).map_err(refused))
}

/// A pure refusal, as the worker answers it.
fn refused(refusal: CopyRefusal) -> ReplayRefused {
    match refusal {
        // An emptied run is not a run of this execution any more.
        CopyRefusal::NoSuchRun => ReplayRefused::TaskRunNotFound,
        CopyRefusal::NotRestartable(s) => ReplayRefused::NotRestartable(s.wire().to_string()),
        CopyRefusal::NothingToRestart => ReplayRefused::NothingToRestart,
        CopyRefusal::InsideWorkingDirectory => ReplayRefused::InsideWorkingDirectory,
        CopyRefusal::TaskNotInSource { task_id, why } => {
            ReplayRefused::TaskNotInSource { task_id, why }
        }
    }
}

/// Copy the kept runs into the new execution: every result column from the
/// old row, the lease cleared (`claimed_at` is not listed, so NULL), a new
/// task-run id (the column's default; Kestra `mapTaskRunId`). The task's
/// columns are the TARGET plan's: `copy_onto` matches a leaf on its kind's
/// variant only, so a leaf whose plugin type changed must run as the new
/// type, not the old row's. A copied `WorkingDirectory` block owns no
/// `_wd/{block}/` snapshot under its new id, so it is stamped swept.
const INSERT_COPIED_RUNS: &str = "\
INSERT INTO workflow.task_runs \
  (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, rendered, rendered_run_if, \
   state, history, attempts, not_before_ms, created_seq, ended_seq, resume_with, working_dir, \
   started_at, ended_at, outputs, secret_outputs, outputs_bytes, message, \
   working_dir_snapshot, working_dir_swept_at) \
SELECT $1, m.new_ix, m.task_ix, m.task_id, m.type_id, m.parent_ix, o.value, o.rendered, \
       o.rendered_run_if, o.state, o.history, m.attempts, m.not_before_ms, o.created_seq, \
       o.ended_seq, o.resume_with, o.working_dir, o.started_at, o.ended_at, o.outputs, \
       o.secret_outputs, o.outputs_bytes, o.message, o.working_dir_snapshot, \
       CASE WHEN o.working_dir IS NOT NULL THEN COALESCE(o.working_dir_swept_at, now()) END \
  FROM unnest($3::int[], $4::int[], $5::int[], $6::text[], $7::text[], $8::int[], \
              $9::jsonb[], $10::bigint[]) \
       AS m(old_ix, new_ix, task_ix, task_id, type_id, parent_ix, attempts, not_before_ms) \
  JOIN workflow.task_runs o ON o.execution_id = $2 AND o.run_ix = m.old_ix";

async fn insert_runs(
    conn: &mut PgConnection,
    new: Uuid,
    old: Uuid,
    target: &Plan,
    copied: &CopiedRuns,
) -> Result<()> {
    let ix = |i: usize| i32::try_from(i).expect("a run index fits the column");
    let n = copied.runs.len();
    let mut old_ix = Vec::with_capacity(n);
    let mut new_ix = Vec::with_capacity(n);
    let mut task_ix = Vec::with_capacity(n);
    let mut task_id = Vec::with_capacity(n);
    let mut type_id: Vec<Option<String>> = Vec::with_capacity(n);
    let mut parent_ix: Vec<Option<i32>> = Vec::with_capacity(n);
    let mut attempts = Vec::with_capacity(n);
    let mut not_before: Vec<Option<i64>> = Vec::with_capacity(n);
    for (i, run) in copied.runs.iter().enumerate() {
        let task = &target.tasks[run.task];
        old_ix.push(ix(copied.old_of[i]));
        new_ix.push(ix(i));
        task_ix.push(ix(run.task));
        task_id.push(task.id.clone());
        type_id.push(match &task.kind {
            TaskKind::Runnable { type_id } => Some(type_id.clone()),
            _ => None,
        });
        parent_ix.push(run.parent.map(ix));
        attempts.push(claim::attempts_json(&run.attempts));
        not_before.push(run.not_before.map(|t| t.0));
    }
    let hit = sqlx::query(INSERT_COPIED_RUNS)
        .bind(new)
        .bind(old)
        .bind(&old_ix)
        .bind(&new_ix)
        .bind(&task_ix)
        .bind(&task_id)
        .bind(&type_id)
        .bind(&parent_ix)
        .bind(&attempts)
        .bind(&not_before)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    if hit.rows_affected() != n as u64 {
        return Err(Error::Internal(anyhow::anyhow!(
            "copied {} of {n} runs of execution {old} into {new}",
            hit.rows_affected()
        )));
    }
    Ok(())
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
