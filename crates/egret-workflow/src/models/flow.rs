// kestra: core/src/main/java/io/kestra/core/models/flows/Flow.java
// kestra: core/src/main/java/io/kestra/core/models/flows/AbstractFlow.java
// kestra: core/src/main/java/io/kestra/core/models/flows/Concurrency.java
// kestra: core/src/main/java/io/kestra/core/models/flows/Output.java
// kestra: core/src/main/java/io/kestra/core/models/Label.java
// kestra: core/src/main/java/io/kestra/core/models/flows/sla/SLA.java (+ sla/types/)
// kestra: core/src/main/java/io/kestra/core/models/flows/check/Check.java
// kestra: core/src/main/java/io/kestra/core/models/flows/quota/Quota.java

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::models::task::Retry;
use crate::models::{Input, InputType, Property, Task, Trigger};

/// A flow: the top-level unit Kestra schedules and runs.
///
/// This is `AbstractFlow` and `Flow` merged into one type -- Kestra splits them
/// because `Flow` "is planned for deprecation" in favour of `FlowWithSource`
/// (which additionally carries the flow's raw YAML), but W1 has no source-
/// tracking type yet, so there is nothing to gain from mirroring that split
/// here.
///
/// Fields dropped from `AbstractFlow`/`Flow` and why:
/// - `revision`, `updated`, `deleted`, `draft`, `tenantId` -- persistence/
///   multi-tenancy metadata about a *stored* flow, not part of what a user
///   pastes as flow YAML. Nothing in this crate persists a flow yet.
/// - `workerSelector` -- worker-routing (tags + fallback), not modelled by any
///   type in this crate.
/// - `policyRefs` -- `@PluginProperty` list of governance-policy ids,
///   Enterprise Edition only ("parsed but ignored in the open-source
///   edition" per its own `@Schema` description) and not a type discriminator
///   the deserializer must recognise (unlike `Input`'s EE-only
///   `REUSABLE_INPUTS`, which Task 2 kept).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    pub id: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: Vec<Label>,

    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub outputs: Vec<FlowOutput>,
    #[serde(default)]
    pub variables: BTreeMap<String, serde_json::Value>,

    // `@NotEmpty` in Java (Task 4 owes that check) -- `#[serde(default)]` so an
    // absent key parses to an empty `Vec` rather than erroring, matching
    // Jackson leaving the field `null` when the key is missing and no
    // `@Builder.Default` fills it.
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub errors: Vec<Task>,

    // `Flow.java`'s field is `_finally`, `@JsonProperty("finally")` -- the wire
    // name is `finally`, a Rust keyword. This rename is the entire point of
    // Step 7's mutation test.
    //
    // Finding from running that mutation: bare-removing `rename = "finally"`
    // here does NOT reproduce the failure the brief describes, because this
    // struct's `#[serde(rename_all = "camelCase")]` independently maps the
    // Rust identifier `finally_` to the wire name `finally` too -- serde's
    // camelCase conversion splits on `_`, capitalises each word after the
    // first, and a trailing `_` yields a trailing empty word, which
    // capitalises to nothing. So `rename_all` alone already produces
    // "finally" from "finally_", and the explicit `rename` is redundant
    // today. The mutation actually run for Step 7 instead changed this to a
    // deliberately WRONG name (`"finallyWRONGMUTATION"`), which does compile
    // and does fail the test -- proving the attribute is real and wired,
    // just not that its specific VALUE is load-bearing against a bare
    // removal. See the task report for the full trace.
    #[serde(rename = "finally", default)]
    pub finally_: Vec<Task>,

    // `Flow.java` line 88: a FOURTH task list this brief's field list missed
    // entirely -- distinct from `_finally`/`errors`. Used for cleanup tasks
    // that must run once even across a restart (`ExecutionService` strips
    // `errors`/`finally`/`afterExecution` tasks specially when an execution
    // is restarted or terminated). Ported as a fourth `Vec<Task>` for the
    // same reason `LoopUntil` was added to `TaskKind` in Task 2: "if one
    // exists that the brief omits, add it with a test."
    #[serde(default)]
    pub after_execution: Vec<Task>,

    #[serde(default)]
    pub triggers: Vec<Trigger>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Concurrency>,

    // `AbstractRetry` -- the exact same type `Task::retry` uses (Flow.java's
    // field is literally typed `AbstractRetry retry`), so this reuses Task
    // 2's `Retry` rather than inventing a second retry model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,

    #[serde(default)]
    pub sla: Vec<Sla>,
    #[serde(default)]
    pub checks: Vec<Check>,
    #[serde(default)]
    pub quotas: Vec<Quota>,

    #[serde(default)]
    pub disabled: bool,
}

/// `io.kestra.core.models.Label` -- a key/value pair. Kept as a plain
/// `{key, value}` record; `Flow::labels`'s custom (de)serialisation is what
/// accepts the map shorthand, not this type itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub key: String,
    pub value: String,
}

