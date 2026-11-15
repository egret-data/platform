//! Pause by hand and resume with `onResume` inputs -- workflow package 7c
//! (spec `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D3, D5,
//! D6). Both run inside the execution's lock (`claim::load`) and end in
//! `claim::settle`, as every writer does.

use egret_core::{Error, Result};
use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::{self, Resumed, Stack};
use super::txn::{transaction, InTxn};
use crate::executor::{apply, pause_by_hand_commands, PauseRefusal, RunState};

/// What [`pause_by_hand`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paused {
    /// The execution is `PAUSED`; no task run changed.
    Done,
    /// Nothing written: the execution is not `RUNNING` or `RESTARTED`; its
    /// state's wire name (D3).
    NotRunning(String),
    /// Nothing written: a `Pause` or `LoopUntil` is already waiting (Q5).
    TaskWaiting,
}

/// **Pause by hand** (D3; Kestra `ES.pause`, `ES:907-916`): one
/// `SetExecutionState { PAUSED }` under the execution's lock, then the
/// settle, whose `advance` answers nothing (D2). Runs already `RUNNING` are
/// left to finish. The state is re-checked under the lock, so a pause that
/// loses the race to a kill, to the execution's end or to a `Pause` that
/// has just started waiting writes nothing.
///
/// Scoped by `workspace`: an execution of another workspace is `NotFound`.
pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused> {
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
        let (ctx, mut state, now) = claim::load(conn, id, Stack::Fresh(gate.clone())).await?;
        if ctx.workspace_id != workspace {
            return Err(Error::NotFound("execution".into()));
        }
        let cmds = match pause_by_hand_commands(&state) {
            Ok(cmds) => cmds,
            Err(PauseRefusal::NotRunning(s)) => {
                return Ok(Paused::NotRunning(
                    s.map_or("UNKNOWN", |s| s.wire()).to_string(),
                ));
            }
            Err(PauseRefusal::TaskWaiting) => return Ok(Paused::TaskWaiting),
        };
        let before = state.clone();
        for c in &cmds {
            apply(&mut state, c);
        }
        claim::settle(conn, &ctx, &before, state, now, None).await?;
        Ok(Paused::Done)
    })
}

/// A resume, as the route read it (D5, D6).
pub struct ResumeRequest<'a> {
    pub id: Uuid,
    /// The posted values; `None` = no body or `"inputs": null`.
    pub inputs: Option<&'a Map<String, Value>>,
    /// The files the route stored for this resume (S2), all under `expected`.
    pub files: &'a [crate::storage::StoredFile],
    /// The `Pause` run the route read as waiting and stored files under.
    pub expected: Option<Uuid>,
}

/// Why [`resume_with`] wrote nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeRefused {
    /// D5: inputs where no `onResume` applies (paused by hand, or a `Pause`
    /// with none).
    InputsUnexpected,
    /// A value refused, a key not declared (S6), a FILE not readable, or a
    /// SECRET with no master key (S7): the message names the input.
    Invalid(String),
    /// The waiting `Pause` run is not the one the route read (a lost race).
    Moved,
}

