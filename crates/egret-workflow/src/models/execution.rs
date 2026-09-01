// kestra: core/src/main/java/io/kestra/core/models/executions/Execution.java
// kestra: core/src/main/java/io/kestra/core/models/executions/TaskRun.java
// kestra: core/src/main/java/io/kestra/core/models/executions/TaskRunAttempt.java
// kestra: core/src/main/java/io/kestra/core/models/executions/ExecutionKind.java
// kestra: core/src/main/java/io/kestra/core/models/flows/State.java (State.History)
//
// Nothing produces an `Execution` until W3 -- the pure state machine that
// advances one. Defined here, one task early, for the same reason `State`
// itself landed in W0: W3 is written against a fixed target rather than a
// moving one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::models::flow::{deserialize_labels, Label};
use crate::models::State;

/// `Execution.java`. Curated to the fields relevant to the pure state
/// machine and its callers; dropped:
/// - `tenantId` -- multi-tenancy, not modelled anywhere else in this crate
///   (`Flow`'s `AbstractFlow.tenantId` was dropped for the same reason).
/// - `trigger` (`ExecutionTrigger`) -- which trigger/execution caused this
///   one; a nested type of its own this task does not port.
/// - `metadata` (`ExecutionMetadata`) -- attempt-count/original-created-date
///   bookkeeping, not needed by W1's model.
/// - `traceParent` -- distributed-tracing header, infra plumbing.
/// - `fixtures` (`List<TaskFixture>`) -- test-fixture support, out of scope.
/// - `kind` (`ExecutionKind`) -- ported below anyway (cheap, and
///   `LOOP`-vs-`NORMAL` matters to how W3 walks a `Loop` task's children),
///   see `ExecutionKind`.
/// - `breakpoints`, `loopRun` -- debugger and loop-iteration features not in
///   W1's scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub id: String,
    pub namespace: String,
    pub flow_id: String,
    // `@NotNull Integer flowRevision`, no Java default -- but structural
    // (identifies exactly which flow revision this execution runs), the same
    // category as `Task::id`/`Flow::id` rather than a config knob, so kept
    // required like those instead of `Option`-wrapped.
    pub flow_revision: i32,

    #[serde(default)]
    pub task_run_list: Vec<TaskRun>,
    #[serde(default)]
    pub inputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub outputs: BTreeMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub variables: BTreeMap<String, serde_json::Value>,

    pub state: StateHistory,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    // `@NotNull String originalId`, no Java default -- `Option` (Task 4/W3
    // owe the not-null check), unlike `id` itself which is this execution's
    // own identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,

    #[serde(default)]
    pub deleted: bool,

    // `Instant`, `@Nullable`. No `java.time.Instant`-compatible deserializer
    // exists in this crate yet -- kept as raw RFC 3339 text, same
    // "no parser yet" reasoning `flow.rs` applies to `Sla`/`Quota`'s
    // `Duration` fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_date: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ExecutionKind>,
}

/// `ExecutionKind.java`. `NORMAL` "is not persisted but null is used
/// instead" per its own doc comment -- reproduced here as `Execution::kind`
/// being `Option<ExecutionKind>` rather than giving this enum its own
/// `#[default]` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionKind {
    Normal,
    Test,
    Playground,
    Loop,
}

/// `TaskRun.java`. Curated the same way `Execution` is: `tenantId` dropped
/// (multi-tenancy), `assetEmits` dropped (asset-lineage tracking, out of
/// scope for W1, mirrors `Execution`'s dropped fields of the same kind).
/// `outputs` is Kestra's own pre-2.0 compatibility field, `@Deprecated
/// (forRemoval = true, since = "2.0.0")` in the source -- ported anyway
/// because it is still a real wire field an old execution may carry, exactly
/// the "declared but unimplemented" treatment `flow.rs` gives `Sla`/`Check`/
/// `Quota`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    pub id: String,
    pub execution_id: String,
    pub namespace: String,
    pub flow_id: String,
    pub task_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub attempts: Vec<TaskRunAttempt>,

    pub state: StateHistory,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_execution: Option<bool>,

    #[serde(default)]
    pub outputs: BTreeMap<String, serde_json::Value>,
}

