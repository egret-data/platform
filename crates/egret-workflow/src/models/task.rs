// kestra: core/src/main/java/io/kestra/core/models/tasks/Task.java
// kestra: core/src/main/java/io/kestra/core/models/tasks/retrys/
// kestra: core/src/main/java/io/kestra/plugin/core/flow/

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::models::Property;

/// A task node in a flow. Control flow is typed (`TaskKind`'s non-`Leaf`
/// variants); everything else is an opaque `Leaf` -- a state machine that
/// knows what `egret.sql` is has stopped being a state machine.
///
/// `Serialize` is hand-written (see below), mirroring the hand-written
/// `Deserialize`: a derive + `#[serde(flatten)] kind: TaskKind` would put a
/// `Leaf`'s own fields under a `"spec"` key (`#[serde(untagged)]` serialises
/// a *struct* variant as its field map, not flattened into the parent), and
/// re-parsing that output would grow a `spec.spec.spec...` nest on every
/// round-trip. See the task report, Finding 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub r#type: String,
    pub description: Option<String>,
    pub retry: Option<Retry>,
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    pub kind: TaskKind,
}

/// Control flow the executor must see into, versus the opaque leaf it must
/// not. Variant names and the types they carry are Kestra's own names for
/// the classes actually present in this checkout's
/// `core/src/main/java/io/kestra/plugin/core/flow/` -- verified by reading
/// that directory, not assumed from the W1 plan, which named this type
/// `ForEach` (no such class exists here; Kestra's own name is `Loop`) and
/// omitted `LoopUntil` entirely (see the task report for both corrections).
///
/// No `Serialize`/`Deserialize` derive here on purpose: `Task`'s own hand
/// -written impls dispatch into (or build) the right variant's fields
/// directly, at the top level of the JSON object, rather than through this
/// enum's own tag/flatten machinery.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskKind {
    Sequential(Sequential),
    Parallel(Parallel),
    If(If),
    Switch(Switch),
    Dag(Dag),
    Loop(Loop),
    LoopUntil(LoopUntil),
    Subflow(Subflow),
    AllowFailure(AllowFailure),
    WorkingDirectory(WorkingDirectory),
    Leaf { spec: serde_json::Value },
}

impl Task {
    /// The tasks this node owns, in declaration order. Empty for a leaf.
    ///
    /// This is what W3's `advance()` walks, and it is the reason control flow is
    /// typed while leaves are opaque.
    pub fn children(&self) -> Vec<&Task> {
        match &self.kind {
            TaskKind::Sequential(s) => s.tasks.iter().collect(),
            TaskKind::Parallel(p) => p.tasks.iter().collect(),
            TaskKind::If(i) => i.then.iter().chain(i.r#else.iter()).collect(),
            TaskKind::Switch(s) => s
                .cases
                .values()
                .flatten()
                .chain(s.defaults.iter())
                .collect(),
            TaskKind::Dag(d) => d.tasks.iter().map(|n| &n.task).collect(),
            TaskKind::Loop(l) => l.tasks.iter().collect(),
            TaskKind::LoopUntil(l) => l.tasks.iter().collect(),
            TaskKind::AllowFailure(a) => a.tasks.iter().collect(),
            TaskKind::WorkingDirectory(w) => w.tasks.iter().collect(),
            TaskKind::Subflow(_) | TaskKind::Leaf { .. } => Vec::new(),
        }
    }
}

// kestra: Sequential.java -- `AbstractBranch`'s only field, `tasks`, run one
// after another. `errors`/`finally` are AbstractBranch fields too but are
// dropped here along with every other control-flow type's `errors`/`finally`:
// W1 is the model, not the executor, and nothing in this task's interface
// (`children()`) walks them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sequential {
    pub tasks: Vec<Task>,
}

