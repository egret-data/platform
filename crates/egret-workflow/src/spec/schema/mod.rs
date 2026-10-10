//! The JSON schema of one flow document, as egret accepts it -- what
//! `GET /api/ws/{ws}/workflows/schema` serves to the flow editor
//! (W7a spec §7.1).
//!
//! # Three committed inputs (README.md has the commands)
//!
//! * `kestra-flow-schema.json` -- Kestra's own flow schema at 2.0.0-rc13,
//!   exported once from the local `kestra/kestra` image and trimmed by
//!   `trim.py` to the definitions egret can reach. Its task and trigger unions
//!   already point at `#/$defs/egret.task` and `#/$defs/egret.trigger`.
//! * `egret-task-schemas.json` -- hand-written schemas for egret's own types
//!   and for the script tasks' `taskRunner`.
//! * `property-treatment.json` -- for every Kestra property of every type
//!   egret accepts, whether egret honours, warns about, ignores or refuses it
//!   (Task 1's measurement; `measure.rs` re-measures it on every test run).
//!
//! # What is served
//!
//! * the types egret accepts and nothing else: the registry
//!   ([`registry::TASK_TYPES`]), the flowables of `models::task::kind_for`,
//!   the typed triggers of `models::trigger::kind_for`, and the catalog's
//!   plugin classes ([`crate::plugins::catalog`]), each under its canonical
//!   spelling with its aliases beside it;
//! * minus what the workspace's features turn off: the script types need
//!   `workflow_containers`, the plugin types need it **and**
//!   `workflow_kestra_plugins` -- the order the two publish gates check;
//! * of each Kestra type, only the properties egret honours or warns about.
//!   An ignored property is never offered (completion would suggest something
//!   egret silently drops), and neither is a refused one (it would suggest a
//!   save error).
//!
//! Each combination of [`SchemaFeatures`] is built once and kept: the inputs
//! are compiled in, so the answer cannot change while the process runs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, OnceLock};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::spec::registry;

#[cfg(test)]
mod measure;

const KESTRA_JSON: &str = include_str!("kestra-flow-schema.json");
const EGRET_JSON: &str = include_str!("egret-task-schemas.json");
const TREATMENT_JSON: &str = include_str!("property-treatment.json");

/// The workspace features the served schema depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SchemaFeatures {
    /// `workflow_kestra_plugins` is on.
    pub kestra_plugins: bool,
    /// `workflow_containers` is on.
    pub containers: bool,
}

impl SchemaFeatures {
    fn index(self) -> usize {
        usize::from(self.kestra_plugins) * 2 + usize::from(self.containers)
    }

    /// The plugin types are served only when publish could accept them:
    /// `plugins::gate::check` refuses a plugin task with
    /// `workflow_containers` off, whatever `workflow_kestra_plugins` says.
    fn plugins_served(self) -> bool {
        self.kestra_plugins && self.containers
    }
}

/// How egret treats one property Kestra documents on a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Treatment {
    /// Parsed and acted on.
    Honoured,
    /// Parsed, not acted on, and the validator says so with a
    /// `not_yet_enforced` warning. Served: the editor shows the warning, so
    /// nothing is dropped silently.
    Warned,
    /// Accepted and dropped without a word. Never served.
    Ignored,
    /// Refused at save or publish. Never served.
    Refused,
}

impl Treatment {
    fn served(self) -> bool {
        matches!(self, Treatment::Honoured | Treatment::Warned)
    }
}

/// `property-treatment.json`: type (or `Flow` for the flow document itself)
/// -> property -> treatment.
pub type TreatmentTable = BTreeMap<String, BTreeMap<String, Treatment>>;

/// The committed measurement.
pub fn property_treatment() -> &'static TreatmentTable {
    static TABLE: LazyLock<TreatmentTable> = LazyLock::new(|| {
        serde_json::from_str(TREATMENT_JSON)
            .unwrap_or_else(|e| panic!("spec/schema/property-treatment.json: {e}"))
    });
    &TABLE
}