/// `Concurrency.java`. `limit` is `@NotNull @Min(1) Integer` with no
/// `@Builder.Default` -- absent *or* explicit `null` both leave it `null` in
/// Java, so it is `Option<i32>` here (Task 4 owes the not-null and `>= 1`
/// checks); the brief's interface typed this field as a bare `limit: i32`,
/// but the `@NotNull`-is-bean-validation principle applies here exactly as it
/// does to `Parallel::concurrent` in `task.rs`, so the source wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Concurrency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub behavior: ConcurrencyBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConcurrencyBehavior {
    #[default]
    Queue,
    Cancel,
    Fail,
}

/// `Output.java`, renamed `FlowOutput` (per this task's interface) to avoid
/// colliding with a task's own output. `type` reuses `InputType`: Kestra's
/// `io.kestra.core.models.flows.Type` enum (the type Flow-level outputs are
/// declared with) has the identical 20-member wire vocabulary as
/// `Input.java`'s `@JsonSubTypes` list Task 2 already ported -- both ultimately
/// select from the same `input/` class hierarchy -- so this reuses `InputType`
/// rather than duplicating it.
///
/// `value` is `@NotNull Object value` in Java, described as "Can be a dynamic
/// expression" -- unrendered source either way, same reasoning `Loop::values`
/// in `task.rs` already applied to a plain (non-`Property`) Java field, so it
/// is `Property<serde_json::Value>` here too, wrapped in `Option` for the same
/// `@NotNull`-is-bean-validation reason as every other `@NotNull`
/// `Property<T>` field in this crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowOutput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Property<serde_json::Value>>,
    pub r#type: InputType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// `sla/SLA.java` + `sla/types/{MaxDurationSLA,ExecutionAssertionSLA}.java`.
/// Ported as a data shape only -- W1 is the model, not the executor;
/// evaluating an SLA against a running execution is W3/W8. `SLA.Type`'s two
/// `@JsonSubTypes` names (`MAX_DURATION`, `EXECUTION_ASSERTION`) are used as
/// the external tag, the same internally-tagged-enum approach `task.rs`
/// already uses for `Retry`'s three `@JsonSubTypes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sla {
    MaxDuration {
        #[serde(flatten)]
        common: SlaCommon,
        // `MaxDurationSLA.duration` is a plain `Duration`, not rendered via
        // `runContext.render(...)` anywhere in `evaluate()` (unlike
        // `ExecutionAssertionSLA`'s `_assert`) -- so it is genuinely static
        // config, not unrendered `Property` source. Kept as raw ISO-8601
        // text: no `java.time.Duration`-compatible parser exists in this
        // crate yet (`Property<Duration>` never actually parses a `Duration`
        // either -- see `property.rs`'s `T` is a marker only). `@NotNull`
        // with no Java default, so `Option` (Task 4 owes the not-null check).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<String>,
    },
    ExecutionAssertion {
        #[serde(flatten)]
        common: SlaCommon,
        // `ExecutionAssertionSLA._assert`, `@JsonProperty("assert")` -- the
        // wire name is the Rust keyword `assert`, same situation as `Flow`'s
        // `_finally`/`finally`. Its Java type is a plain `String`, but
        // `evaluate()` renders it through `runContext.render(...)`, so it is
        // unrendered source -- `Property<String>`, same reasoning as
        // `FlowOutput::value` above.
        #[serde(rename = "assert", default, skip_serializing_if = "Option::is_none")]
        assert_: Option<Property<String>>,
    },
}

/// Fields `SLA.java`'s abstract base declares once for both subtypes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlaCommon {
    pub id: String,
    // `@NotNull` with no Java default (unlike `Retry::behavior`, which has
    // `@Builder.Default`) -- Task 4 owes the not-null check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<SlaBehavior>,
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SlaBehavior {
    Fail,
    Cancel,
    None,
}

/// `check/Check.java`. Ported as a data shape only, same as `Sla` above --
/// checks are evaluated before an execution is created, which is W3/W8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    // `@NotNull @NotEmpty String when`, no Java default. Rendered by
    // `runContext.render(...)`-equivalent expression evaluation at check time
    // (its own doc comment calls it "the expression"), so `Property<String>`;
    // `Option` for the `@NotNull`-is-bean-validation reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Property<String>>,
    // `@NotEmpty String message` -- NOT `@NotNull` (no annotation stacks
    // `@NotNull` here, unlike `when`), genuinely optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub style: CheckStyle,
    #[serde(default)]
    pub behavior: CheckBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStyle {
    Error,
    Success,
    Warning,
    #[default]
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckBehavior {
    #[default]
    BlockExecution,
    FailExecution,
    CreateExecution,
}

/// `quota/Quota.java`. Ported as a data shape only, same as `Sla`/`Check` --
/// quotas are Enterprise Edition ("Quotas can also be defined at the
/// namespace and tenant level" -- EE-only in this checkout) and evaluated
/// before an execution is created, W3/W8 territory either way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    // `@NotNull @DurationMin(minutes = 1) Duration duration`, no Java
    // default. Same "no Duration parser yet, keep raw text" reasoning as
    // `Sla::MaxDuration.duration` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    // `@NotNull @Positive Long limit`, no Java default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    // `@NotNull Behavior behavior`, no Java default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<QuotaBehavior>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QuotaBehavior {
    Fail,
    Cancel,
}

