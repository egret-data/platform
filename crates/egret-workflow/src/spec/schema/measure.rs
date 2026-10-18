//! W7a Task 1 Step 2 (and its fix round 1): how egret treats each property
//! Kestra documents -- on every type egret accepts that has a Kestra
//! definition, on the flow document, and on the nested definitions those
//! reach that egret parses (inputs, retry, the trigger and flow sub-objects).
//! The result is `property-treatment.json`, which `super::build` serves from;
//! the test below re-measures it on every run and fails when the committed
//! file no longer matches. `EGRET_WRITE_PROPERTY_TREATMENT=1` rewrites it.
//!
//! # The instrument, per property
//!
//! Each row is an object at a [`Site`]: a minimal flow that holds it, built
//! with and without the property, through three stages:
//!
//! 1. **parse + validate** -- a parse failure or a new validation error;
//! 2. **the later stage**, where egret has one that refuses by name:
//!    `ScheduleTrigger::resolve` for a Schedule trigger,
//!    `trigger::webhook::declared_unsupported` and `build_output` for a
//!    Webhook (the 409 every firing would answer);
//! 3. **what egret keeps** -- the *read set*:
//!    * a type row: the base keys `models::Task` reads, plus the fields of the
//!      struct its handler deserializes the leaf spec into (serde's own list,
//!      [`super::wire_fields_of`]), or of its typed model struct (flowables,
//!      triggers), or `ScriptKind::implemented_properties`, or the catalog's
//!      properties (plugins); for a trigger, the base fields its own firing
//!      path reads ([`trigger_base_read`]);
//!    * a nested row egret models with a struct: whether the key is still
//!      there after `parse` and re-serialising the parsed `Flow`
//!      ([`Read::Kept`]) -- serde drops every key the struct lacks;
//!    * a nested row a handler deserializes `deny_unknown_fields`
//!      ([`Read::Strict`]): its fields, and anything else is refused.
//!
//! | stage 1 | stage 2 | read | treatment |
//! |---|---|---|---|
//! | any | new refusal | any | `refused` |
//! | new error refusing the property by name | -- | any | `refused` |
//! | fails | -- | no | `refused` |
//! | new `not_yet_enforced` warning | -- | any | `warned` |
//! | clean | clean | yes | `honoured` |
//! | clean | clean | no | `ignored` |
//!
//! A read property whose sample fails stage 1 stays `honoured` and is
//! printed, so a bad sample is visible rather than turning it into `refused`
//! -- **unless** the new error is one of [`REFUSED_BY_NAME`] at the
//! property's own path. Such an error is not about the sample's value: it
//! refuses the property whatever it holds. `Pause`'s `behavior`, `onPause`
//! and `errors` are the case (2026-10-11 ruling): `models::Pause` reads them,
//! so they are in the read set, and `spec::validate` refuses each by name
//! through `executor::pause_gap`.
//!
//! **Enum values.** For each served property whose schema has an `enum`,
//! each value is tried alone; a value that fails to parse, or that stage 2
//! refuses, is listed in `refused_values` and removed from the served enum
//! (Webhook `fetchType: STORE`). Validation errors are not counted for
//! values: they are cross-field (`mode: AT_LEAST` without `minSatisfied`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::{
    wire_fields_of as fields_of, Measured, Treatment, FLOWABLE_TYPES, FLOW_ROW, TRIGGER_TYPES,
};
use crate::models::{self, Flow, TriggerKind};
use crate::spec::{parse, registry, validate};
use crate::worker::handlers as h;

/// Properties egret reads and that, by Kestra's own definition, do nothing:
/// the script tasks' `warningOnStdErr` ("Not used anymore, will be removed
/// soon"; W5a Task 1 measured no effect with `true`, `false` or unset).
const NO_EFFECT: &[&str] = &["warningOnStdErr"];

/// Validation error codes that refuse a property by NAME, whatever its value,
/// so a read property drawing one at its own path is `refused` rather than a
/// bad sample. `unsupported_pause_property` is `spec::validate`'s refusal of a
/// `Pause` declaring a property `executor::pause_gap` names (the user's ruling
/// of 2026-10-11): the model parses it, egret does not run it.
const REFUSED_BY_NAME: &[&str] = &["unsupported_pause_property"];

