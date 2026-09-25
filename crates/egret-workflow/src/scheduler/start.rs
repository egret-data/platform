//! What the two loops on the scheduler ticker share when they start an
//! execution on a trigger's behalf: the off switches a trigger is claimed
//! under, and how a failed start is classified.
//!
//! [`fire_due`](super::fire_due) (W12, `Schedule`) and
//! [`flow_trigger::process_events`](crate::flow_trigger::process_events) (W13a,
//! `Flow`) both read these from here, so the SQL spelling of an off switch and
//! the answer to "is this failure worth retrying?" cannot drift between them.
//! Moved out of `fire.rs` by W13a Task 4, unchanged except that [`classify`]
//! now also knows the Flow trigger's backstop index.

use egret_core::Error;

/// The per-workspace feature key whose OFF row freezes every trigger in the
/// workspace (W12 final review I3). Read from `workspace_features`, the table
/// the route gate in `egret_workspace::context` (`WsContext`) reads through
/// `WorkspaceRepo::disabled_features`, with the same rule: an absent row is ON,
/// only `enabled = false` is OFF. Tied to the catalog by
/// `fire.rs`'s `the_feature_key_is_the_one_the_route_gate_uses`.
pub(crate) const WORKFLOWS_FEATURE: &str = "workflows";

/// The four off switches that live outside the YAML, as one SQL fragment over
/// a `workflow.flows` row aliased **`f`**: the flow is not deleted, its
/// `state` is not `'disabled'`, its workspace is `active`, and the
/// workspace's `workflows` feature is not OFF. `$feature` is the placeholder
/// the caller binds [`WORKFLOWS_FEATURE`] to.
///
/// The other two of the six -- the flow YAML's `disabled` and the trigger's
/// own `disabled` -- are copied onto the trigger's row by `sync`
/// (`workflow.triggers.disabled`, `workflow.flow_listeners.disabled`), so each
/// caller adds `NOT <row>.disabled` beside this.
///
/// The spelling of the last two is W12's (final review I3 and the user's
/// decision of 2026-09-25): an `EXISTS` over `workspaces` requiring
/// `status = 'active'`, the column and value `WsContext` and the webhook route
/// refuse a non-active workspace on; and a `NOT EXISTS` over
/// `workspace_features`, the rows the route gate reads.
macro_rules! flow_is_on {
    ($feature:literal) => {
        concat!(
            " f.deleted_at IS NULL \
              AND f.state <> 'disabled' \
              AND EXISTS (SELECT 1 FROM workspaces w \
                           WHERE w.id = f.workspace_id AND w.status = 'active') \
              AND NOT EXISTS (SELECT 1 FROM workspace_features wf \
                               WHERE wf.workspace_id = f.workspace_id \
                                 AND wf.feature = ",
            $feature,
            " AND NOT wf.enabled) "
        )
    };
}
pub(crate) use flow_is_on;

/// The index that makes a second execution for one schedule tick impossible
/// (migration 0156).
pub(crate) const ONE_PER_TICK: &str = "workflow_executions_one_per_schedule_tick";

/// The index that makes a second execution for one (outbox event, listener)
/// impossible (migration 0157).
pub(crate) const ONE_PER_FLOW_TRIGGER_EVENT: &str =
    "workflow_executions_one_per_flow_trigger_event";

/// What starting a trigger's execution came to.
pub(crate) enum Started {
    Yes,
    /// `Error::InvalidInput`: the source does not parse or plan, or an input
    /// is refused. Fixed only by a save, so retrying cannot help.
    Refused(String),
    /// A unique violation on [`ONE_PER_TICK`] or
    /// [`ONE_PER_FLOW_TRIGGER_EVENT`]: this fire already has its execution.
    AlreadyFired(String),
    /// Anything else. Possibly transient (a timeout, a deadlock, a
    /// serialization failure), so the caller retries on its next pass.
    Failed(String),
}

/// Classify a failed `start_execution_in`. See [`Started`].
pub(crate) fn classify(err: &Error) -> Started {
    let why = format!("the execution could not be started: {err}");
    match err {
        Error::InvalidInput(_) => Started::Refused(why),
        Error::Internal(source) => match source
            .downcast_ref::<sqlx::Error>()
            .and_then(|e| e.as_database_error())
            .and_then(|d| d.constraint())
        {
            // W12's wording, unchanged: it lands in `workflow.triggers.last_error`.
            Some(ONE_PER_TICK) => {
                Started::AlreadyFired(format!("this tick already has an execution: {err}"))
            }
            Some(ONE_PER_FLOW_TRIGGER_EVENT) => Started::AlreadyFired(format!(
                "this listener already fired on this flow-trigger event: {err}"
            )),
            _ => Started::Failed(why),
        },
        _ => Started::Failed(why),
    }
}

#[cfg(test)]
mod tests {

    /// The fragment names the table alias `f` and the placeholder it was
    /// given, and nothing else a caller would have to bind.
    #[test]
    fn the_off_switch_fragment_binds_only_the_feature() {
        let sql = flow_is_on!("$7");
        assert!(sql.contains("wf.feature = $7 AND NOT wf.enabled"), "{sql}");
        assert!(sql.contains("f.deleted_at IS NULL"), "{sql}");
        assert!(!sql.contains("$1"), "{sql}");
    }
}