// kestra: Parallel.java -- `AbstractBranch` + `concurrent` (0 = unlimited).
// `concurrent` is `@NotNull` in Java (Task 4 owes a check for that), but
// `@NotNull` is bean validation, evaluated when Kestra *validates* a flow --
// not when Jackson *parses* one. So an explicit `concurrent: null` genuinely
// parses in Kestra (it just fails validation later), which means parsing
// here must accept it too: `Option<Property<i32>>`, not a bare `Property<i32>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parallel {
    pub tasks: Vec<Task>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Option<Property<i32>>,
}

// kestra: If.java. `condition` is a plain `String`, NOT `Property<String>`,
// and Kestra's own source explains why: "we can't use Property<String> here
// because of the cache of the property evaluation which causes issue when
// using If in a ForEach with concurrencyLimit > 1"
// (https://github.com/kestra-io/kestra/issues/8697). The plan's brief typed
// this field `Property<String>`; the source wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct If {
    pub condition: String,
    #[serde(default)]
    pub then: Vec<Task>,
    #[serde(default, rename = "else")]
    pub r#else: Vec<Task>,
}

// kestra: Switch.java. `value` is `@NotNull` in Java (Task 4 owes a check);
// bean validation runs at flow-validation time, not at parse time, so parse
// must still accept an explicit `value: null` -- `Option<Property<String>>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Property<String>>,
    #[serde(default)]
    pub cases: BTreeMap<String, Vec<Task>>,
    #[serde(default)]
    pub defaults: Vec<Task>,
}

// kestra: Dag.java -- `tasks` is a list of `(task, dependsOn)` pairs, not a
// plain task list, and it carries the same `concurrent` field as `Parallel`
// (Kestra's field, not this task's invention -- `Dag.java` line 98), same
// `@NotNull`-is-bean-validation-not-parsing caveat as `Parallel::concurrent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dag {
    pub tasks: Vec<DagNode>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Option<Property<i32>>,
}

// kestra: Dag.DagTask (inner class). Named `DagNode` here per this task's
// produced interface -- the Rust name isn't wire-visible (unlike `type`), so
// it does not need to match Kestra's inner-class name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DagNode {
    pub task: Task,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

// kestra: Loop.java. THIS is Kestra's `ForEach` in this checkout -- there is
// no `io.kestra.plugin.core.flow.ForEach` class here (verified: only
// `Loop.java` and `LoopUntil.java` exist under `core/flow/`). The plan and
// design doc both assumed the name `ForEach`; Kestra's real name is `Loop`,
// so that is what this type and its `TaskKind` variant are called.
//
// `values` is `Object` in Java (string, array, or ION-file URI, rendered at
// use time) and is never wrapped in `Property<T>` there -- but it is
// unrendered source either way, so `Property<serde_json::Value>` reproduces
// the "any YAML shape becomes source text" behaviour every other dynamic
// field in this crate already gets from `Property`. `values` is `@NotNull`
// in Java (Task 4 owes a check); parsing must still accept an explicit
// `values: null`, same reasoning as `Parallel::concurrent` above.
//
// `transmitFailed` (Java: `Boolean = true`, no `@NotNull`) is ported too --
// omitted from the first pass of this task without being named as dropped,
// which was the actual defect flagged in review (silence, not the omission
// itself); `outputs`/`fetchType` remain deliberately out (execution-plane,
// unused before W3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loop {
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Property<serde_json::Value>>,
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: i32,
    #[serde(default = "default_true")]
    pub transmit_failed: bool,
}

// kestra: LoopUntil.java -- a second `AbstractBranch` sibling of `Loop` this
// checkout has that the plan's Step-1 reading did not list (it named only
// Sequential/Parallel/If/Switch/Dag/ForEach/Subflow/AllowFailure/
// WorkingDirectory). It runs `tasks` in a loop, evaluating `condition` after
// each iteration, so W3's `advance()` must walk its `tasks` exactly like
// `Loop`'s -- added here per this task's resolution: "if one exists that the
// brief omits, add it with a test." `condition` is `@NotNull` in Java (Task 4
// owes a check); parsing must still accept an explicit `condition: null`,
// same reasoning as `Parallel::concurrent` above.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopUntil {
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Property<String>>,
}

