// kestra: executor/src/main/java/io/kestra/executor/ExecutorService.java (onEnd, :441-477)
// kestra: core/src/main/java/io/kestra/core/models/flows/State.java (withState, :73-80)
//! A flow's `outputs:`, evaluated when its execution reaches a terminal state
//! (package #6 Task B1, spec D6, D11).
//!
//! Kestra's `ExecutorService.onEnd` computes the final state, then renders and
//! types `flow.getOutputs()` (`renderOutputs` + `typedOutputs`) and stores them
//! on the execution; a failure logs `Failed to render output values: <cause>`
//! and moves the execution to `FAILED`. egret does the same with the one
//! evaluator of package #6 ([`crate::outputs::evaluate`], spec D2), in the
//! settle that made the execution terminal (`claim::settle`'s outbox phase),
//! so before anything of `afterExecution:` has run, and stores the typed map
//! in `workflow.executions.outputs` (migration 0214).
//!
//! # When
//!
//! - **Any terminal, measured in Kestra's source:** `onEnd` is not gated on
//!   the state, so a FAILED execution evaluates its outputs too; a KILLED one
//!   does as well, because `handleEnd` runs before `handleKilling` and a
//!   `KILLING` execution whose runs have all ended goes through `onEnd` with
//!   `guessFinalState` = `KILLED`.
//! - **Only an execution that ran** (its history holds `RUNNING`): Kestra
//!   reaches `onEnd` only through `handleEnd`, which needs the current tasks
//!   to have run; an execution killed or cancelled before it started never
//!   gets there.
//! - **Not a `Loop` iteration** (spec D6, recorded divergence §6 item 1):
//!   `spawn::sub_flow_source` clears the parent flow's `outputs:` from an
//!   iteration's flow, and [`at_terminal`] skips an iteration too, for an
//!   iteration whose flow was stored before that clear.
//!
//! # A failure
//!
//! Appends `FAILED` after the terminal (`CREATED RUNNING SUCCESS FAILED`,
//! golden `valids/flow-with-outputs-failed`) through the caller's
//! `fail_after_terminal`, which, as Kestra's `State.withState`, appends
//! nothing when the execution is already `FAILED`. The column is written
//! `NULL` ("no outputs evaluated"), as Kestra keeps the execution without
//! outputs.
//!
//! # SECRET and FILE (spec D9, D10), brought forward from Task B4
//!
//! B1 is the first writer of `workflow.executions.outputs`, so, as Task A4 did
//! for a `Loop`'s list, nothing is stored unprotected:
//!
//! - a SECRET output's value is stored as `******` in `outputs` and sealed,
//!   `{output id: value}`, into `secret_outputs` with the `secret_inputs`
//!   cipher; a deployment with no master key fails the output naming it and
//!   `secrets.master_key`; the value is masked in every line written after
//!   the evaluation;
//! - a FILE output must name a readable file of this execution's loop family
//!   (`storage::inputs::check_file_outputs`), or the output fails with
//!   `egret refuses output <id>: <uri> is not a file of this execution`.
//!
//! # The cap (the controller's decision of 2026-10-29, spec §2.2)
//!
//! The typed map may serialise to at most 16 MiB
//! ([`MAX_FLOW_OUTPUTS_BYTES`](crate::runners::env::MAX_FLOW_OUTPUTS_BYTES),
//! the `Loop` list's total); beyond it the evaluation fails as a failing
//! output does, with a message that points to a FILE output. Kestra has no
//! cap; an egret-only limit (spec §6).
//!
//! # Who reads the column
//!
//! `worker::render::Surface::load` (Task B2): `{{ execution.outputs }}` in
//! every later render of the execution (`afterExecution:`), the sealed values
//! revealed there alone; a Flow trigger's render context over the execution
//! sees them masked. Task B3 (2026-10-29): a waiting `Subflow` parent copies
//! the columns onto its run (`worker::subflow::on_child_terminal`), a Flow
//! trigger publishes the masked map as `trigger.outputs`
//! (`flow_trigger::process`), and the execution API returns the masked map
//! (`repo::ExecutionRow::outputs`); no API statement selects `secret_outputs`.

use egret_core::Result;
use sqlx::PgConnection;

use super::claim::ExecutionCtx;
use super::render::Surface;
use super::spawn::log_on_execution;
use crate::executor::{ExecutionState, RunState};
use crate::logs::LogLevel;

/// What [`at_terminal`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlowEnd {
    /// Nothing to evaluate: no `outputs:`, a `Loop` iteration, or an
    /// execution that never ran.
    NoOutputs,
    /// Evaluated and stored.
    Stored,
    /// The evaluation failed: logged on the execution and the column cleared;
    /// the caller appends `FAILED`.
    Failed,
}