/// The key of the flow document's own row in [`property_treatment`].
pub const FLOW_ROW: &str = "Flow";

/// The flowables `models::task::kind_for` dispatches to a typed `TaskKind`,
/// canonical FQCN first, then every other spelling it accepts.
/// `tests/flow_schema.rs` checks each against the parser.
pub const FLOWABLE_TYPES: &[(&str, &[&str])] = &[
    (
        "io.kestra.plugin.core.flow.Sequential",
        &["core.sequential"],
    ),
    ("io.kestra.plugin.core.flow.Parallel", &["core.parallel"]),
    ("io.kestra.plugin.core.flow.If", &["core.if"]),
    ("io.kestra.plugin.core.flow.Switch", &["core.switch"]),
    ("io.kestra.plugin.core.flow.Dag", &["core.dag"]),
    ("io.kestra.plugin.core.flow.Loop", &["core.loop"]),
    (
        "io.kestra.plugin.core.flow.LoopUntil",
        &[
            "io.kestra.plugin.core.flow.WaitFor",
            "core.loopuntil",
            "core.waitfor",
        ],
    ),
    ("io.kestra.plugin.core.flow.Subflow", &["core.subflow"]),
    (
        "io.kestra.plugin.core.flow.AllowFailure",
        &["core.allowfailure"],
    ),
    (
        "io.kestra.plugin.core.flow.WorkingDirectory",
        &["core.workingdirectory"],
    ),
    ("io.kestra.plugin.core.flow.Pause", &["core.pause"]),
];

/// The triggers `models::trigger::kind_for` dispatches to a typed
/// `TriggerKind`. Any other trigger type parses as an inert leaf that never
/// fires, so it is not offered.
pub const TRIGGER_TYPES: &[(&str, &[&str])] = &[
    ("io.kestra.plugin.core.trigger.Schedule", &["core.schedule"]),
    ("io.kestra.plugin.core.trigger.Webhook", &["core.webhook"]),
    ("io.kestra.plugin.core.trigger.Flow", &["core.flow"]),
];

/// The base task properties egret's `Task` model reads, served on every
/// egret-only type with Kestra's own schema for each (taken from
/// `io.kestra.plugin.core.log.Log`).
const BASE_TASK_PROPERTIES: &[&str] = crate::plugins::HANDLED_BY_TASK_MODEL;
const BASE_SOURCE: &str = "io.kestra.plugin.core.log.Log";

/// One served schema: the document, its bytes, and its strong `ETag`.
#[derive(Debug)]
pub struct ServedSchema {
    pub value: Value,
    pub bytes: Vec<u8>,
    /// `"sha256-<hex>"`, quoted, over [`ServedSchema::bytes`].
    pub etag: String,
}

/// The schema for `features`, built on first use and kept.
pub fn served(features: &SchemaFeatures) -> &'static ServedSchema {
    static CACHE: [OnceLock<ServedSchema>; 4] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    CACHE[features.index()].get_or_init(|| {
        let value = build(*features);
        let bytes = serde_json::to_vec(&value).expect("a serde_json::Value always serialises");
        let etag = format!("\"sha256-{}\"", hex::encode(Sha256::digest(&bytes)));
        ServedSchema { value, bytes, etag }
    })
}

/// The JSON schema of one flow document for a workspace with `features`.
pub fn flow_schema(features: &SchemaFeatures) -> Value {
    served(features).value.clone()
}

struct Inputs {
    kestra_defs: Map<String, Value>,
    kestra_flow: Value,
    egret_types: Map<String, Value>,
    egret_overrides: Map<String, Value>,
    egret_defs: Map<String, Value>,
}