// kestra: Subflow.java. Kept to the interface's field list
// (namespace/flowId/inputs/wait/transmitFailed); `revision`, `labels`,
// `inheritLabels`, `scheduleDate`, and `restartBehavior` are dropped for now,
// the same curation the brief already applied to this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subflow {
    pub namespace: String,
    pub flow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(default = "default_true")]
    pub wait: bool,
    #[serde(default = "default_true")]
    pub transmit_failed: bool,
}

// kestra: AllowFailure.java -- `extends Sequential` with no fields of its
// own, so its Rust shape is identical to `Sequential`'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowFailure {
    pub tasks: Vec<Task>,
}

// kestra: WorkingDirectory.java -- also `extends Sequential`; `cache`,
// `namespaceFiles`, `inputFiles`, `outputFiles` are execution-plane concerns
// dropped here (the same curation the brief applied to `Subflow`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingDirectory {
    pub tasks: Vec<Task>,
}

// Used only for the `#[serde(default = "...")]` case: the key is *absent*,
// matching Java's `@Builder.Default` value (`Property.ofValue(0)`) taking
// effect when the field is never assigned. An explicit `null` in the input
// bypasses this function entirely -- `Option<Property<i32>>`'s own
// `Deserialize` sees the `null` token and yields `None` directly, exactly
// as Jackson would null out the field despite its default initializer.
fn default_concurrent() -> Option<Property<i32>> {
    Some(Property::from_source("0".to_string()))
}

fn default_concurrency_limit() -> i32 {
    1
}

const fn default_true() -> bool {
    true
}

// kestra: retrys/AbstractRetry.java + Constant.java + Exponential.java +
// Random.java. Unlike `Task`, `AbstractRetry` really does use
// `@JsonSubTypes` (`@JsonTypeInfo(use = NAME, property = "type", ...)`) over
// a closed set of exactly three subtypes, so a plain internally-tagged serde
// enum reproduces Kestra's own dispatch instead of approximating it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryCommon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Property<Duration>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<i32>,
    #[serde(default)]
    pub warning_on_retry: bool,
    #[serde(default)]
    pub behavior: RetryBehavior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetryBehavior {
    #[default]
    RetryFailedTask,
    CreateNewExecution,
}

