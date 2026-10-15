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
use crate::models::wire::required;
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

    /// W5b spec §4.1: a top-level `pluginDefaults:` key, captured **only so
    /// `spec::validate` can refuse it** (`removed_plugin_defaults`) -- Kestra
    /// 2.0 removed flow-level plugin defaults and its artifact refuses the key
    /// at save (Task 1, `plugin_defaults_refusal`). `Some` whenever the key is
    /// present, whatever its value (`null` and `[]` included, see
    /// [`present`]). Nothing reads the value: a draft that runs anyway applies
    /// nothing, as Kestra's lenient read path does.
    ///
    /// `skip_serializing`: a `Flow` is re-serialised into a stored draft
    /// `plan`, a published snapshot's `plan`, and a loop body's synthetic
    /// source (`worker::spawn`); none of them may carry a key egret refuses.
    #[serde(
        default,
        rename = "pluginDefaults",
        deserialize_with = "present",
        skip_serializing
    )]
    pub plugin_defaults: Option<serde_json::Value>,
}

/// `Some(value)` for a key that is present, **including `null`**: a plain
/// `Option<Value>` reads `pluginDefaults:` (YAML null) as `None`, which would
/// let the one shape with no value at all escape the refusal. An absent key
/// never reaches this function (`#[serde(default)]` gives `None`).
fn present<'de, D>(d: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    serde_json::Value::deserialize(d).map(Some)
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
    // `@NotNull @Valid Type type`, no Java default -- same shape as
    // `Concurrency::limit`/`Schedule::cron`. Fix round 1 finding 5: this was
    // left required in the first pass with no comment explaining why it was
    // exempt from the `@NotNull` principle applied everywhere else in this
    // task; it isn't exempt, so it is `Option` too. Task 4 owes the
    // not-null check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<InputType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// `sla/SLA.java` + `sla/types/{MaxDurationSLA,ExecutionAssertionSLA}.java`.
///
/// **W1 ported this as a data shape only and said *"evaluating an SLA against a
/// running execution is W3/W8"*. W8 Task 6 is that, for ONE of the two
/// variants, so the sentence is now half true and the half matters:**
///
/// * [`Sla::ExecutionAssertion`] is Kestra's `ExecutionChangedSLA` and **is
///   evaluated** -- [`crate::sla::first_execution_changed_violation`], called
///   by `worker::claim::settle` where the artifact calls
///   `ExecutorService.handleExecutionChangedSLA`. It banks two golden entries.
/// * [`Sla::MaxDuration`] is Kestra's `ExecutionMonitoringSLA`, evaluated by a
///   separate scheduled loop in the artifact, and **nothing in this crate
///   evaluates one**. `spec::validate`'s `not_yet_enforced` warning was
///   narrowed to exactly that case at W8 Task 6; `crate::sla`'s module doc
///   carries the measurement of why the execution oracle cannot score it
///   either.
///
/// `SLA.Type`'s two `@JsonSubTypes` names (`MAX_DURATION`,
/// `EXECUTION_ASSERTION`) are used as the external tag, the same
/// internally-tagged-enum approach `task.rs` already uses for `Retry`'s three
/// `@JsonSubTypes`.
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

/// `check/Check.java`. Ported as a data shape only -- checks are evaluated
/// before an execution is created, which is W3/W8.
///
/// **This used to read *"same as `Sla` above"* and that pointer went stale at
/// W8 Task 6**, which evaluates one of `Sla`'s two variants. Nothing evaluates
/// a `Check`, which is still the whole claim, and it is now stated rather than
/// delegated -- a comparison to a neighbour is a claim that travels when the
/// neighbour moves.
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
/// `pub(crate)` (not private) so `trigger.rs` and `execution.rs`, both
/// siblings of this module under `models`, can reuse it for their own
/// `labels` fields instead of duplicating the map/list dispatch.
pub(crate) fn deserialize_labels<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Label>, D::Error> {
    use serde::de::Error as _;

    let value = serde_json::Value::deserialize(d)?;
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        // Fix round 1 finding 3: `ListOrMapOfLabelDeserializer.java`'s array
        // branch reads each item as a raw `Map<String, Object>` and coerces
        // `value` with the SAME `String.valueOf` rule its map branch uses --
        // `labels: [{key: count, value: 5}]` (a bare number) is accepted by
        // Kestra. The first pass here instead deserialized each item
        // straight into `Label` (whose derived `Deserialize` requires an
        // already-`String` `value`), so it rejected exactly that input.
        // `coerce_label_value` below is now shared by both branches so they
        // can't drift apart again.
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| {
                let mut obj = match item {
                    serde_json::Value::Object(obj) => obj,
                    other => {
                        return Err(D::Error::custom(format!(
                            "each item in a `labels` list must be an object with `key`/`value`, \
                             got {other}"
                        )))
                    }
                };
                // `required` (see `wire.rs`) so a present-but-wrong-typed
                // `key` (e.g. `key: 5`) is a real type error, not silently
                // read as "missing" the way `.as_str()` would.
                let key: String = required(&mut obj, "key")?;
                let raw_value = obj
                    .remove("value")
                    .ok_or_else(|| D::Error::missing_field("value"))?;
                let value = coerce_label_value(&key, raw_value).map_err(D::Error::custom)?;
                Ok(Label { key, value })
            })
            .collect(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(key, v)| {
                let value = coerce_label_value(&key, v).map_err(D::Error::custom)?;
                Ok(Label { key, value })
            })
            .collect(),
        other => Err(D::Error::custom(format!(
            "labels must be a list of {{key, value}} or a map of string to scalar, got {other}"
        ))),
    }
}

/// `ListOrMapOfLabelDeserializer.java`'s scalar coercion
/// (`String.valueOf(value)` for `String/Integer/Long/Float/Double/Boolean`),
/// shared by both branches of `deserialize_labels` above so the list form and
/// the map form accept the same value types, exactly as Kestra's own
/// deserializer does identically in both of its branches.
fn coerce_label_value(key: &str, v: serde_json::Value) -> Result<String, String> {
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        other => Err(format!(
            "label '{key}': unsupported value {other} (labels must be strings, numbers, or \
             booleans)"
        )),
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
        type: egret.sql
        database: trino_prod
        sql: SELECT 2
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

    /// Fix round 1 finding 3: `ListOrMapOfLabelDeserializer.java`'s array
    /// branch coerces `value` the SAME way its map branch does
    /// (`String.valueOf` for `String/Integer/Long/Float/Double/Boolean`) --
    /// `labels: [{key: count, value: 5}]` (a bare number) is accepted by
    /// Kestra. The first pass here deserialized each list item straight
    /// into `Label`, which requires an already-`String` `value` and
    /// rejected exactly this input.
    #[test]
    fn labels_list_form_coerces_scalars_like_the_map_form_does() {
        let f: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\nlabels:\n  - key: count\n    value: 5\n  - key: active\n    value: true\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        assert_eq!(
            f.labels,
            vec![
                super::Label {
                    key: "count".to_string(),
                    value: "5".to_string(),
                },
                super::Label {
                    key: "active".to_string(),
                    value: "true".to_string(),
                },
            ]
        );

        // The map form already coerced -- this locks in that both branches
        // now agree, not just that the list branch alone was fixed.
        let map_form: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\nlabels:\n  count: 5\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        assert_eq!(map_form.labels[0].value, "5");
    }
}
