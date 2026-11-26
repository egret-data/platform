//! Schedule `stopAfter` (package 11b): when an execution a Schedule trigger
//! started ends in one of the trigger's `stopAfter` states, the trigger is
//! switched off.
//!
//! The off switch is the manual one (`triggers::switch`): a
//! `workflow.disabled_triggers` row, which `scheduler::sync` re-derives
//! `workflow.triggers.disabled` from on every save and restart, so the
//! disable survives them. `disabled_by` is NULL: a system disable. The
//! trigger's own row is also written, so firing stops at once. Runs in the
//! settle's transaction; the caller owns it.

use egret_core::{Error, Result};
use sqlx::PgConnection;

use super::claim::ExecutionCtx;
use crate::executor::ExecutionState;
use crate::models::State;

/// The settle's hook: `ctx`'s execution just became terminal in `state`.
pub(super) async fn on_terminal(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    state: &ExecutionState,
) -> Result<()> {
    let Some(flow_id) = ctx.flow_id else {
        return Ok(());
    };
    let Some(ended) = state.history.last() else {
        return Ok(());
    };
    let trigger: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT execution_trigger FROM workflow.executions WHERE id = $1")
            .bind(ctx.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
    let Some(trigger) = trigger else {
        return Ok(());
    };
    if trigger.get("type").and_then(|t| t.as_str()) != Some(crate::scheduler::SCHEDULE_TYPE) {
        return Ok(());
    }
    let Some(trigger_id) = trigger.get("id").and_then(|t| t.as_str()) else {
        return Ok(());
    };
    // `ctx.source` is the execution's SNAPSHOT of the flow, so an edit to
    // `stopAfter` between the fire and the terminal applies the fired version,
    // as everywhere else the worker reads `ctx.source`.
    let Ok(flow) = crate::spec::parse(&ctx.source) else {
        return Ok(());
    };
    let Some(declared) = flow.triggers.iter().find(|t| t.id == trigger_id) else {
        return Ok(());
    };
    if declared.stop_after.is_empty() {
        return Ok(());
    }
    let Some(ended) = to_state(*ended) else {
        return Ok(());
    };
    if !declared.stop_after.contains(&ended) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id, disabled_by) \
         VALUES ($1, $2, $3, NULL) ON CONFLICT (flow_id, trigger_id) DO NOTHING",
    )
    .bind(flow_id)
    .bind(trigger_id)
    .bind(ctx.workspace_id)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "UPDATE workflow.triggers SET disabled = true, next_evaluation_date = NULL, \
         updated_at = now() WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow_id)
    .bind(trigger_id)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// `RunState::wire` is Kestra's spelling, which is `State`'s serde name.
fn to_state(ended: crate::executor::RunState) -> Option<State> {
    serde_json::from_value(serde_json::Value::String(ended.wire().to_string())).ok()
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::RunState;

    /// Every terminal `RunState` maps to its own `State`; a rename or addition
    /// that broke the mapping would make `stopAfter` silently never disable.
    #[test]
    fn every_terminal_run_state_maps_to_exactly_one_state() {
        let expected = [
            (RunState::Success, State::Success),
            (RunState::Failed, State::Failed),
            (RunState::Warning, State::Warning),
            (RunState::Killed, State::Killed),
            (RunState::Cancelled, State::Cancelled),
            (RunState::Retried, State::Retried),
            (RunState::Skipped, State::Skipped),
        ];
        let terminal: Vec<RunState> = RunState::ALL
            .iter()
            .copied()
            .filter(|s| s.is_terminal())
            .collect();
        assert_eq!(terminal.len(), expected.len(), "a terminal is unlisted");
        for (run, state) in expected {
            assert!(terminal.contains(&run), "{run:?} is terminal");
            assert_eq!(to_state(run), Some(state), "{run:?}");
        }
        let mapped: std::collections::HashSet<State> =
            terminal.iter().filter_map(|r| to_state(*r)).collect();
        assert_eq!(mapped.len(), expected.len(), "one State each");
    }
}