// `interval`/`maxInterval`/`minInterval` are `@NotNull` on all three Java
// subtypes (Task 4 owes a check on each). Same bean-validation-not-parsing
// reasoning as `Parallel::concurrent`: parsing must accept an explicit
// `null` for any of them, so all five are `Option<Property<Duration>>`, not
// bare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Retry {
    Constant {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval: Option<Property<Duration>>,
    },
    #[serde(rename_all = "camelCase")]
    Exponential {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_factor: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Random {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_interval: Option<Property<Duration>>,
    },
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut map = serde_json::Map::deserialize(d)?;

        let id = map
            .remove("id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("id"))?;
        let r#type = map
            .remove("type")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("type"))?;
        let description = map
            .remove("description")
            .and_then(|v| v.as_str().map(str::to_owned));
        let retry = match map.remove("retry") {
            Some(v) => Some(serde_json::from_value(v).map_err(D::Error::custom)?),
            None => None,
        };
        let timeout = match map.remove("timeout") {
            Some(v) => Some(serde_json::from_value(v).map_err(D::Error::custom)?),
            None => None,
        };
        let disabled = map
            .remove("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let rest = serde_json::Value::Object(map);
        let kind = kind_for(&r#type, rest).map_err(D::Error::custom)?;

        Ok(Task {
            id,
            r#type,
            description,
            retry,
            timeout,
            disabled,
            kind,
        })
    }
}

/// The inverse of the custom `Deserialize` above: emit the shared fields,
/// then flatten the control-flow struct's (or the leaf's `spec` map's)
/// fields into the *same* JSON object, at the top level -- not nested under
/// a `"kind"` or `"spec"` key. That is what makes `parse -> serialise` valid
/// Kestra YAML again and what makes `parse -> serialise -> parse ->
/// serialise` idempotent (see the round-trip tests below).
impl Serialize for Task {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let kind_value = match &self.kind {
            TaskKind::Sequential(v) => serde_json::to_value(v),
            TaskKind::Parallel(v) => serde_json::to_value(v),
            TaskKind::If(v) => serde_json::to_value(v),
            TaskKind::Switch(v) => serde_json::to_value(v),
            TaskKind::Dag(v) => serde_json::to_value(v),
            TaskKind::Loop(v) => serde_json::to_value(v),
            TaskKind::LoopUntil(v) => serde_json::to_value(v),
            TaskKind::Subflow(v) => serde_json::to_value(v),
            TaskKind::AllowFailure(v) => serde_json::to_value(v),
            TaskKind::WorkingDirectory(v) => serde_json::to_value(v),
            // The leaf's own fields are already a flat map -- `rest` from
            // `Deserialize`, untouched -- so there is nothing to flatten
            // them out of, unlike a struct-variant's `#[serde(untagged)]`
            // serialisation.
            TaskKind::Leaf { spec } => Ok(spec.clone()),
        }
        .map_err(S::Error::custom)?;

        let mut map = match kind_value {
            serde_json::Value::Object(map) => map,
            other => {
                return Err(S::Error::custom(format!(
                    "task kind serialised to a non-object JSON value: {other}"
                )))
            }
        };

        // Task's own fields are inserted last so they always win: `id`/
        // `type`/etc. can never appear inside a leaf's `spec` map to begin
        // with (the Deserialize above strips them before building `rest`),
        // so there is no real collision today, but this ordering keeps it
        // that way even if a future control-flow struct ever grew a
        // same-named field.
        map.insert("id".to_string(), serde_json::Value::String(self.id.clone()));
        map.insert(
            "type".to_string(),
            serde_json::Value::String(self.r#type.clone()),
        );
        if let Some(description) = &self.description {
            map.insert(
                "description".to_string(),
                serde_json::Value::String(description.clone()),
            );
        }
        if let Some(retry) = &self.retry {
            map.insert(
                "retry".to_string(),
                serde_json::to_value(retry).map_err(S::Error::custom)?,
            );
        }
        if let Some(timeout) = &self.timeout {
            map.insert(
                "timeout".to_string(),
                serde_json::to_value(timeout).map_err(S::Error::custom)?,
            );
        }
        map.insert(
            "disabled".to_string(),
            serde_json::Value::Bool(self.disabled),
        );

        serde_json::Value::Object(map).serialize(serializer)
    }
}

/// Dispatch a task's remaining fields on its `type`.
///
/// Both spellings are accepted and the FQCN is canonical, so a Kestra flow
/// pasted in parses and looking a task up in Kestra's docs lands on the right
/// page (programme spec section 7 rule 4). The short-alias spelling itself
/// (`core.` + the lower-cased class name, e.g. `core.allowfailure`) is the
/// W1 plan's own established scheme (used throughout
/// `docs/en/plans/2026-09-01-workflow-w1-model.md`), not invented for this
/// task; it is applied here to every real control-flow class name found by
/// reading `core/src/main/java/io/kestra/plugin/core/flow/`, including the
/// two names (`loop`, `loopuntil`) the plan's own reading missed.
fn kind_for(r#type: &str, rest: serde_json::Value) -> Result<TaskKind, serde_json::Error> {
    let canonical = r#type
        .strip_prefix("io.kestra.plugin.core.flow.")
        .map(str::to_ascii_lowercase)
        .or_else(|| r#type.strip_prefix("core.").map(str::to_ascii_lowercase));

    Ok(match canonical.as_deref() {
        Some("sequential") => TaskKind::Sequential(serde_json::from_value(rest)?),
        Some("parallel") => TaskKind::Parallel(serde_json::from_value(rest)?),
        Some("if") => TaskKind::If(serde_json::from_value(rest)?),
        Some("switch") => TaskKind::Switch(serde_json::from_value(rest)?),
        Some("dag") => TaskKind::Dag(serde_json::from_value(rest)?),
        Some("loop") => TaskKind::Loop(serde_json::from_value(rest)?),
        Some("loopuntil") => TaskKind::LoopUntil(serde_json::from_value(rest)?),
        Some("subflow") => TaskKind::Subflow(serde_json::from_value(rest)?),
        Some("allowfailure") => TaskKind::AllowFailure(serde_json::from_value(rest)?),
        Some("workingdirectory") => TaskKind::WorkingDirectory(serde_json::from_value(rest)?),
        _ => TaskKind::Leaf { spec: rest },
    })
}

#[cfg(test)]
mod tests {
    use super::{Retry, RetryBehavior, Task, TaskKind};

    /// A leaf task egret owns. The model must NOT understand what egret.sql is --
    /// a state machine that knows what SQL is has stopped being a state machine.
    #[test]
    fn an_unknown_type_becomes_an_opaque_leaf() {
        let yaml = r#"
id: stage
type: egret.sql
database: trino_prod
sql: SELECT 1
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert_eq!(t.id, "stage");
        assert_eq!(t.r#type, "egret.sql");
        match &t.kind {
            TaskKind::Leaf { spec } => {
                assert_eq!(spec["database"], "trino_prod");
                assert_eq!(spec["sql"], "SELECT 1");
                // `id` and `type` are Task's own fields and must not be
                // duplicated into the leaf spec, or a handler would see two
                // sources of truth for them.
                assert!(spec.get("id").is_none(), "id leaked into the leaf spec");
                assert!(spec.get("type").is_none(), "type leaked into the leaf spec");
            }
            other => panic!("expected Leaf, got {other:?}"),
        }
    }

    /// Control flow the state machine DOES have to see into. The FQCN is the
    /// canonical spelling; the short alias is the secondary path.
    #[test]
    fn control_flow_is_typed_under_both_spellings() {
        for spelling in ["io.kestra.plugin.core.flow.Sequential", "core.sequential"] {
            let yaml = format!(
                "id: fan\ntype: {spelling}\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n"
            );
            let t: Task = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TaskKind::Sequential(s) => assert_eq!(s.tasks.len(), 1, "{spelling}"),
                other => panic!("{spelling}: expected Sequential, got {other:?}"),
            }
        }
    }

    /// `children()` is what W3's advance() walks. A Leaf owning children would
    /// make the state machine descend into a task it must not understand.
    #[test]
    fn children_are_the_control_flow_nodes_only() {
        let yaml = r#"
id: outer
type: core.sequential
tasks:
  - id: a
    type: egret.sql
    sql: SELECT 1
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "children must be in declaration order");
        assert!(
            t.children()[0].children().is_empty(),
            "a leaf owns no children"
        );
    }

    #[test]
    fn disabled_defaults_to_false() {
        let t: Task = serde_norway::from_str("id: a\ntype: egret.sql\nsql: SELECT 1\n").unwrap();
        assert!(!t.disabled);
    }

    /// Kestra's `ForEach` is not named that in this checkout -- verified by
    /// reading `core/src/main/java/io/kestra/plugin/core/flow/`, which has
    /// `Loop.java`, not `ForEach.java`. Both the W1 plan and design doc
    /// assumed `ForEach`; this locks in the correction with the same
    /// both-spellings coverage `control_flow_is_typed_under_both_spellings`
    /// gives `Sequential`.
    #[test]
    fn foreach_is_really_named_loop_in_this_checkout() {
        for spelling in ["io.kestra.plugin.core.flow.Loop", "core.loop"] {
            let yaml = format!(
                "id: fan\ntype: {spelling}\nvalues: [1, 2, 3]\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n"
            );
            let t: Task = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TaskKind::Loop(l) => {
                    assert_eq!(l.tasks.len(), 1, "{spelling}");
                    assert_eq!(l.values.as_ref().unwrap().source(), "[1,2,3]", "{spelling}");
                }
                other => panic!("{spelling}: expected Loop, got {other:?}"),
            }
        }
    }

    /// `LoopUntil.java` is a second control-flow type this checkout has that
    /// the plan's Step-1 reading omitted entirely. It is a genuine sibling of
    /// `Loop` (both extend `AbstractBranch`), so it must be typed the same
    /// way, not fall through to `Leaf`.
    #[test]
    fn loop_until_is_a_control_flow_type_the_plan_missed() {
        let yaml = r#"
id: retry_until_ready
type: io.kestra.plugin.core.flow.LoopUntil
condition: "{{ outputs.check.value == 'ready' }}"
tasks:
  - id: check
    type: egret.sql
    sql: SELECT 1
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::LoopUntil(l) => {
                assert_eq!(
                    l.condition.as_ref().unwrap().source(),
                    "{{ outputs.check.value == 'ready' }}"
                );
                assert_eq!(t.children().len(), 1);
            }
            other => panic!("expected LoopUntil, got {other:?}"),
        }
    }

    // -- Review round 1, Finding 1: Serialize must be Deserialize's inverse --

    /// `Leaf { spec }` is a struct variant, and `#[serde(untagged)]`'s
    /// derived `Serialize` writes a struct variant as its *field map*
    /// (`{"spec": {...}}`), not flattened -- so re-parsing that and
    /// serialising again nested `spec.spec.spec...` one level deeper every
    /// round-trip, and even the FIRST serialisation wasn't valid Kestra YAML
    /// (a leaf's own fields belong at the top level, beside `id`/`type`).
    /// The fix is `Task`'s hand-written `Serialize`; this asserts the
    /// property directly rather than trusting the implementation.
    #[test]
    fn leaf_round_trips_without_growing_a_spec_wrapper() {
        let yaml = "id: t\ntype: egret.sql\ndatabase: d\nsql: SELECT 1\n";
        let t1: Task = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(as_value["database"], "d");
        assert_eq!(as_value["sql"], "SELECT 1");
        assert!(
            as_value.get("spec").is_none(),
            "a leaf's fields must sit at the top level, not nested under spec: {once}"
        );

        let t2: Task = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
    }

    /// Same property, one level down: a `Sequential` whose only child is a
    /// leaf must not corrupt that child on re-serialisation either.
    #[test]
    fn nested_sequential_leaf_round_trips_without_growing_a_spec_wrapper() {
        let yaml = "id: outer\ntype: core.sequential\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t1: Task = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        let inner = &as_value["tasks"][0];
        assert_eq!(inner["sql"], "SELECT 1");
        assert!(inner.get("spec").is_none());

        let t2: Task = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
    }

    // -- Review round 1, Finding 2: one parsing test per remaining control-flow type --

    /// `Parallel.java`'s `concurrent`, plus `children()` still returning
    /// `tasks` in declaration order for this type too.
    #[test]
    fn parallel_carries_its_own_tasks_and_concurrent() {
        let yaml = r#"
id: fan
type: core.parallel
concurrent: 2
tasks:
  - id: a
    type: egret.sql
    sql: SELECT 1
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Parallel(p) => {
                assert_eq!(p.concurrent.as_ref().unwrap().source(), "2");
            }
            other => panic!("expected Parallel, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    /// `If.java`'s `_else` field is `@JsonProperty("else")` -- the wire name
    /// is the Rust keyword `else`, hence `r#else`. Exercises both branches
    /// and `children()`'s `then`-before-`else` chaining order.
    #[test]
    fn if_uses_else_not_r_else_on_the_wire() {
        let yaml = r#"
id: gate
type: io.kestra.plugin.core.flow.If
condition: "{{ 1 == 1 }}"
then:
  - id: a
    type: egret.sql
    sql: SELECT 1
else:
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::If(i) => {
                assert_eq!(i.condition, "{{ 1 == 1 }}");
                assert_eq!(i.then.len(), 1);
                assert_eq!(i.r#else.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "children() must chain then before else");
    }

    /// `Switch.java`'s `cases` is a `Map<String, List<Task>>` keyed by the
    /// literal matched value, not a list of `(value, tasks)` pairs.
    #[test]
    fn switch_cases_are_keyed_by_the_matched_value() {
        let yaml = r#"
id: route
type: core.switch
value: "{{ inputs.env }}"
cases:
  prod:
    - id: p
      type: egret.sql
      sql: SELECT 1
  dev:
    - id: d
      type: egret.sql
      sql: SELECT 2
defaults:
  - id: fallback
    type: egret.sql
    sql: SELECT 3
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Switch(s) => {
                assert_eq!(s.value.as_ref().unwrap().source(), "{{ inputs.env }}");
                assert_eq!(s.cases["prod"][0].id, "p");
                assert_eq!(s.cases["dev"][0].id, "d");
                assert_eq!(s.defaults[0].id, "fallback");
            }
            other => panic!("expected Switch, got {other:?}"),
        }
        // BTreeMap iterates keys in sorted order ("dev" < "prod"), so
        // children() sees dev's tasks, then prod's, then defaults.
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["d", "p", "fallback"]);
    }

    /// `Dag.DagTask.dependsOn` is camelCase on the wire (`dependsOn`), and
    /// `Dag` carries the same `concurrent` field `Parallel` does. `children()`
    /// must stay in declaration order, not dependency order.
    #[test]
    fn dag_depends_on_is_camel_case_and_concurrent_is_ported() {
        let yaml = r#"
id: pipeline
type: core.dag
concurrent: 1
tasks:
  - task:
      id: extract
      type: egret.sql
      sql: SELECT 1
  - task:
      id: load
      type: egret.sql
      sql: SELECT 2
    dependsOn: ["extract"]
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Dag(d) => {
                assert_eq!(d.concurrent.as_ref().unwrap().source(), "1");
                assert_eq!(d.tasks[1].depends_on, vec!["extract".to_string()]);
                assert!(d.tasks[0].depends_on.is_empty());
            }
            other => panic!("expected Dag, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["extract", "load"]);
    }

    /// `Subflow.java`'s `flowId` is camelCase on the wire, and both booleans
    /// default to `true` in Kestra. A `Subflow` spawns a *different* flow's
    /// execution, so `children()` must stay empty -- it owns no local tasks.
    #[test]
    fn subflow_uses_flow_id_camel_case_and_defaults_wait_true() {
        let yaml = r#"
id: call
type: io.kestra.plugin.core.flow.Subflow
namespace: company.team
flowId: nightly_rollup
inputs:
  as_of: "{{ execution.startDate }}"
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Subflow(s) => {
                assert_eq!(s.namespace, "company.team");
                assert_eq!(s.flow_id, "nightly_rollup");
                assert!(s.wait);
                assert!(s.transmit_failed);
                assert_eq!(
                    s.inputs.as_ref().unwrap()["as_of"],
                    serde_json::json!("{{ execution.startDate }}")
                );
            }
            other => panic!("expected Subflow, got {other:?}"),
        }
        assert!(
            t.children().is_empty(),
            "Subflow spawns a different flow's execution, not local children"
        );
    }

    /// `AllowFailure.java` is `extends Sequential` with no fields of its
    /// own -- confirms it still gets `tasks` and isn't accidentally routed
    /// to `Leaf`.
    #[test]
    fn allow_failure_carries_tasks_like_sequential() {
        let yaml = "id: risky\ntype: core.allowfailure\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::AllowFailure(a) => assert_eq!(a.tasks.len(), 1),
            other => panic!("expected AllowFailure, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }

    /// `WorkingDirectory.java` is also `extends Sequential` -- same check as
    /// `AllowFailure`, under its own FQCN.
    #[test]
    fn working_directory_carries_tasks_like_sequential() {
        let yaml = "id: scoped\ntype: io.kestra.plugin.core.flow.WorkingDirectory\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::WorkingDirectory(w) => assert_eq!(w.tasks.len(), 1),
            other => panic!("expected WorkingDirectory, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }

    /// `AbstractRetry`'s three real `@JsonSubTypes` -- `constant`/
    /// `exponential`/`random` -- dispatched on `type`, plus the shared
    /// `RetryCommon` fields each variant flattens in.
    #[test]
    fn retry_dispatches_all_three_json_subtypes() {
        let constant = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: constant\n  interval: PT10S\n  maxAttempts: 3\n";
        let t: Task = serde_norway::from_str(constant).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Constant { common, interval } => {
                assert_eq!(common.max_attempts, Some(3));
                assert_eq!(interval.as_ref().unwrap().source(), "PT10S");
            }
            other => panic!("expected Retry::Constant, got {other:?}"),
        }

        let exponential = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: exponential\n  interval: PT1S\n  maxInterval: PT30S\n  behavior: CREATE_NEW_EXECUTION\n";
        let t: Task = serde_norway::from_str(exponential).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Exponential {
                common,
                max_interval,
                ..
            } => {
                assert_eq!(common.behavior, RetryBehavior::CreateNewExecution);
                assert_eq!(max_interval.as_ref().unwrap().source(), "PT30S");
            }
            other => panic!("expected Retry::Exponential, got {other:?}"),
        }

        let random = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: random\n  minInterval: PT1S\n  maxInterval: PT5S\n";
        let t: Task = serde_norway::from_str(random).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Random { min_interval, .. } => {
                assert_eq!(min_interval.as_ref().unwrap().source(), "PT1S");
            }
            other => panic!("expected Retry::Random, got {other:?}"),
        }
    }

    // -- Review round 1, Finding 4: @NotNull is bean validation, not parsing --

    /// Task 1's fix made `Property::deserialize` reject `null` for a bare
    /// `Property<T>` -- correct in isolation, but it silently broke every
    /// bare (non-`Option`) `Property<T>` field in this file, because
    /// Kestra's `@NotNull` is bean validation (checked when a flow is
    /// *validated*), not a Jackson parse-time constraint. An explicit
    /// `concurrent: null` (or `value: null`, `condition: null`, `values:
    /// null`, any retry interval `: null`, ...) must still parse; only
    /// Task 4's semantic validator may reject it.
    #[test]
    fn explicit_null_still_parses_on_notnull_property_fields() {
        let yaml = "id: fan\ntype: core.parallel\nconcurrent: null\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Parallel(p) => assert!(
                p.concurrent.is_none(),
                "explicit null must clear the field, not error"
            ),
            other => panic!("expected Parallel, got {other:?}"),
        }

        // Omitting the key entirely still gets Kestra's own default (0) --
        // `null` and "absent" are NOT the same thing when the Java field
        // has a `@Builder.Default`.
        let yaml_omitted = "id: fan\ntype: core.parallel\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t2: Task = serde_norway::from_str(yaml_omitted).unwrap();
        match &t2.kind {
            TaskKind::Parallel(p) => assert_eq!(p.concurrent.as_ref().unwrap().source(), "0"),
            other => panic!("expected Parallel, got {other:?}"),
        }

        // Switch::value has no Java default at all, so absent and explicit
        // null both correctly land on None -- Kestra doesn't distinguish
        // them either once a field has no initializer.
        let switch_null = "id: s\ntype: core.switch\nvalue: null\ndefaults:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t3: Task = serde_norway::from_str(switch_null).unwrap();
        match &t3.kind {
            TaskKind::Switch(s) => assert!(s.value.is_none()),
            other => panic!("expected Switch, got {other:?}"),
        }
    }
}