static INPUTS: LazyLock<Inputs> = LazyLock::new(|| {
    let kestra: Value = serde_json::from_str(KESTRA_JSON)
        .unwrap_or_else(|e| panic!("spec/schema/kestra-flow-schema.json: {e}"));
    let egret: Value = serde_json::from_str(EGRET_JSON)
        .unwrap_or_else(|e| panic!("spec/schema/egret-task-schemas.json: {e}"));
    let obj = |v: &Value, key: &str| v.get(key).and_then(Value::as_object).cloned();
    Inputs {
        kestra_defs: obj(&kestra, "$defs").expect("kestra-flow-schema.json has $defs"),
        kestra_flow: kestra
            .get("flow")
            .cloned()
            .expect("kestra-flow-schema.json has flow"),
        egret_types: obj(&egret, "types").expect("egret-task-schemas.json has types"),
        egret_overrides: obj(&egret, "property_overrides").unwrap_or_default(),
        egret_defs: obj(&egret, "$defs").unwrap_or_default(),
    }
});

/// The Kestra definition of `fqcn` in the committed export, if any.
pub fn kestra_definition(fqcn: &str) -> Option<&'static Value> {
    INPUTS.kestra_defs.get(fqcn)
}

/// The Kestra version the committed export was taken from.
pub const KESTRA_VERSION: &str = "2.0.0-rc13";

