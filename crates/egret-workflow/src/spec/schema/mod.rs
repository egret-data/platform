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
//! * minus every task type this deployment does not serve
//!   (`runtime::served()`, no-silent-stall §4.4): `core.http`,
//!   `egret.report.send` and the other types no handler runs, and, with no container runner,
//!   the script and plugin types -- the types publish refuses as
//!   `task_type_not_served`;
//! * of each Kestra type, only the properties egret honours or warns about.
//!   An ignored property is never offered (completion would suggest something
//!   egret silently drops), and neither is a refused one (it would suggest a
//!   save error).
//!
//! * of each nested definition egret parses (inputs, retry, the trigger and
//!   flow sub-objects), the same: what serde would drop or a later stage
//!   would refuse is not offered, and neither are refused `enum` values.
//!
//! Which gated types are offered follows the publish gates for the
//! workspace's features AND the deployment's `plugin_configurations`,
//! default runner and served set; each outcome is built once and kept.

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

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

/// Which gated types this workspace, on this deployment, may publish --
/// decided by the two publish gates themselves, never by a copy of their
/// rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Accepted {
    features: SchemaFeatures,
    /// The script types: `scripts::gate::check_unrendered` (the feature, an
    /// `allowed-images` entry for the runner) with the configured default
    /// runner, which `publish::check_script_tasks` requires.
    scripts: bool,
    /// The catalog classes `plugins::gate::check` lets through (both
    /// features, a runner, `allowed-types`) and this deployment serves.
    plugins: Vec<String>,
    /// The `registry::TASK_TYPES` rows this deployment serves
    /// (`runtime::Served`, no-silent-stall §4.4), by canonical spelling.
    /// Part of the cache key, so a schema built for one served set is never
    /// answered for another.
    served_types: Vec<&'static str>,
}