/// `ListOrMapOfLabelDeserializer.java` -- "labels was first a map but has
/// been updated to a List of Label so this deserializer allows using both
/// types." Verified against the source rather than assumed: a JSON/YAML
/// `null` deserialises to a Java `null` there (no `getNullValue()`
/// override needed since there IS a null branch); this crate's `labels`
/// fields are plain (non-`Option`) `Vec<Label>`, so `null` is folded into
/// an empty `Vec` here rather than modelled as a third state -- nothing in
/// this crate's interface distinguishes "no labels" from "labels was
/// explicitly null".
///
/// `pub(super)` (not private) so `trigger.rs` and `execution.rs`, both
/// siblings of this module under `models`, can reuse it for their own
/// `labels` fields instead of duplicating the map/list dispatch.
pub(super) fn deserialize_labels<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Label>, D::Error> {
    use serde::de::Error as _;

    let value = serde_json::Value::deserialize(d)?;
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(D::Error::custom))
            .collect(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(key, v)| {
                let value = match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    other => {
                        return Err(D::Error::custom(format!(
                            "label '{key}': unsupported value {other} (labels must be strings, \
                             numbers, or booleans)"
                        )))
                    }
                };
                Ok(Label { key, value })
            })
            .collect(),
        other => Err(D::Error::custom(format!(
            "labels must be a list of {{key, value}} or a map of string to scalar, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::Flow;

    /// The example from the programme spec, which is what a user will paste.
    #[test]
    fn parses_the_reference_flow() {
        let yaml = r#"
id: daily_sales_rollup
namespace: company.analytics
description: Roll up orders by day
labels:
  team: analytics
inputs:
  - id: run_date
    type: DATE
tasks:
  - id: stage
    type: egret.sql
    database: trino_prod
    sql: SELECT 1
  - id: fanout
    type: core.parallel
    tasks:
      - id: enrich
        type: egret.python
        notebook: nb_enrich
errors:
  - id: alert
    type: core.slack
    message: FAILED
triggers:
  - id: nightly
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 2 * * *"
"#;
        let f: Flow = serde_norway::from_str(yaml).unwrap();
        assert_eq!(f.id, "daily_sales_rollup");
        assert_eq!(f.namespace, "company.analytics");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.tasks.len(), 2);
        assert_eq!(f.errors.len(), 1);
        assert_eq!(f.triggers.len(), 1);
        assert_eq!(f.labels.len(), 1);
        // Nesting must survive: `fanout` owns `enrich`.
        assert_eq!(f.tasks[1].children().len(), 1);
    }

    /// A flow with only the required fields must parse. Every optional field
    /// defaulting correctly is what lets a minimal Kestra flow work.
    #[test]
    fn a_minimal_flow_parses_with_empty_collections() {
        let f: Flow = serde_norway::from_str(
            "id: min\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    sql: SELECT 1\n",
        )
        .unwrap();
        assert!(f.inputs.is_empty());
        assert!(f.errors.is_empty());
        assert!(f.triggers.is_empty());
        assert!(f.labels.is_empty());
        assert!(f.concurrency.is_none());
        assert!(!f.disabled);
    }

    /// `finally` is a Rust keyword-adjacent name and a Kestra field. Getting the
    /// serde rename wrong would silently drop the handler.
    #[test]
    fn finally_is_read_under_its_kestra_spelling() {
        let f: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nfinally:\n  - id: c\n    type: y\n",
        )
        .unwrap();
        assert_eq!(f.finally_.len(), 1, "`finally:` must map onto finally_");
    }

    /// Round 1 (in-task) review: `Flow.java` line 88 has a fourth task list,
    /// `afterExecution`, that this task's own brief missed entirely. Locks in
    /// that it is a real field, not folded into `finally`.
    #[test]
    fn after_execution_is_a_fourth_task_list_the_brief_missed() {
        let f: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nafterExecution:\n  - id: c\n    type: y\n",
        )
        .unwrap();
        assert_eq!(f.after_execution.len(), 1);
        assert!(
            f.finally_.is_empty(),
            "afterExecution must not alias finally"
        );
    }

    /// `labels` accepts both Kestra spellings: a map, and a list of
    /// `{key, value}`. A flow using the list form must not fail to parse.
    #[test]
    fn labels_accepts_both_map_and_list_shapes() {
        let map_form: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\nlabels:\n  team: analytics\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        assert_eq!(
            map_form.labels,
            vec![super::Label {
                key: "team".to_string(),
                value: "analytics".to_string(),
            }]
        );

        let list_form: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\nlabels:\n  - key: team\n    value: analytics\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        assert_eq!(list_form.labels, map_form.labels);
    }
}