fn build(features: SchemaFeatures) -> Value {
    let inputs = &*INPUTS;
    let mut defs = inputs.kestra_defs.clone();
    for (k, v) in &inputs.egret_defs {
        defs.insert(k.clone(), v.clone());
    }

    let mut tasks: Vec<String> = Vec::new();
    for row in registry::TASK_TYPES {
        if crate::scripts::kind_of(row.r#type).is_some() && !features.containers {
            continue;
        }
        let def = match inputs.kestra_defs.get(row.r#type) {
            Some(kestra) => kestra_type(row.r#type, row.aliases, kestra),
            None => egret_type(row.r#type, row.aliases),
        };
        defs.insert(row.r#type.to_string(), def);
        tasks.push(row.r#type.to_string());
    }
    if features.plugins_served() {
        for (fqcn, class) in &crate::plugins::catalog().classes {
            let name = format!("{}{fqcn}", crate::plugins::PREFIX);
            defs.insert(name.clone(), plugin_type(&name, fqcn, class));
            tasks.push(name);
        }
    }
    for (fqcn, aliases) in FLOWABLE_TYPES {
        let def = kestra_type(fqcn, aliases, &kestra_def(fqcn));
        defs.insert(fqcn.to_string(), def);
        tasks.push(fqcn.to_string());
    }
    let mut triggers: Vec<String> = Vec::new();
    for (fqcn, aliases) in TRIGGER_TYPES {
        let def = kestra_type(fqcn, aliases, &kestra_def(fqcn));
        defs.insert(fqcn.to_string(), def);
        triggers.push(fqcn.to_string());
    }
    defs.insert(
        "egret.task".into(),
        json!({
            "title": "A task",
            "oneOf": tasks.iter().map(|t| json!({ "$ref": format!("#/$defs/{t}") })).collect::<Vec<_>>(),
        }),
    );
    defs.insert(
        "egret.trigger".into(),
        json!({
            "title": "A trigger",
            "oneOf": triggers.iter().map(|t| json!({ "$ref": format!("#/$defs/{t}") })).collect::<Vec<_>>(),
        }),
    );

    let mut root = Map::new();
    root.insert(
        "$schema".into(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    root.insert("title".into(), json!("An egret flow"));
    root.insert(
        "x-egret-features".into(),
        json!({
            "workflow_kestra_plugins": features.kestra_plugins,
            "workflow_containers": features.containers,
        }),
    );
    root.insert("x-egret-kestra-version".into(), json!(KESTRA_VERSION));
    let mut flow = inputs.kestra_flow.clone();
    filter_properties(FLOW_ROW, &mut flow);
    if let Value::Object(f) = flow {
        for (k, v) in f {
            root.insert(k, v);
        }
    }
    let reachable = reachable_defs(&Value::Object(root.clone()), &defs);
    let defs: Map<String, Value> = defs
        .into_iter()
        .filter(|(k, _)| reachable.contains(k))
        .collect();
    root.insert("$defs".into(), Value::Object(defs));
    Value::Object(root)
}

fn kestra_def(fqcn: &str) -> Value {
    INPUTS
        .kestra_defs
        .get(fqcn)
        .cloned()
        .unwrap_or_else(|| panic!("{fqcn} is not in kestra-flow-schema.json; re-run trim.py"))
}

/// A `type` property naming `canonical`, with each alias as a second
/// accepted value whose description names the canonical spelling.
fn type_property(canonical: &str, aliases: &[&str], what: &str) -> Value {
    let mut members = vec![json!({
        "const": canonical,
        "description": what,
    })];
    for alias in aliases {
        members.push(json!({
            "const": alias,
            "description": format!("Alias of `{canonical}`: egret accepts both spellings."),
            "x-egret-alias-of": canonical,
        }));
    }
    json!({ "anyOf": members })
}

/// Keep only the properties [`property_treatment`] serves for `row`; a
/// property with no row is not served (fail-closed: an unmeasured property
/// is not known to be honoured). `required` follows.
fn filter_properties(row: &str, def: &mut Value) {
    let table = property_treatment().get(row);
    let Some(obj) = def.as_object_mut() else {
        return;
    };
    let mut kept: BTreeSet<String> = BTreeSet::new();
    if let Some(Value::Object(props)) = obj.get_mut("properties") {
        props.retain(|name, _| {
            let served = table.and_then(|t| t.get(name)).is_some_and(|t| t.served());
            if served {
                kept.insert(name.clone());
            }
            served
        });
    }
    if let Some(Value::Array(required)) = obj.get_mut("required") {
        required.retain(|r| r.as_str().is_some_and(|r| kept.contains(r)));
    }
}

fn kestra_type(canonical: &str, aliases: &[&str], kestra: &Value) -> Value {
    let mut def = kestra.clone();
    filter_properties(canonical, &mut def);
    if let Some(overrides) = INPUTS
        .egret_overrides
        .get(canonical)
        .and_then(Value::as_object)
    {
        if let Some(Value::Object(props)) = def.get_mut("properties") {
            for (name, schema) in overrides {
                if props.contains_key(name) {
                    props.insert(name.clone(), schema.clone());
                }
            }
        }
    }
    let what = format!("Kestra's `{canonical}`.");
    set_type(&mut def, type_property(canonical, aliases, &what));
    def
}

fn set_type(def: &mut Value, ty: Value) {
    if let Some(obj) = def.as_object_mut() {
        let props = obj
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(props) = props {
            props.insert("type".into(), ty);
        }
        let required = obj.entry("required").or_insert_with(|| json!([]));
        if let Value::Array(required) = required {
            for key in ["id", "type"] {
                if !required.iter().any(|r| r == key) {
                    required.push(json!(key));
                }
            }
        }
        obj.insert("type".into(), json!("object"));
    }
}

/// An egret-only type: its hand-written schema plus the base task
/// properties, each with Kestra's own schema.
fn egret_type(canonical: &str, aliases: &[&str]) -> Value {
    let mut def = INPUTS
        .egret_types
        .get(canonical)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "{canonical} has neither a Kestra definition nor an egret-task-schemas.json entry"
            )
        });
    let base = kestra_def(BASE_SOURCE);
    if let Some(obj) = def.as_object_mut() {
        let props = obj
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(props) = props {
            for name in BASE_TASK_PROPERTIES {
                if let Some(schema) = base.pointer(&format!("/properties/{name}")) {
                    props
                        .entry(name.to_string())
                        .or_insert_with(|| schema.clone());
                }
            }
        }
    }
    let what = format!("egret's own `{canonical}`.");
    set_type(&mut def, type_property(canonical, aliases, &what));
    def
}

/// A catalog plugin class, `kestra.<fqcn>`. Kestra's definition gives the
/// titles and descriptions; the catalog decides which properties exist and
/// which are required, and its schema replaces Kestra's for any property
/// whose `type` or `enum` differs (W7a spec §7.1, *Precedence*).
fn plugin_type(name: &str, fqcn: &str, class: &crate::plugins::PluginClass) -> Value {
    let kestra = kestra_def(fqcn);
    let mut props = Map::new();
    let mut required = Vec::new();
    for (prop, spec) in &class.properties {
        let ours = kestra.pointer(&format!("/properties/{prop}"));
        let schema = match ours {
            Some(k) if core_of(k) == core_of(&spec.schema) => k.clone(),
            _ => spec.schema.clone(),
        };
        props.insert(prop.clone(), schema);
        if spec.required {
            required.push(json!(prop));
        }
    }
    let mut def = kestra.clone();
    if let Some(obj) = def.as_object_mut() {
        obj.insert("properties".into(), Value::Object(props));
        obj.insert("required".into(), Value::Array(required));
        let described: BTreeMap<String, Value> = obj
            .get("x-egret-outputs")
            .and_then(Value::as_array)
            .map(|outs| {
                outs.iter()
                    .filter_map(|o| Some((o.get("name")?.as_str()?.to_string(), o.clone())))
                    .collect()
            })
            .unwrap_or_default();
        let outputs: Vec<Value> = class
            .outputs
            .iter()
            .map(|o| {
                described
                    .get(o)
                    .cloned()
                    .unwrap_or_else(|| json!({ "name": o }))
            })
            .collect();
        obj.insert("x-egret-outputs".into(), Value::Array(outputs));
    }
    filter_properties(name, &mut def);
    let what = format!(
        "Kestra's `{fqcn}`, run by egret's plugin host. Needs `workflow_kestra_plugins` and \
         `workflow_containers`."
    );
    set_type(&mut def, type_property(name, &[], &what));
    def
}

/// The validation-relevant core of a property schema: its `type` and
/// `enum`, looked for at the top level and inside an `allOf`.
fn core_of(schema: &Value) -> (Option<Value>, Option<Value>) {
    let mut ty = schema.get("type").cloned();
    let mut en = schema.get("enum").cloned();
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        for p in parts {
            ty = ty.or_else(|| p.get("type").cloned());
            en = en.or_else(|| p.get("enum").cloned());
        }
    }
    (ty, en)
}

/// Every `$defs` name reachable from `root` through `#/$defs/` references,
/// transitively through `defs`.
fn reachable_defs(root: &Value, defs: &Map<String, Value>) -> BTreeSet<String> {
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get("$ref") {
                    if let Some(name) = r.strip_prefix("#/$defs/") {
                        out.push(name.to_string());
                    }
                }
                m.values().for_each(|x| refs(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    let mut todo = Vec::new();
    refs(root, &mut todo);
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(d) = defs.get(&name) {
            refs(d, &mut todo);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `$ref` in a served schema resolves: a dangling one would make
    /// monaco-yaml give up on the whole subtree.
    #[test]
    fn every_reference_in_every_served_schema_resolves() {
        for kestra_plugins in [false, true] {
            for containers in [false, true] {
                let f = SchemaFeatures {
                    kestra_plugins,
                    containers,
                };
                let s = flow_schema(&f);
                let defs = s["$defs"].as_object().unwrap();
                let names = reachable_defs(&s, defs);
                let missing: Vec<_> = names.iter().filter(|n| !defs.contains_key(*n)).collect();
                assert!(missing.is_empty(), "{f:?}: dangling {missing:?}");
            }
        }
    }

    /// The four combinations are four different documents, and the same
    /// combination is the same bytes every time.
    #[test]
    fn each_feature_combination_has_its_own_stable_etag() {
        let mut etags = BTreeSet::new();
        for kestra_plugins in [false, true] {
            for containers in [false, true] {
                let f = SchemaFeatures {
                    kestra_plugins,
                    containers,
                };
                assert_eq!(served(&f).etag, served(&f).etag);
                let rebuilt = serde_json::to_vec(&build(f)).unwrap();
                assert_eq!(rebuilt, served(&f).bytes, "{f:?} is not deterministic");
                etags.insert(served(&f).etag.clone());
            }
        }
        // `kestra_plugins` without `containers` serves no plugin types, so
        // it is the same TYPE set as neither -- but the features are named
        // in the document, so the bytes still differ.
        assert_eq!(etags.len(), 4);
    }
}