/// The base trigger fields each trigger's own firing path reads, measured by
/// searching that path for each `models::Trigger` field (fix round 1):
///
/// * every trigger: `id`, `type`, `description` (documentation), `disabled`
///   (`scheduler::sync`, `webhook_routes`, `trigger::flow`);
/// * `Flow` alone also reads `when` and `labels` (`trigger::flow`
///   `when_source`/`labels`). A Schedule refuses any `when` but `"true"`
///   (stage 2); a Webhook's path reads neither.
///
/// `stopAfter`, `logToFile`, `failOnTriggerError` and `allowConcurrent` are
/// parsed by `models::Trigger` and read by nothing.
fn trigger_base_read(t: &str) -> Vec<&'static str> {
    let mut v = vec!["description", "disabled", "id", "type"];
    if t == "io.kestra.plugin.core.trigger.Flow" {
        v.extend(["labels", "when"]);
    }
    v
}

fn task_model() -> BTreeSet<&'static str> {
    crate::plugins::HANDLED_BY_TASK_MODEL
        .iter()
        .copied()
        .collect()
}

/// The fields a core leaf task's handler deserializes, or `None` for a type
/// with no such handler.
fn leaf_handler_fields(t: &str) -> Option<Vec<&'static str>> {
    use h::storage::items;
    Some(match t {
        h::log::TYPE_ID => fields_of::<h::log::Spec>(),
        h::ret::TYPE_ID => fields_of::<h::ret::Spec>(),
        h::fail::TYPE_ID => fields_of::<h::fail::Spec>(),
        h::sleep::TYPE_ID => fields_of::<h::sleep::Spec>(),
        // Group C1 Task 2.
        h::output_values::TYPE_ID => fields_of::<h::output_values::Spec>(),
        h::storage::WRITE => fields_of::<h::storage::WriteSpec>(),
        h::storage::SIZE => fields_of::<h::storage::UriSpec>(),
        h::storage::DELETE => fields_of::<h::storage::DeleteSpec>(),
        // Reads no property: it purges the running execution's own files.
        h::storage::PURGE_CURRENT_EXECUTION_FILES => Vec::new(),
        h::storage::CONCAT => fields_of::<items::ConcatSpec>(),
        h::storage::SPLIT => fields_of::<items::SplitSpec>(),
        h::storage::REVERSE => fields_of::<items::ReverseSpec>(),
        h::storage::DEDUPLICATE_ITEMS => fields_of::<items::DeduplicateSpec>(),
        h::storage::FILTER_ITEMS => fields_of::<items::FilterSpec>(),
        h::namespace::UPLOAD_FILES => fields_of::<h::namespace::UploadSpec>(),
        h::namespace::DOWNLOAD_FILES => fields_of::<h::namespace::DownloadSpec>(),
        h::namespace::DELETE_FILES => fields_of::<h::namespace::DeleteSpec>(),
        h::namespace::PURGE_FILES => fields_of::<h::namespace::PurgeSpec>(),
        h::kv::SET => fields_of::<h::kv::SetSpec>(),
        h::kv::GET => fields_of::<h::kv::GetSpec>(),
        h::kv::GET_KEYS => fields_of::<h::kv::GetKeysSpec>(),
        h::kv::DELETE => fields_of::<h::kv::DeleteSpec>(),
        h::kv::PUT => fields_of::<h::kv::PutSpec>(),
        h::kv::PURGE_KV => fields_of::<h::kv::PurgeSpec>(),
        // Kill-and-exit Task 3: no handler -- the executor runs it -- and
        // `build_plan` reads its leaf spec through this struct.
        crate::executor::exit::TYPE_ID => fields_of::<crate::executor::exit::Spec>(),
        _ => return None,
    })
}

fn flowable_fields(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.flow.Sequential" => fields_of::<models::Sequential>(),
        "io.kestra.plugin.core.flow.Parallel" => fields_of::<models::Parallel>(),
        "io.kestra.plugin.core.flow.If" => fields_of::<models::If>(),
        "io.kestra.plugin.core.flow.Switch" => fields_of::<models::Switch>(),
        "io.kestra.plugin.core.flow.Dag" => fields_of::<models::Dag>(),
        "io.kestra.plugin.core.flow.Loop" => fields_of::<models::Loop>(),
        "io.kestra.plugin.core.flow.LoopUntil" => fields_of::<models::LoopUntil>(),
        "io.kestra.plugin.core.flow.Subflow" => fields_of::<models::Subflow>(),
        "io.kestra.plugin.core.flow.AllowFailure" => fields_of::<models::AllowFailure>(),
        "io.kestra.plugin.core.flow.WorkingDirectory" => fields_of::<models::WorkingDirectory>(),
        "io.kestra.plugin.core.flow.Pause" => fields_of::<models::Pause>(),
        other => panic!("no typed model for flowable {other}"),
    }
}

