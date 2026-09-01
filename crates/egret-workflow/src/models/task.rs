// kestra: core/src/main/java/io/kestra/core/models/tasks/Task.java
// kestra: core/src/main/java/io/kestra/core/models/tasks/retrys/
// kestra: core/src/main/java/io/kestra/plugin/core/flow/

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::models::Property;

/// A task node in a flow. Control flow is typed (`TaskKind`'s non-`Leaf`
/// variants); everything else is an opaque `Leaf` -- a state machine that
/// knows what `egret.sql` is has stopped being a state machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    #[serde(flatten)]
    pub kind: TaskKind,
}

/// Control flow the executor must see into, versus the opaque leaf it must
/// not. Variant names and the types they carry are Kestra's own names for
/// the classes actually present in this checkout's
/// `core/src/main/java/io/kestra/plugin/core/flow/` -- verified by reading
/// that directory, not assumed from the W1 plan, which named this type
/// `ForEach` (no such class exists here; Kestra's own name is `Loop`) and
/// omitted `LoopUntil` entirely (see the task report for both corrections).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parallel {
    pub tasks: Vec<Task>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Property<i32>,
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

// kestra: Switch.java.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    pub value: Property<String>,
    #[serde(default)]
    pub cases: BTreeMap<String, Vec<Task>>,
    #[serde(default)]
    pub defaults: Vec<Task>,
}

// kestra: Dag.java -- `tasks` is a list of `(task, dependsOn)` pairs, not a
// plain task list, and it carries the same `concurrent` field as `Parallel`
// (Kestra's field, not this task's invention -- `Dag.java` line 98).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dag {
    pub tasks: Vec<DagNode>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Property<i32>,
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
// field in this crate already gets from `Property`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loop {
    pub tasks: Vec<Task>,
    pub values: Property<serde_json::Value>,
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: i32,
}

// kestra: LoopUntil.java -- a second `AbstractBranch` sibling of `Loop` this
// checkout has that the plan's Step-1 reading did not list (it named only
// Sequential/Parallel/If/Switch/Dag/ForEach/Subflow/AllowFailure/
// WorkingDirectory). It runs `tasks` in a loop, evaluating `condition` after
// each iteration, so W3's `advance()` must walk its `tasks` exactly like
// `Loop`'s -- added here per this task's resolution: "if one exists that the
// brief omits, add it with a test."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopUntil {
    pub tasks: Vec<Task>,
    pub condition: Property<String>,
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

fn default_concurrent() -> Property<i32> {
    Property::from_source("0".to_string())
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Retry {
    Constant {
        #[serde(flatten)]
        common: RetryCommon,
        interval: Property<Duration>,
    },
    #[serde(rename_all = "camelCase")]
    Exponential {
        #[serde(flatten)]
        common: RetryCommon,
        interval: Property<Duration>,
        max_interval: Property<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_factor: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Random {
        #[serde(flatten)]
        common: RetryCommon,
        min_interval: Property<Duration>,
        max_interval: Property<Duration>,
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
    use super::{Task, TaskKind};

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
                    assert_eq!(l.values.source(), "[1,2,3]", "{spelling}");
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
                assert_eq!(l.condition.source(), "{{ outputs.check.value == 'ready' }}");
                assert_eq!(t.children().len(), 1);
            }
            other => panic!("expected LoopUntil, got {other:?}"),
        }
    }
}