/// Evaluate and store the flow's `outputs:` of the execution `ctx` whose
/// state this settle made terminal (see the module doc).
pub(super) async fn at_terminal(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    surface: &Surface,
    state: &ExecutionState,
) -> Result<FlowEnd> {
    if surface.is_loop_iteration() || !state.history.contains(&RunState::Running) {
        return Ok(FlowEnd::NoOutputs);
    }
    let Some(flow) = surface.flow() else {
        return Ok(FlowEnd::NoOutputs);
    };
    if flow.outputs.is_empty() {
        return Ok(FlowEnd::NoOutputs);
    }

    let cx = surface.context(&ctx.plan, state, None);
    let mut redactions: Vec<String> = cx.redactions().to_vec();
    let typed = match crate::outputs::evaluate(&cx, &flow.outputs) {
        Ok(typed) => typed,
        Err(e) => {
            let message = format!("Failed to render output values: {e}");
            return fail(conn, ctx, &message, &redactions).await;
        }
    };
    if let Err(message) = cap(&typed, crate::outputs::flow_outputs_cap(ctx.workspace_id)) {
        return fail(conn, ctx, &message, &redactions).await;
    }
    redactions.extend(typed.secret_ids.iter().filter_map(|id| {
        typed.values.get(id).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    }));
    for warning in &typed.warnings {
        log_on_execution(conn, ctx, LogLevel::Warn, warning, &redactions).await?;
    }
    if !typed.secret_ids.is_empty() && !crate::secret_inputs::can_seal() {
        let message = crate::secret_inputs::no_key_for_secret_outputs(&typed.secret_ids);
        return fail(conn, ctx, &message, &redactions).await;
    }
    if let Err(message) = crate::storage::inputs::check_file_outputs(
        conn,
        ctx.workspace_id,
        ctx.id,
        &flow.outputs,
        &typed.values,
    )
    .await?
    {
        return fail(conn, ctx, &message, &redactions).await;
    }
    let (masked, sealed) = crate::secret_inputs::split_outputs(&typed.values, &typed.secret_ids)?;
    write(conn, ctx, Some(serde_json::Value::Object(masked)), sealed).await?;
    Ok(FlowEnd::Stored)
}

/// The cap of the controller's decision of 2026-10-29 (spec §2.2): `Err`
/// with the message when `typed`'s map serialises to more than `limit` bytes
/// ([`flow_outputs_cap`](crate::outputs::flow_outputs_cap), 16 MiB). The
/// size is the typed map's, SECRET values in clear; the message names sizes
/// only, never a value. A flow output has no `fetchType`, so the way out it
/// names is a FILE output (a value written to internal storage, its URI
/// published), which the cap does not read.
fn cap(typed: &crate::outputs::Typed, limit: usize) -> std::result::Result<(), String> {
    // `Map` serialises infallibly to a `Vec`; an impossible error reads as
    // "no bytes", which a positive cap passes.
    let bytes = serde_json::to_vec(&typed.values).map_or(0, |v| v.len());
    if bytes <= limit {
        return Ok(());
    }
    Err(format!(
        "Flow outputs are {bytes} bytes; egret keeps at most {} of flow outputs per execution. \
         Write a large value to internal storage and declare it as a type: FILE output instead.",
        super::spawn::bytes_text(limit),
    ))
}

/// Log `message` at ERROR on the execution, clear the columns, answer
/// [`FlowEnd::Failed`].
async fn fail(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    message: &str,
    redactions: &[String],
) -> Result<FlowEnd> {
    log_on_execution(conn, ctx, LogLevel::Error, message, redactions).await?;
    write(conn, ctx, None, None).await?;
    Ok(FlowEnd::Failed)
}

/// Write `outputs` and `secret_outputs` of the execution (`NULL` for `None`):
/// a re-evaluation (a restarted execution's new terminal) replaces the last.
async fn write(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    outputs: Option<serde_json::Value>,
    sealed: Option<Vec<u8>>,
) -> Result<()> {
    sqlx::query("UPDATE workflow.executions SET outputs = $2, secret_outputs = $3 WHERE id = $1")
        .bind(ctx.id)
        .bind(outputs)
        .bind(sealed)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            egret_core::Error::Internal(anyhow::anyhow!(
                "execution {}: writing its outputs: {e}",
                ctx.id
            ))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cap;
    use crate::runners::env::MAX_FLOW_OUTPUTS_BYTES as CAP;

    fn typed(v: &str) -> crate::outputs::Typed {
        let mut values = serde_json::Map::new();
        values.insert("big".into(), serde_json::Value::String(v.into()));
        crate::outputs::Typed {
            values,
            secret_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// The controller's cap of 2026-10-29 at its real value: 16 MiB is kept,
    /// one byte more fails with the message naming the size, `16 MiB` and a
    /// FILE output (`{"big":"<s>"}` is `s.len() + 10` bytes).
    #[test]
    fn flow_outputs_over_sixteen_mib_fail_naming_a_file_output() {
        assert_eq!(CAP, 16 << 20);
        assert_eq!(cap(&typed(&"x".repeat(CAP - 10)), CAP), Ok(()));
        assert_eq!(
            cap(&typed(&"x".repeat(CAP - 9)), CAP),
            Err(format!(
                "Flow outputs are {} bytes; egret keeps at most 16 MiB of flow outputs per \
                 execution. Write a large value to internal storage and declare it as a type: \
                 FILE output instead.",
                CAP + 1
            ))
        );
    }
}