fn trigger_fields(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.trigger.Schedule" => fields_of::<models::Schedule>(),
        "io.kestra.plugin.core.trigger.Webhook" => fields_of::<models::Webhook>(),
        "io.kestra.plugin.core.trigger.Flow" => fields_of::<models::FlowTrigger>(),
        other => panic!("no typed model for trigger {other}"),
    }
}

// ---------- stages ----------

#[derive(Debug, PartialEq)]
struct Outcome {
    parsed: bool,
    errors: BTreeSet<(String, String)>,
    not_yet_enforced: BTreeSet<String>,
    /// Stage 2's refusal, if any.
    downstream: Option<String>,
    /// The re-serialised parsed flow, for [`Read::Kept`].
    plan: Option<Value>,
}

/// Stage 2 for every trigger in `flow`: the first refusal, in its own words.
fn downstream(flow: &Flow) -> Option<String> {
    for t in &flow.triggers {
        match &t.kind {
            TriggerKind::Schedule(_) => {
                if let Err(e) = crate::trigger::schedule::ScheduleTrigger::resolve(t) {
                    return Some(format!("trigger::schedule::ScheduleTrigger::resolve: {e}"));
                }
            }
            TriggerKind::Webhook(w) => {
                if let Some(u) = crate::trigger::webhook::declared_unsupported(w) {
                    return Some(format!(
                        "trigger::webhook::declared_unsupported (409 at firing): {}",
                        u.message(&t.id)
                    ));
                }
                let req = crate::trigger::webhook::WebhookRequest {
                    query: Vec::new(),
                    headers: Vec::new(),
                    content_type: Some("application/json".into()),
                    body: b"{}".to_vec(),
                };
                if let Err(u) = crate::trigger::webhook::build_output(&req, w.fetch_type, None) {
                    return Some(format!(
                        "trigger::webhook::build_output (409 at firing): {}",
                        u.message(&t.id)
                    ));
                }
            }
            _ => {}
        }
    }
    None
}

fn run(flow: &Value) -> Outcome {
    let source = serde_json::to_string(flow).expect("a Value serialises");
    match parse(&source) {
        Err(_) => Outcome {
            parsed: false,
            errors: BTreeSet::new(),
            not_yet_enforced: BTreeSet::new(),
            downstream: None,
            plan: None,
        },
        Ok(f) => {
            let r = validate(&f);
            Outcome {
                parsed: true,
                errors: r
                    .errors
                    .iter()
                    .map(|e| (e.code.to_string(), e.path.clone()))
                    .collect(),
                not_yet_enforced: r
                    .warnings
                    .iter()
                    .filter(|w| w.code == "not_yet_enforced")
                    .map(|w| w.path.clone())
                    .collect(),
                downstream: downstream(&f),
                plan: serde_json::to_value(&f).ok(),
            }
        }
    }
}

fn log_task(id: &str) -> Value {
    json!({ "id": id, "type": "io.kestra.plugin.core.log.Log", "message": "m" })
}