/// **Resume** (D5): in the execution's lock. Paused by hand: back to
/// `RUNNING` (no inputs allowed). Otherwise the first `PAUSED` `Pause` run's
/// `onResume` is checked and written into its outputs as `{onResume,
/// resumed}` (S9; SECRET values masked there and sealed into
/// `secret_outputs`, S7), the files stored for it are indexed under it (S2),
/// every waiting `Pause` is released as today (each other one with
/// `onResume: {}`), and the execution settles on (`RESTARTED`,
/// `pause_mark`). Both branches end in a `settle`, so what the pause
/// deferred (a `LoopUntil`'s condition, S4) is rendered now.
///
/// Every refusal comes before the first write: `transaction!` commits an
/// `Ok`, and a refusal's `Ok` has nothing to commit.
pub async fn resume_with(pool: &PgPool, req: &ResumeRequest<'_>) -> Result<Resumed> {
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
        let (ctx, mut state, now) = claim::load(conn, req.id, Stack::Fresh(gate.clone())).await?;
        if state.history.last() != Some(&RunState::Paused) {
            return Ok(Resumed::NotPaused);
        }
        let posted = req.inputs.cloned().unwrap_or_default();
        let before = state.clone();
        if crate::executor::paused_by_hand(&state) {
            if !posted.is_empty() {
                return Ok(Resumed::Refused(ResumeRefused::InputsUnexpected));
            }
            for c in crate::executor::resume_by_hand_commands(&state) {
                apply(&mut state, &c);
            }
            claim::settle(conn, &ctx, &before, state, now, None).await?;
            return Ok(Resumed::ByHand);
        }
        let Some(chosen) = crate::executor::first_waiting_pause(&state) else {
            // Only a `LoopUntil` waits: a resume does not end it.
            return Ok(Resumed::NoPauseWaiting);
        };
        let chosen_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
        )
        .bind(req.id)
        .bind(run_ix(chosen))
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
        if req.expected.is_some_and(|e| e != chosen_id) {
            return Ok(Resumed::Refused(ResumeRefused::Moved));
        }
        let flow = crate::spec::parse(&ctx.source).map_err(|e| {
            Error::Internal(anyhow::anyhow!(
                "execution {}: its frozen source does not parse: {}",
                req.id,
                e.message
            ))
        })?;
        let task_id = ctx.plan.tasks[state.runs[chosen].task].id.clone();
        let declared: Vec<crate::models::Input> = crate::spec::path::find_task(&flow, &task_id)
            .and_then(|t| match &t.kind {
                crate::models::TaskKind::Pause(p) => p.on_resume.clone(),
                _ => None,
            })
            .unwrap_or_default();
        let checked =
            checked_on_resume(conn, &ctx, &flow, &task_id, &declared, &posted, req.files).await?;
        let (values, sealed) = match checked {
            Ok(v) => v,
            Err(r) => return Ok(Resumed::Refused(r)),
        };

        let released = crate::executor::release_paused_pauses(&mut state, now);
        let on = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        for &ix in &released {
            let (outputs, secret) = if ix == chosen {
                (
                    crate::executor::resumed_outputs(values.clone(), &on),
                    sealed.clone(),
                )
            } else {
                (crate::executor::resumed_outputs(Map::new(), &on), None)
            };
            sqlx::query(
                "UPDATE workflow.task_runs SET outputs = $3, secret_outputs = $4 \
                  WHERE execution_id = $1 AND run_ix = $2 AND state = 'PAUSED'",
            )
            .bind(req.id)
            .bind(run_ix(ix))
            .bind(&outputs)
            .bind(secret.as_deref())
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        crate::storage::index_task_files(conn, ctx.workspace_id, chosen_id, req.files).await?;
        claim::settle(conn, &ctx, &before, state, now, None).await?;
        Ok(Resumed::Released(released.len()))
    })
}

/// D5, D6, S6, S7: the posted values against `declared`. `Ok(Err(_))` is the
/// caller's refusal; `Err` is the database's. Answers the values to store
/// (SECRET ones masked) and, when a SECRET was given, their seal.
#[allow(clippy::type_complexity)]
async fn checked_on_resume(
    conn: &mut sqlx::PgConnection,
    ctx: &claim::ExecutionCtx,
    flow: &crate::models::Flow,
    task_id: &str,
    declared: &[crate::models::Input],
    posted: &Map<String, Value>,
    files: &[crate::storage::StoredFile],
) -> Result<std::result::Result<(Map<String, Value>, Option<Vec<u8>>), ResumeRefused>> {
    if declared.is_empty() {
        return Ok(if posted.is_empty() {
            Ok((Map::new(), None))
        } else {
            Err(ResumeRefused::InputsUnexpected)
        });
    }
    // S6: refused, not dropped (the no-silent-drops rule).
    if let Some(k) = posted
        .keys()
        .find(|k| !declared.iter().any(|i| &i.id == *k))
    {
        return Ok(Err(ResumeRefused::Invalid(format!(
            "Invalid value for input `{k}`: the Pause `{task_id}` declares no such onResume input"
        ))));
    }
    let values = match claim::checked_inputs_for(flow, declared, &Value::Object(posted.clone())) {
        Ok(Value::Object(m)) => m,
        Ok(other) => {
            return Err(Error::Internal(anyhow::anyhow!(
                "the input check answered {other}, not an object"
            )))
        }
        Err(e) => return Ok(Err(ResumeRefused::Invalid(e.message))),
    };
    if let Err(why) = crate::storage::inputs::check_file_values(
        conn,
        ctx.workspace_id,
        ctx.id,
        declared,
        &Value::Object(values.clone()),
        files,
    )
    .await?
    {
        return Ok(Err(ResumeRefused::Invalid(why)));
    }
    let secret_ids: Vec<String> = declared
        .iter()
        .filter(|i| i.r#type == crate::models::InputType::Secret)
        .filter(|i| values.get(&i.id).is_some_and(|v| !v.is_null()))
        .map(|i| i.id.clone())
        .collect();
    if secret_ids.is_empty() {
        return Ok(Ok((values, None)));
    }
    if !crate::secret_inputs::can_seal() {
        return Ok(Err(ResumeRefused::Invalid(format!(
            "egret refuses input {}: a SECRET value cannot be stored on this deployment: \
             set secrets.master_key",
            secret_ids.join(", ")
        ))));
    }
    let (masked, sealed) = crate::secret_inputs::split_outputs(&values, &secret_ids)?;
    Ok(Ok((masked, sealed)))
}

fn run_ix(ix: crate::executor::RunIx) -> i32 {
    i32::try_from(ix).expect("a run index fits the column")
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