impl Accepted {
    fn of(
        features: SchemaFeatures,
        deployment: &crate::scripts::ScriptSettings,
        served: &crate::runtime::Served,
    ) -> Self {
        let cfg = &deployment.plugin_configurations;
        let scripts = deployment.default_runner.is_some_and(|runner| {
            crate::scripts::gate::check_unrendered(features.containers, runner, cfg).is_ok()
        });
        let plugins = crate::plugins::catalog()
            .classes
            .keys()
            .filter(|fqcn| {
                crate::plugins::gate::check(&crate::plugins::gate::GateInput {
                    feature_enabled: features.kestra_plugins,
                    containers_enabled: features.containers,
                    plugin_configurations: cfg,
                    runner: deployment.default_runner,
                    fqcn,
                })
                .is_ok()
            })
            .filter(|fqcn| served.serves(&format!("{}{fqcn}", crate::plugins::PREFIX)))
            .cloned()
            .collect();
        let served_types = registry::TASK_TYPES
            .iter()
            .map(|row| row.r#type)
            .filter(|t| served.serves(t))
            .collect();
        Accepted {
            features,
            scripts,
            plugins,
            served_types,
        }
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
    /// Accepted and not acted on. Never served, and a draft that writes one
    /// gets a `not_honoured` warning that names the package which lifts it
    /// (`spec::keys::findings`, package #1).
    Ignored,
    /// Refused at save or publish. Never served.
    Refused,
}

impl Treatment {
    fn served(self) -> bool {
        matches!(self, Treatment::Honoured | Treatment::Warned)
    }
}

/// Row (a served type, a nested Kestra definition, or `Flow` for the flow
/// document itself) -> property -> treatment.
pub type TreatmentTable = BTreeMap<String, BTreeMap<String, Treatment>>;

/// `property-treatment.json`, the committed measurement (`measure.rs`).
#[derive(Debug, Default, Deserialize, serde::Serialize)]
pub struct Measured {
    /// How egret treats each property.
    pub properties: TreatmentTable,
    /// Row -> property -> the values of its `enum` that egret refuses (at
    /// parse, or at a later stage). Removed from the served enum.
    #[serde(default)]
    pub refused_values: BTreeMap<String, BTreeMap<String, Vec<Value>>>,
    /// Row -> property (or `property=value`) -> the refusal a later stage
    /// answered (trigger resolution, webhook firing): the site, by its own
    /// words, of every `refused` that parse and validate let through.
    #[serde(default)]
    pub refused_downstream: BTreeMap<String, BTreeMap<String, String>>,
}

/// The committed measurement.
pub fn measured() -> &'static Measured {
    static TABLE: LazyLock<Measured> = LazyLock::new(|| {
        serde_json::from_str(TREATMENT_JSON)
            .unwrap_or_else(|e| panic!("spec/schema/property-treatment.json: {e}"))
    });
    &TABLE
}

/// [`Measured::properties`].
pub fn property_treatment() -> &'static TreatmentTable {
    &measured().properties
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

/// The schema for a workspace with `features` on this `deployment` (its
/// `workflow.plugin_configurations` and default runner) and this process's
/// served set (`runtime::served()`), built on first use and kept. The key is
/// what the two publish gates accept and the served set lets through, so a
/// deployment, whose served set is fixed at start-up, has at most four
/// entries.
///
/// # Panics
///
/// When no served set is installed (`runtime::served`).
pub fn served(
    features: &SchemaFeatures,
    deployment: &crate::scripts::ScriptSettings,
) -> Arc<ServedSchema> {
    served_with(features, deployment, crate::runtime::served())
}

/// [`served`] for the task types `served` names rather than the ones this
/// process installed: for a test that compares two sets in one process.
///
/// The cache is keyed by what is offered, served types included, so two sets
/// never share an entry; in production the set is fixed per process and
/// this adds no entry.
pub fn served_with(
    features: &SchemaFeatures,
    deployment: &crate::scripts::ScriptSettings,
    served: &crate::runtime::Served,
) -> Arc<ServedSchema> {
    static CACHE: LazyLock<Mutex<HashMap<Accepted, Arc<ServedSchema>>>> =
        LazyLock::new(Default::default);
    let accepted = Accepted::of(*features, deployment, served);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    cache
        .entry(accepted.clone())
        .or_insert_with(|| {
            let value = build(&accepted);
            let bytes = serde_json::to_vec(&value).expect("a serde_json::Value always serialises");
            let etag = format!("\"sha256-{}\"", hex::encode(Sha256::digest(&bytes)));
            Arc::new(ServedSchema { value, bytes, etag })
        })
        .clone()
}

/// The JSON schema of one flow document for a workspace with `features` on
/// this `deployment`.
pub fn flow_schema(
    features: &SchemaFeatures,
    deployment: &crate::scripts::ScriptSettings,
) -> Value {
    served(features, deployment).value.clone()
}

struct Inputs {
    kestra_source: Value,
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
        kestra_source: kestra
            .get("x-egret-source")
            .cloned()
            .expect("kestra-flow-schema.json has x-egret-source"),
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

/// The Kestra version the committed export was taken from, as recorded in
/// the file (`x-egret-source.kestra_version`).
pub fn kestra_version() -> &'static str {
    INPUTS.kestra_source["kestra_version"]
        .as_str()
        .expect("x-egret-source.kestra_version is a string")
}

fn build(accepted: &Accepted) -> Value {
    build_with(accepted, true)
}

/// [`build`], with [`defer_required_to_type`] optional so a test can compare
/// the document before and after it.
fn build_with(accepted: &Accepted, defer_required: bool) -> Value {
    let features = accepted.features;
    let inputs = &*INPUTS;
    let mut defs = inputs.kestra_defs.clone();
    for (k, v) in &inputs.egret_defs {
        defs.insert(k.clone(), v.clone());
    }

    let mut tasks: Vec<String> = Vec::new();
    for row in registry::TASK_TYPES {
        if crate::scripts::kind_of(row.r#type).is_some() && !accepted.scripts {
            continue;
        }
        if !accepted.served_types.contains(&row.r#type) {
            continue;
        }
        let def = match inputs.kestra_defs.get(row.r#type) {
            Some(kestra) => kestra_type(row.r#type, row.aliases, kestra),
            None => egret_type(row.r#type, row.aliases),
        };
        defs.insert(row.r#type.to_string(), def);
        tasks.push(row.r#type.to_string());
    }
    for fqcn in &accepted.plugins {
        let class = &crate::plugins::catalog().classes[fqcn];
        let name = format!("{}{fqcn}", crate::plugins::PREFIX);
        defs.insert(name.clone(), plugin_type(&name, fqcn, class));
        tasks.push(name);
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
    root.insert("x-egret-kestra-version".into(), json!(kestra_version()));
    let mut flow = inputs.kestra_flow.clone();
    filter_properties(FLOW_ROW, &mut flow);
    remove_refused_values(FLOW_ROW, &mut flow);
    // Every measured row, served type or nested definition, through the same
    // two filters. Idempotent on the types `kestra_type`/`plugin_type` built.
    for (name, def) in defs.iter_mut() {
        if property_treatment().contains_key(name) {
            filter_properties(name, def);
            remove_refused_values(name, def);
        }
    }
    // After the filters, which may drop a required property egret ignores.
    for name in tasks.iter().chain(&triggers) {
        if let Some(def) = defs.get_mut(name).filter(|_| defer_required) {
            defer_required_to_type(def);
        }
    }
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

/// The guard [`defer_required_to_type`] puts in a type's `if`.
fn type_is_a_string() -> Value {
    json!({ "required": ["type"], "properties": { "type": { "type": "string" } } })
}

/// Move a task or trigger type's required properties, other than `id` and
/// `type`, into `then.required` behind `if: type is a string`.
///
/// **Why.** The editor's completion is yaml-language-server's (`monaco-yaml`).
/// To complete `type: |` it scores every `oneOf` member against the mapping
/// being typed and merges only the best-scoring ones, and a missing required
/// property counts against a member. With `required: [id, message, type]`,
/// `Log` loses to every type that requires nothing else, so the `type:`
/// completion offered 10 of the 54 task spellings and 2 of the 6 trigger
/// spellings (measured on this schema with the language service of `monaco-yaml`
/// 5.5.1). While `type` has no value the guard is false, so no member is
/// penalised and every spelling is offered; once `type` holds a string the
/// guard holds and the "Missing property" diagnostic is the same as before.
///
/// **It accepts exactly the same documents.** A member is only satisfied when
/// `type` is present and equals one of its string `const`s, and then the
/// guard holds, so `then` applies; when `type` is absent or not a string the
/// member already fails on `required`/`type`. Kestra's schema has the same
/// shape problem; Kestra's UI papers over it with its own `type:` completion.
fn defer_required_to_type(def: &mut Value) {
    let Some(Value::Array(required)) = def.get("required") else {
        return;
    };
    let (outright, deferred): (Vec<Value>, Vec<Value>) = required
        .iter()
        .cloned()
        .partition(|r| r == "id" || r == "type");
    if deferred.is_empty() {
        return;
    }
    let m = def
        .as_object_mut()
        .expect("a definition with `required` is an object");
    m.insert("required".into(), Value::Array(outright));
    m.insert("if".into(), type_is_a_string());
    m.insert("then".into(), json!({ "required": deferred }));
}

/// Remove from each property's `enum` (wherever it sits in that property's
/// own schema: top level, `allOf`/`anyOf`/`oneOf` members, `items`) the
/// values [`Measured::refused_values`] lists for `row`.
fn remove_refused_values(row: &str, def: &mut Value) {
    fn strip(schema: &mut Value, refused: &[Value]) {
        let Value::Object(m) = schema else {
            return;
        };
        if let Some(Value::Array(values)) = m.get_mut("enum") {
            values.retain(|v| !refused.contains(v));
        }
        if let Some(d) = m.get("default") {
            if refused.contains(d) {
                m.remove("default");
            }
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(Value::Array(parts)) = m.get_mut(key) {
                parts.iter_mut().for_each(|p| strip(p, refused));
            }
        }
        if let Some(items) = m.get_mut("items") {
            strip(items, refused);
        }
    }
    let Some(table) = measured().refused_values.get(row) else {
        return;
    };
    for (prop, refused) in table {
        if let Some(schema) = def.pointer_mut(&format!("/properties/{prop}")) {
            strip(schema, refused);
            // Kestra's own text still describes the removed values; say so.
            if let Value::Object(m) = schema {
                let names: Vec<String> = refused
                    .iter()
                    .map(|v| format!("`{}`", v.as_str().unwrap_or_default()))
                    .collect();
                let note = format!("**Not accepted by egret:** {}.", names.join(", "));
                let text = match m.get("markdownDescription").and_then(Value::as_str) {
                    Some(t) => format!("{t}\n\n{note}"),
                    None => note,
                };
                m.insert("markdownDescription".into(), Value::String(text));
            }
        }
    }
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

/// A deployment that lets every gated type through: a Docker runner whose
/// `allowed-images` is `*`, and no `kestra` entry (every catalog class).
pub fn permissive_deployment() -> crate::scripts::ScriptSettings {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    crate::scripts::ScriptSettings {
        plugin_configurations: PluginConfigurations(vec![PluginConfiguration {
            r#type: crate::scripts::DOCKER_RUNNER.to_string(),
            values: PluginConfigurationValues {
                allowed_images: Some(vec!["*".to_string()]),
                ..Default::default()
            },
        }]),
        default_runner: Some(crate::scripts::RunnerKind::Docker),
    }
}

// ---------- serde's own field list ----------

#[derive(Debug)]
struct Captured(Option<&'static [&'static str]>);

impl std::fmt::Display for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
impl std::error::Error for Captured {}
impl serde::de::Error for Captured {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Captured(None)
    }
}

/// A deserializer that answers nothing and records the field list a derived
/// `Deserialize` hands to `deserialize_struct`.
struct FieldProbe;

impl<'de> serde::de::Deserializer<'de> for FieldProbe {
    type Error = Captured;
    fn deserialize_any<V: serde::de::Visitor<'de>>(self, _: V) -> Result<V::Value, Captured> {
        Err(Captured(None))
    }
    fn deserialize_struct<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Captured> {
        Err(Captured(Some(fields)))
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The wire names of `T`'s fields, as its derived `Deserialize` knows them
/// -- the keys serde keeps; any other key it drops (or, with
/// `deny_unknown_fields`, refuses). Panics for a type that does not
/// deserialize as a struct.
pub fn wire_fields_of<T: serde::de::DeserializeOwned>() -> Vec<&'static str> {
    match T::deserialize(FieldProbe) {
        Err(Captured(Some(fields))) => fields.to_vec(),
        _ => panic!(
            "{} does not deserialize as a struct",
            std::any::type_name::<T>()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The served set of a deployment with a container runner: every type
    /// some configuration serves. This binary installs no set, so every
    /// test here names its own.
    fn runner() -> crate::runtime::Served {
        let mut cfg = egret_config::Config::default().workflow;
        cfg.container_runner = egret_config::ContainerRunner::Docker;
        crate::runtime::served_for(&cfg)
    }

    /// Every type the registry and the catalog name: the runner set plus the
    /// types no handler runs yet. The schema's own properties hold for every
    /// type it can offer, so they are checked on all of them.
    fn all_types() -> crate::runtime::Served {
        crate::runtime::Served::from_type_ids(
            runner()
                .type_ids()
                .chain(registry::TASK_TYPES.iter().map(|row| row.r#type))
                .map(str::to_string),
        )
    }

    /// No-silent-stall §4.4: a type the set lacks is not offered, the cache
    /// keeps the two sets apart, and the `ETag` changes with the set.
    #[test]
    fn the_served_set_decides_the_registry_types_and_the_etag() {
        let f = SchemaFeatures {
            kestra_plugins: true,
            containers: true,
        };
        let d = permissive_deployment();
        let runner = runner();
        // `egret.report.send` is a registered row no deployment serves
        // (spec D10 defers it); `egret.sql` was the probe until package #4
        // Task C5 served it.
        let with_report = crate::runtime::Served::from_type_ids(
            runner
                .type_ids()
                .map(str::to_string)
                .chain(["egret.report.send".to_string()]),
        );
        let without = served_with(&f, &d, &runner);
        let with = served_with(&f, &d, &with_report);
        assert!(without.value["$defs"].get("egret.report.send").is_none());
        assert!(with.value["$defs"]["egret.report.send"].is_object());
        assert!(without.value["$defs"]["io.kestra.plugin.core.log.Log"].is_object());
        assert_ne!(without.etag, with.etag);
        // Asked again in the other order: each set still gets its own entry.
        assert_eq!(served_with(&f, &d, &runner).etag, without.etag);
    }

    fn all_features() -> Vec<SchemaFeatures> {
        let mut v = Vec::new();
        for kestra_plugins in [false, true] {
            for containers in [false, true] {
                v.push(SchemaFeatures {
                    kestra_plugins,
                    containers,
                });
            }
        }
        v
    }

    /// Every `$ref` in a served schema resolves: a dangling one would make
    /// monaco-yaml give up on the whole subtree.
    #[test]
    fn every_reference_in_every_served_schema_resolves() {
        for f in all_features() {
            let s = served_with(&f, &permissive_deployment(), &all_types())
                .value
                .clone();
            let defs = s["$defs"].as_object().unwrap();
            let names = reachable_defs(&s, defs);
            let missing: Vec<_> = names.iter().filter(|n| !defs.contains_key(*n)).collect();
            assert!(missing.is_empty(), "{f:?}: dangling {missing:?}");
        }
    }

    /// Every served task and trigger type requires only `id` and `type`
    /// outright; the rest of its required properties sit behind an `if` that
    /// holds once `type` is a string (`defer_required_to_type`). The full
    /// requirement is unchanged: `required` plus `then.required` is what the
    /// type required before, which `egret.sql` and Kestra's `Log` pin.
    #[test]
    fn every_served_type_defers_its_other_required_properties_behind_its_type() {
        let strs = |v: &Value| -> BTreeSet<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        for f in all_features() {
            let s = served_with(&f, &permissive_deployment(), &all_types())
                .value
                .clone();
            let defs = s["$defs"].as_object().unwrap();
            let mut deferred = 0;
            for union in ["egret.task", "egret.trigger"] {
                for member in s["$defs"][union]["oneOf"].as_array().unwrap() {
                    let name = member["$ref"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("#/$defs/");
                    let def = &defs[name];
                    let outright = strs(&def["required"]);
                    assert!(outright.contains("type"), "{f:?} {name}: {outright:?}");
                    assert!(
                        outright.iter().all(|r| r == "id" || r == "type"),
                        "{f:?} {name} requires {outright:?} outright"
                    );
                    if def.get("then").is_some() {
                        deferred += 1;
                        assert_eq!(def["if"], type_is_a_string(), "{f:?} {name}");
                        let rest = strs(&def["then"]["required"]);
                        assert!(
                            !rest.is_empty() && rest.is_disjoint(&outright),
                            "{f:?} {name}"
                        );
                    } else {
                        assert!(def.get("if").is_none(), "{f:?} {name}");
                    }
                }
            }
            assert!(deferred > 30, "{f:?}: only {deferred} types defer anything");
            let full = |name: &str| {
                let d = &defs[name];
                let mut all = strs(&d["required"]);
                all.extend(strs(&d["then"]["required"]));
                all
            };
            assert_eq!(
                full("egret.sql"),
                strs(&json!(["database", "sql", "id", "type"]))
            );
            assert_eq!(
                full("io.kestra.plugin.core.log.Log"),
                strs(&kestra_definition("io.kestra.plugin.core.log.Log").unwrap()["required"])
            );
        }
    }

    /// For every served type, in every feature combination: the deferral moved
    /// required properties and changed nothing else. `required` plus
    /// `then.required` is exactly the type's `required` without the deferral,
    /// every other key is untouched, and no type had an `if` or `then` of its
    /// own for the guard to overwrite.
    #[test]
    fn deferring_required_keeps_every_types_requirement_and_nothing_else_changes() {
        let set = |v: &Value| -> BTreeSet<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let d = permissive_deployment();
        for f in all_features() {
            let accepted = Accepted::of(f, &d, &all_types());
            let before = build_with(&accepted, false);
            let after = build_with(&accepted, true);
            let mut members = 0;
            for union in ["egret.task", "egret.trigger"] {
                for member in after["$defs"][union]["oneOf"].as_array().unwrap() {
                    let name = member["$ref"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("#/$defs/");
                    let (b, a) = (&before["$defs"][name], &after["$defs"][name]);
                    assert!(
                        b.get("if").is_none() && b.get("then").is_none(),
                        "{f:?} {name} has its own if/then"
                    );
                    let mut full = set(&a["required"]);
                    full.extend(set(&a["then"]["required"]));
                    assert_eq!(full, set(&b["required"]), "{f:?} {name}");
                    let strip = |v: &Value| {
                        let mut m = v.as_object().unwrap().clone();
                        for k in ["required", "if", "then"] {
                            m.remove(k);
                        }
                        m
                    };
                    assert_eq!(strip(a), strip(b), "{f:?} {name}: other keys changed");
                    members += 1;
                }
            }
            assert!(members > 40, "{f:?}: only {members} types");
            // Outside the type definitions, the documents are the same.
            let others = |v: &Value| {
                let mut m = v.as_object().unwrap().clone();
                m.remove("$defs");
                m
            };
            assert_eq!(others(&after), others(&before), "{f:?}");
        }
    }

    /// The four combinations are four different documents, and the same
    /// combination is the same bytes every time.
    #[test]
    fn each_feature_combination_has_its_own_stable_etag() {
        let d = permissive_deployment();
        let mut etags = BTreeSet::new();
        for f in all_features() {
            assert_eq!(
                served_with(&f, &d, &all_types()).etag,
                served_with(&f, &d, &all_types()).etag
            );
            let rebuilt = serde_json::to_vec(&build(&Accepted::of(f, &d, &all_types()))).unwrap();
            assert_eq!(
                rebuilt,
                served_with(&f, &d, &all_types()).bytes,
                "{f:?} is not deterministic"
            );
            etags.insert(served_with(&f, &d, &all_types()).etag.clone());
        }
        // `kestra_plugins` without `containers` serves no plugin types, so
        // it is the same TYPE set as neither -- but the features are named
        // in the document, so the bytes still differ.
        assert_eq!(etags.len(), 4);
    }
}