/// A value for `prop` that egret's model can hold. Hand-picked where a
/// generic one would not parse or would not show a warning; otherwise the
/// schema's own default or first enum value, else by its type.
fn sample(row: &str, prop: &str, schema: &Value) -> Value {
    match (row, prop) {
        (FLOW_ROW, "concurrency") => return json!({ "limit": 1 }),
        (FLOW_ROW, "sla") => {
            return json!([{ "id": "s", "type": "MAX_DURATION", "behavior": "FAIL", "duration": "PT1H" }])
        }
        (FLOW_ROW, "checks") => return json!([{ "when": "{{ true }}", "message": "m" }]),
        (FLOW_ROW, "quotas") => {
            return json!([{ "duration": "PT1H", "limit": 1, "behavior": "FAIL" }])
        }
        (FLOW_ROW, "triggers") => {
            return json!([{ "id": "s", "type": "io.kestra.plugin.core.trigger.Schedule", "cron": "0 * * * *" }])
        }
        (FLOW_ROW, "tasks" | "errors" | "finally" | "afterExecution") => {
            return json!([log_task("x")])
        }
        (FLOW_ROW, "inputs") => return json!([{ "id": "i", "type": "STRING" }]),
        (FLOW_ROW, "outputs") => return json!([{ "id": "o", "type": "STRING", "value": "x" }]),
        ("io.kestra.plugin.core.flow.Dag", "tasks") => return json!([{ "task": log_task("x") }]),
        (_, "namespaceFiles") => return json!({ "enabled": true }),
        (_, "tasks" | "errors" | "finally" | "then" | "else" | "defaults")
            if row.contains(".flow.") =>
        {
            return json!([log_task("x")])
        }
        ("io.kestra.plugin.core.flow.Pause", "onPause") => return log_task("x"),
        ("io.kestra.plugin.core.flow.Pause", "behavior") => return json!("RESUME"),
        ("io.kestra.plugin.core.flow.Switch", "cases") => return json!({ "a": [log_task("x")] }),
        ("io.kestra.plugin.core.flow.LoopUntil", "checkFrequency") => {
            return json!({ "maxIterations": 1 })
        }
        // Realistic values: stage 2 resolves the whole trigger, so a bad
        // sample here would read as a refusal of the property.
        ("io.kestra.plugin.core.trigger.Schedule", "cron") => return json!("0 * * * *"),
        ("io.kestra.plugin.core.trigger.Schedule", "timezone") => return json!("UTC"),
        ("io.kestra.plugin.core.trigger.Schedule", "withSeconds") => return json!(false),
        ("io.kestra.plugin.core.trigger.Schedule", "lateMaximumDelay") => return json!("PT1H"),
        ("io.kestra.plugin.core.trigger.Flow", "states") => return json!(["SUCCESS"]),
        ("io.kestra.plugin.core.trigger.Flow", "dependsOn") => {
            return json!([{ "namespace": "a.b", "flowId": "f" }])
        }
        ("io.kestra.plugin.core.trigger.Flow", "window") => return json!({ "lookback": "PT1H" }),
        (_, "stopAfter") => return json!(["SUCCESS"]),
        (_, "labels") => return json!({ "k": "v" }),
        (_, "retry") => return json!({ "type": "constant", "interval": "PT1S", "maxAttempts": 1 }),
        (_, "dependsOn") if row.contains(".flows.input.") => return json!({ "inputs": ["a"] }),
        (_, "dependsOn") if row.ends_with("Dag-DagTask") => return json!(["x"]),
        (_, "task") => return log_task("x"),
        _ => {}
    }
    if let Some(e) = enum_values(schema).first() {
        return if is_array(schema) {
            json!([e])
        } else {
            e.clone()
        };
    }
    let ty = schema
        .get("type")
        .or_else(|| schema.pointer("/allOf/0/type"))
        .or_else(|| schema.pointer("/anyOf/0/type"))
        .and_then(Value::as_str);
    match ty {
        Some("boolean") => json!(true),
        Some("integer") | Some("number") => json!(1),
        Some("array") => json!(["x"]),
        Some("object") => json!({ "x": "x" }),
        _ => json!("x"),
    }
}

/// A property schema's `enum`, wherever `remove_refused_values` looks for it.
fn enum_values(schema: &Value) -> Vec<Value> {
    fn walk(v: &Value, out: &mut Vec<Value>) {
        let Value::Object(m) = v else { return };
        if let Some(Value::Array(e)) = m.get("enum") {
            out.extend(e.iter().cloned());
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(Value::Array(parts)) = m.get(key) {
                parts.iter().for_each(|p| walk(p, out));
            }
        }
        if let Some(items) = m.get("items") {
            walk(items, out);
        }
    }
    let mut out = Vec::new();
    walk(schema, &mut out);
    out.dedup();
    out
}

fn is_array(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("array")
        || schema.pointer("/allOf/0/type").and_then(Value::as_str) == Some("array")
}

// ---------- sites ----------