/// `TaskRunAttempt.java`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunAttempt {
    pub state: StateHistory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    // `URI`, `@Nullable` -- raw text, same "no parser for this Java type yet"
    // reasoning as the `Instant`/`Duration` fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
}

/// `io.kestra.core.models.flows.State` -- named `StateHistory` here (per
/// this task's interface) to keep the name distinct from the `State` enum
/// itself (`State::Type` in Java) that Task/W0 already exports as `State`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHistory {
    pub current: State,
    #[serde(default)]
    pub histories: Vec<StateTransition>,
}

/// `State.History` (inner class) -- named `StateTransition` here for the
/// same reason `StateHistory` itself was renamed: `History` alone reads as
/// generic outside Kestra's own `State` namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateTransition {
    pub state: State,
    // `Instant` -- raw text, same reasoning as `Execution::schedule_date`.
    pub date: String,
}

#[cfg(test)]
mod tests {
    use super::{Execution, StateHistory, TaskRun};
    use crate::models::State;

    /// `Execution`/`TaskRun` have no producer until W3, so this exercises
    /// only that the shapes round-trip -- the same kind of test `State`
    /// itself got in W0 before anything consumed it.
    #[test]
    fn execution_and_task_run_round_trip() {
        let json = serde_json::json!({
            "id": "exec-1",
            "namespace": "company.analytics",
            "flowId": "daily_sales_rollup",
            "flowRevision": 3,
            "taskRunList": [{
                "id": "run-1",
                "executionId": "exec-1",
                "namespace": "company.analytics",
                "flowId": "daily_sales_rollup",
                "taskId": "stage",
                "state": {
                    "current": "SUCCESS",
                    "histories": [
                        {"state": "CREATED", "date": "2026-09-01T00:00:00Z"},
                        {"state": "SUCCESS", "date": "2026-09-01T00:00:05Z"}
                    ]
                }
            }],
            "state": {
                "current": "SUCCESS",
                "histories": [{"state": "CREATED", "date": "2026-09-01T00:00:00Z"}]
            }
        });

        let execution: Execution = serde_json::from_value(json).unwrap();
        assert_eq!(execution.flow_revision, 3);
        assert_eq!(execution.state.current, State::Success);
        assert_eq!(execution.task_run_list.len(), 1);
        assert_eq!(execution.task_run_list[0].task_id, "stage");
        assert_eq!(execution.task_run_list[0].state.histories.len(), 2);

        let back = serde_json::to_value(&execution).unwrap();
        let round: Execution = serde_json::from_value(back).unwrap();
        assert_eq!(round, execution);
    }

    #[test]
    fn task_run_defaults_its_collections_when_absent() {
        let json = serde_json::json!({
            "id": "run-1",
            "executionId": "exec-1",
            "namespace": "company.analytics",
            "flowId": "daily_sales_rollup",
            "taskId": "stage",
            "state": {"current": "CREATED", "histories": []}
        });
        let task_run: TaskRun = serde_json::from_value(json).unwrap();
        assert!(task_run.attempts.is_empty());
        assert!(task_run.outputs.is_empty());
        assert!(task_run.value.is_none());
    }

    #[test]
    fn state_history_current_matches_last_history_entry_in_a_well_formed_execution() {
        let sh: StateHistory = serde_json::from_value(serde_json::json!({
            "current": "RUNNING",
            "histories": [{"state": "CREATED", "date": "2026-09-01T00:00:00Z"}]
        }))
        .unwrap();
        assert_eq!(sh.current, State::Running);
        assert_eq!(sh.histories[0].state, State::Created);
    }
}