/// What egret keeps of a row.
enum Read {
    /// These keys.
    Set(BTreeSet<&'static str>),
    /// These keys, and the handler refuses any other (`deny_unknown_fields`
    /// on a struct read at run time, which no earlier stage checks).
    Strict(BTreeSet<&'static str>),
    /// Whatever is still at this JSON pointer once the parsed `Flow` is
    /// serialised again.
    Kept(&'static str),
}

/// Where a row's object sits in a minimal flow.
struct Site {
    row: String,
    props: BTreeMap<String, Value>,
    /// The object without the property under test.
    base: Map<String, Value>,
    /// The whole flow holding `object`.
    place: Box<dyn Fn(Value) -> Value>,
    read: Read,
}

fn flow_with(extra: &[(&str, Value)]) -> Value {
    let mut f = json!({ "id": "m", "namespace": "m.t", "tasks": [log_task("base")] });
    for (k, v) in extra {
        f[*k] = v.clone();
    }
    f
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("a site's base is an object"),
    }
}

fn props_of(def: &Value) -> BTreeMap<String, Value> {
    def.get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn kestra_props(name: &str) -> BTreeMap<String, Value> {
    super::kestra_definition(name)
        .map(props_of)
        .unwrap_or_default()
}

fn task_site(row: &str, props: BTreeMap<String, Value>, read: BTreeSet<&'static str>) -> Site {
    Site {
        row: row.to_string(),
        props,
        base: obj(json!({ "id": "t", "type": row })),
        place: Box::new(|o| flow_with(&[("tasks", json!([o]))])),
        read: Read::Set(read),
    }
}

fn trigger_site(row: &str, read: BTreeSet<&'static str>) -> Site {
    let mut base = json!({ "id": "t", "type": row });
    match row {
        "io.kestra.plugin.core.trigger.Schedule" => base["cron"] = json!("0 * * * *"),
        "io.kestra.plugin.core.trigger.Webhook" => base["key"] = json!("k"),
        _ => {}
    }
    Site {
        row: row.to_string(),
        props: kestra_props(row),
        base: obj(base),
        place: Box::new(|o| flow_with(&[("triggers", json!([o]))])),
        read: Read::Set(read),
    }
}

/// A nested definition egret models with a struct, at `pointer`.
fn nested(
    row: &str,
    base: Value,
    place: impl Fn(Value) -> Value + 'static,
    pointer: &'static str,
) -> Site {
    Site {
        row: row.to_string(),
        props: kestra_props(row),
        base: obj(base),
        place: Box::new(place),
        read: Read::Kept(pointer),
    }
}

/// Every nested Kestra definition the served schema can reach once the
/// type rows are filtered, except the ones the README lists as passed
/// through (unions, `java.lang.Void`, `EncryptedString`, `ValueOption`).
fn nested_sites() -> Vec<Site> {
    const IN: &str = "io.kestra.core.models.flows.input.";
    let mut sites = Vec::new();
    // Inputs: one row per Kestra input definition, at `inputs[0]`.
    let input_defs: Vec<(String, Value)> = super::INPUTS
        .kestra_defs
        .iter()
        .filter(|(k, _)| k.starts_with(IN))
        .filter_map(|(k, d)| {
            let ty = d.pointer("/properties/type")?;
            let t = ty
                .get("const")
                .cloned()
                .or_else(|| ty.pointer("/enum/0").cloned())?;
            Some((k.clone(), t))
        })
        .collect();
    for (row, t) in input_defs {
        sites.push(nested(
            &row,
            json!({ "id": "i", "type": t }),
            |o| flow_with(&[("inputs", json!([o]))]),
            "/inputs/0",
        ));
    }
    sites.push(nested(
        "io.kestra.core.models.flows.DependsOn",
        json!({ "inputs": ["a"] }),
        |o| {
            flow_with(&[(
                "inputs",
                json!([{ "id": "a", "type": "STRING" }, { "id": "i", "type": "STRING", "dependsOn": o }]),
            )])
        },
        "/inputs/1/dependsOn",
    ));
    // Retry, on a task.
    for (row, base) in [
        (
            "io.kestra.core.models.tasks.retrys.Constant-2",
            json!({ "type": "constant", "interval": "PT1S", "maxAttempts": 1 }),
        ),
        (
            "io.kestra.core.models.tasks.retrys.Exponential-2",
            json!({ "type": "exponential", "interval": "PT1S", "maxInterval": "PT1M", "maxAttempts": 1 }),
        ),
        (
            "io.kestra.core.models.tasks.retrys.Random-2",
            json!({ "type": "random", "minInterval": "PT1S", "maxInterval": "PT2S", "maxAttempts": 1 }),
        ),
    ] {
        sites.push(nested(
            row,
            base,
            |o| {
                let mut t = log_task("t");
                t["retry"] = o;
                flow_with(&[("tasks", json!([t]))])
            },
            "/tasks/0/retry",
        ));
    }
    // The flow's own sub-objects.
    sites.push(nested(
        "io.kestra.core.models.flows.Concurrency",
        json!({ "limit": 1 }),
        |o| flow_with(&[("concurrency", o)]),
        "/concurrency",
    ));
    sites.push(nested(
        "io.kestra.core.models.flows.check.Check",
        json!({ "when": "{{ true }}", "message": "m" }),
        |o| flow_with(&[("checks", json!([o]))]),
        "/checks/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.flows.quota.Quota",
        json!({ "duration": "PT1H", "limit": 1, "behavior": "FAIL" }),
        |o| flow_with(&[("quotas", json!([o]))]),
        "/quotas/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.flows.sla.types.MaxDurationSLA-2",
        json!({ "id": "s", "type": "MAX_DURATION", "behavior": "FAIL", "duration": "PT1H" }),
        |o| flow_with(&[("sla", json!([o]))]),
        "/sla/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.flows.sla.types.ExecutionAssertionSLA-2",
        json!({ "id": "s", "type": "EXECUTION_ASSERTION", "behavior": "FAIL", "assert": "{{ true }}" }),
        |o| flow_with(&[("sla", json!([o]))]),
        "/sla/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.flows.Output",
        json!({ "id": "o", "type": "STRING", "value": "x" }),
        |o| flow_with(&[("outputs", json!([o]))]),
        "/outputs/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.Label",
        json!({ "key": "k", "value": "v" }),
        |o| flow_with(&[("labels", json!([o]))]),
        "/labels/0",
    ));
    // The Flow trigger's sub-objects.
    sites.push(nested(
        "io.kestra.plugin.core.trigger.Flow-Dependency",
        json!({ "namespace": "a.b", "flowId": "f" }),
        |o| {
            flow_with(&[(
                "triggers",
                json!([{ "id": "g", "type": "io.kestra.plugin.core.trigger.Flow", "dependsOn": [o] }]),
            )])
        },
        "/triggers/0/dependsOn/0",
    ));
    sites.push(nested(
        "io.kestra.core.models.triggers.Window",
        json!({ "lookback": "PT1H" }),
        |o| {
            flow_with(&[(
                "triggers",
                json!([{
                    "id": "g", "type": "io.kestra.plugin.core.trigger.Flow",
                    "dependsOn": [{ "namespace": "a.b", "flowId": "f" }], "window": o,
                }]),
            )])
        },
        "/triggers/0/window",
    ));
    // The flowables' sub-objects.
    sites.push(nested(
        "io.kestra.plugin.core.flow.Dag-DagTask",
        json!({ "task": log_task("x") }),
        |o| {
            flow_with(&[(
                "tasks",
                json!([{ "id": "d", "type": "io.kestra.plugin.core.flow.Dag", "tasks": [o] }]),
            )])
        },
        "/tasks/0/tasks/0",
    ));
    sites.push(nested(
        "io.kestra.plugin.core.flow.LoopUntil-CheckFrequency",
        json!({ "maxIterations": 1 }),
        |o| {
            flow_with(&[(
                "tasks",
                json!([{
                    "id": "l", "type": "io.kestra.plugin.core.flow.LoopUntil",
                    "condition": "{{ true }}", "tasks": [log_task("x")], "checkFrequency": o,
                }]),
            )])
        },
        "/tasks/0/checkFrequency",
    ));
    // `namespaceFiles` on a script task: `NamespaceFilesSpec::parse`, which
    // validate runs, refuses an unknown key by name.
    sites.push(Site {
        row: "io.kestra.core.models.tasks.NamespaceFiles".into(),
        props: kestra_props("io.kestra.core.models.tasks.NamespaceFiles"),
        base: obj(json!({ "enabled": true })),
        place: Box::new(|o| {
            flow_with(&[(
                "tasks",
                json!([{
                    "id": "s", "type": crate::scripts::SHELL_COMMANDS,
                    "commands": ["true"], "namespaceFiles": o,
                }]),
            )])
        }),
        read: Read::Set(
            crate::storage::nsfiles::stage::PROPERTIES
                .iter()
                .copied()
                .collect(),
        ),
    });
    // The purge tasks' `behavior`: deserialized by the handler at run time,
    // `deny_unknown_fields`.
    for (row, fields) in [
        (
            "io.kestra.plugin.core.kv.Key-2",
            fields_of::<h::kv::BehaviorSpec>(),
        ),
        (
            "io.kestra.plugin.core.kv.Version-2",
            fields_of::<h::kv::BehaviorSpec>(),
        ),
        (
            "io.kestra.plugin.core.namespace.Version-2",
            fields_of::<h::namespace::BehaviorSpec>(),
        ),
    ] {
        sites.push(Site {
            row: row.into(),
            props: kestra_props(row),
            base: Map::new(),
            place: Box::new(|_| flow_with(&[])),
            read: Read::Strict(fields.into_iter().collect()),
        });
    }
    sites
}

fn with(base: &Map<String, Value>, prop: &str, v: Value) -> Value {
    let mut m = base.clone();
    m.insert(prop.to_string(), v);
    Value::Object(m)
}

#[derive(Default)]
struct RowResult {
    props: BTreeMap<String, Treatment>,
    values: BTreeMap<String, Vec<Value>>,
    downstream: BTreeMap<String, String>,
}

fn classify(site: &Site, notes: &mut Vec<String>) -> RowResult {
    let mut out = RowResult::default();
    if let Read::Strict(read) = &site.read {
        for p in site.props.keys() {
            let t = if read.contains(p.as_str()) {
                Treatment::Honoured
            } else {
                Treatment::Refused
            };
            out.props.insert(p.clone(), t);
        }
        return out;
    }
    let baseline = run(&(site.place)(Value::Object(site.base.clone())));
    if !baseline.parsed {
        notes.push(format!("{}: the base object does not parse", site.row));
    }
    for (prop, schema) in &site.props {
        // The identity keys: replacing them changes every finding's path, and
        // the discriminator selects the row itself.
        if site.row != FLOW_ROW && (prop == "type" || prop == "id") {
            out.props.insert(prop.clone(), Treatment::Honoured);
            continue;
        }
        let value = sample(&site.row, prop, schema);
        let o = run(&(site.place)(with(&site.base, prop, value.clone())));
        let new_errors: Vec<_> = o.errors.difference(&baseline.errors).collect();
        let warned = o
            .not_yet_enforced
            .difference(&baseline.not_yet_enforced)
            .next()
            .is_some();
        let is_read = match &site.read {
            Read::Set(set) | Read::Strict(set) => set.contains(prop.as_str()),
            Read::Kept(ptr) => o
                .plan
                .as_ref()
                .and_then(|p| p.pointer(ptr))
                .and_then(Value::as_object)
                .is_some_and(|m| m.contains_key(prop)),
        };
        let refused_by_name = new_errors.iter().any(|(code, path)| {
            REFUSED_BY_NAME.contains(&code.as_str()) && path.ends_with(&format!(".{prop}"))
        });
        let t = if o.downstream.is_some() && o.downstream != baseline.downstream {
            out.downstream
                .insert(prop.clone(), o.downstream.clone().unwrap_or_default());
            Treatment::Refused
        } else if refused_by_name {
            Treatment::Refused
        } else if !o.parsed || !new_errors.is_empty() {
            if is_read || matches!(site.read, Read::Kept(_)) && o.parsed {
                notes.push(format!(
                    "{}.{prop}: read, but the sample {value} did not pass ({}; {new_errors:?})",
                    site.row,
                    if o.parsed { "parsed" } else { "parse failed" }
                ));
                if is_read {
                    Treatment::Honoured
                } else {
                    Treatment::Refused
                }
            } else {
                Treatment::Refused
            }
        } else if warned {
            Treatment::Warned
        } else if is_read {
            Treatment::Honoured
        } else {
            Treatment::Ignored
        };
        let t = if NO_EFFECT.contains(&prop.as_str()) && t == Treatment::Honoured {
            Treatment::Ignored
        } else {
            t
        };
        out.props.insert(prop.clone(), t);

        // Enum values, on what is served.
        if !matches!(t, Treatment::Honoured | Treatment::Warned) {
            continue;
        }
        let mut refused = Vec::new();
        for e in enum_values(schema) {
            let v = if is_array(schema) {
                json!([e])
            } else {
                e.clone()
            };
            let o = run(&(site.place)(with(&site.base, prop, v)));
            let late = o.downstream.is_some() && o.downstream != baseline.downstream;
            if !o.parsed || late {
                if late {
                    out.downstream
                        .insert(format!("{prop}={e}"), o.downstream.unwrap_or_default());
                }
                refused.push(e);
            }
        }
        if !refused.is_empty() {
            out.values.insert(prop.clone(), refused);
        }
    }
    out
}

/// Every site: the types, the flow document, and the nested definitions.
fn sites() -> Vec<Site> {
    let base = task_model();
    let mut sites = Vec::new();
    for row in registry::TASK_TYPES {
        let props = kestra_props(row.r#type);
        if props.is_empty() {
            continue; // egret-only: hand-written, nothing of Kestra's to classify
        }
        let mut read = base.clone();
        if let Some(kind) = crate::scripts::kind_of(row.r#type) {
            read.extend(kind.implemented_properties());
        } else {
            let fields = leaf_handler_fields(row.r#type)
                .unwrap_or_else(|| panic!("{}: no handler field list", row.r#type));
            read.extend(fields);
        }
        sites.push(task_site(row.r#type, props, read));
    }
    for (fqcn, class) in &crate::plugins::catalog().classes {
        let name = format!("{}{fqcn}", crate::plugins::PREFIX);
        let mut props = kestra_props(fqcn);
        for (p, spec) in &class.properties {
            props
                .entry(p.clone())
                .or_insert_with(|| spec.schema.clone());
        }
        let mut read = base.clone();
        read.extend(class.properties.keys().map(String::as_str).filter(|p| {
            !crate::plugins::REFUSED_BASE_PROPERTIES.contains(p) && *p != "taskRunner"
        }));
        sites.push(task_site(&name, props, read));
    }
    for (fqcn, _) in FLOWABLE_TYPES {
        let mut read = base.clone();
        read.extend(flowable_fields(fqcn));
        sites.push(task_site(fqcn, kestra_props(fqcn), read));
    }
    for (fqcn, _) in TRIGGER_TYPES {
        let mut read: BTreeSet<&str> = trigger_base_read(fqcn).into_iter().collect();
        read.extend(trigger_fields(fqcn));
        sites.push(trigger_site(fqcn, read));
    }
    sites.push(Site {
        row: FLOW_ROW.into(),
        props: props_of(&super::INPUTS.kestra_flow),
        base: obj(flow_with(&[])),
        place: Box::new(|o| o),
        read: Read::Set(fields_of::<Flow>().into_iter().collect()),
    });
    sites.extend(nested_sites());
    sites
}

/// The whole measurement.
pub(super) fn measure(notes: &mut Vec<String>) -> Measured {
    let mut m = Measured::default();
    for site in sites() {
        let r = classify(&site, notes);
        m.properties.insert(site.row.clone(), r.props);
        if !r.values.is_empty() {
            m.refused_values.insert(site.row.clone(), r.values);
        }
        if !r.downstream.is_empty() {
            m.refused_downstream.insert(site.row.clone(), r.downstream);
        }
    }
    m
}

#[test]
fn the_committed_property_treatment_is_what_egret_does_today() {
    let mut notes = Vec::new();
    let measured = measure(&mut notes);
    for n in &notes {
        eprintln!("note: {n}");
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/spec/schema/property-treatment.json"
    );
    let text = format!("{}\n", serde_json::to_string_pretty(&measured).unwrap());
    if std::env::var_os("EGRET_WRITE_PROPERTY_TREATMENT").is_some() {
        std::fs::write(path, text).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(path).unwrap();
    assert!(
        committed == text,
        "property-treatment.json is stale; re-run with EGRET_WRITE_PROPERTY_TREATMENT=1 and \
         review the diff"
    );
    assert_eq!(
        serde_json::to_value(super::measured()).unwrap(),
        serde_json::to_value(&measured).unwrap(),
        "the compiled-in table is the committed file"
    );
}

/// The instrument itself: serde's field list is the renamed wire names, and
/// a property outside it really is dropped by the handler's struct.
#[test]
fn the_field_probe_reads_wire_names_and_unknown_keys_are_dropped() {
    let fields = fields_of::<h::fail::Spec>();
    assert_eq!(fields, vec!["condition", "errorMessage"]);
    let spec: h::fail::Spec =
        serde_json::from_value(json!({ "errorMessage": "x", "logLevel": "INFO" }))
            .expect("an unknown key is dropped, not refused");
    assert!(format!("{spec:?}").contains("error_message"));
}

/// Stage 2 is wired: the five trigger properties egret parses and then
/// refuses (fix round 1, I1) come out `refused`, each with its site.
#[test]
fn properties_refused_after_parse_are_measured_refused() {
    let m = super::measured();
    for (row, prop, site) in [
        (
            "io.kestra.plugin.core.trigger.Webhook",
            "wait",
            "declared_unsupported",
        ),
        (
            "io.kestra.plugin.core.trigger.Webhook",
            "returnOutputs",
            "declared_unsupported",
        ),
        (
            "io.kestra.plugin.core.trigger.Webhook",
            "responseContentType",
            "declared_unsupported",
        ),
        (
            "io.kestra.plugin.core.trigger.Webhook",
            "responseCode",
            "declared_unsupported",
        ),
        (
            "io.kestra.plugin.core.trigger.Schedule",
            "lateMaximumDelay",
            "ScheduleTrigger::resolve",
        ),
    ] {
        assert_eq!(
            m.properties[row].get(prop),
            Some(&Treatment::Refused),
            "{row}.{prop}"
        );
        assert!(
            m.refused_downstream[row][prop].contains(site),
            "{row}.{prop}"
        );
    }
    assert_eq!(
        m.refused_values["io.kestra.plugin.core.trigger.Webhook"]["fetchType"],
        vec![json!("STORE")]
    );
}
