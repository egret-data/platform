//! The flow JSON schema `GET …/workflows/schema` serves (W7a spec §7.1),
//! checked against what egret actually accepts. No database.
//!
//! Acceptance is read from the functions parse and publish use -- the
//! parser's typed dispatch (`models::TaskKind`/`TriggerKind`),
//! `registry::known_type`, `plugins::lookup` and the two publish gates --
//! never from the schema module's own tables, so a type the schema offers
//! and publish refuses (or the reverse) fails here.

use std::collections::{BTreeMap, BTreeSet};

use egret_workflow::models::{TaskKind, TriggerKind};
use egret_workflow::spec::schema::{
    flow_schema, kestra_definition, property_treatment, SchemaFeatures, Treatment,
};
use egret_workflow::spec::{parse, registry};
use egret_workflow::{plugins, scripts};
use serde_json::{json, Value};

const ALL: [SchemaFeatures; 4] = [
    SchemaFeatures {
        kestra_plugins: false,
        containers: false,
    },
    SchemaFeatures {
        kestra_plugins: false,
        containers: true,
    },
    SchemaFeatures {
        kestra_plugins: true,
        containers: false,
    },
    SchemaFeatures {
        kestra_plugins: true,
        containers: true,
    },
];

/// The committed export, for its list of every task and trigger type Kestra
/// knows -- the candidate spellings the acceptance check probes.
const KESTRA_JSON: &str = include_str!("../src/spec/schema/kestra-flow-schema.json");

#[derive(Debug, Default)]
struct Served {
    /// Canonical spelling -> its definition name in `$defs`.
    tasks: BTreeMap<String, String>,
    triggers: BTreeMap<String, String>,
    /// Every spelling a `type:` may take (canonical and aliases), with its
    /// description.
    task_spellings: BTreeMap<String, String>,
    trigger_spellings: BTreeMap<String, String>,
}

fn union_members(schema: &Value, union: &str) -> Vec<String> {
    schema["$defs"][union]["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("{union} has a oneOf"))
        .iter()
        .map(|r| {
            r["$ref"]
                .as_str()
                .and_then(|r| r.strip_prefix("#/$defs/"))
                .expect("a oneOf member is a $defs reference")
                .to_string()
        })
        .collect()
}

fn type_consts(def: &Value) -> Vec<(String, String)> {
    def["properties"]["type"]["anyOf"]
        .as_array()
        .expect("every served type's `type` is an anyOf of consts")
        .iter()
        .map(|m| {
            (
                m["const"].as_str().unwrap().to_string(),
                m["description"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn served(schema: &Value) -> Served {
    let mut s = Served::default();
    for (union, canon, spellings) in [
        ("egret.task", &mut s.tasks, &mut s.task_spellings),
        ("egret.trigger", &mut s.triggers, &mut s.trigger_spellings),
    ] {
        for name in union_members(schema, union) {
            let consts = type_consts(&schema["$defs"][&name]);
            canon.insert(consts[0].0.clone(), name.clone());
            for (c, d) in consts {
                spellings.insert(c, d);
            }
        }
    }
    s
}

fn kestra_type_names(key: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(KESTRA_JSON).unwrap();
    v["x-kestra-types"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

/// `core.<lower-cased class>` for every Kestra class in `package` -- the
/// short spellings `kind_for` accepts for a flowable or a trigger.
fn short_spellings(names: &[String], package: &str) -> Vec<String> {
    names
        .iter()
        .filter_map(|n| n.strip_prefix(package))
        .map(|class| format!("core.{}", class.to_ascii_lowercase()))
        .collect()
}

/// Would publish accept a task of type `t` in a workspace with `f`, as far
/// as the TYPE decides? The flow is the smallest one carrying it; what the
/// task's own properties would make of it is not the question.
fn task_accepted(t: &str, f: SchemaFeatures) -> bool {
    let source = json!({
        "id": "a", "namespace": "a.b",
        "tasks": [{ "id": "t", "type": t }],
    })
    .to_string();
    let flow = parse(&source).expect("a one-task flow parses");
    if !matches!(flow.tasks[0].kind, TaskKind::Leaf { .. }) {
        return true; // a typed flowable: no type refusal anywhere
    }
    let cfg = egret_config::PluginConfigurations::default();
    if registry::known_type(t).is_some() {
        // Publish step 3b: a script task's FIRST refusal is the feature.
        if scripts::kind_of(t).is_some() {
            let verdict = scripts::gate::check(&scripts::gate::GateInput {
                feature_enabled: f.containers,
                image: "x",
                runner: scripts::RunnerKind::Docker,
                plugin_configurations: &cfg,
            });
            return verdict != Err(scripts::gate::GateRefusal::FeatureDisabled);
        }
        return true;
    }
    // Publish step 4 lets a catalog plugin type through; step 3c gates it.
    if let Some((fqcn, _)) = plugins::lookup(t) {
        return plugins::gate::check(&plugins::gate::GateInput {
            feature_enabled: f.kestra_plugins,
            containers_enabled: f.containers,
            plugin_configurations: &cfg,
            runner: Some(scripts::RunnerKind::Docker),
            fqcn,
        })
        .is_ok();
    }
    false
}

/// A trigger egret parses into a typed `TriggerKind`; any other trigger type
/// is an inert leaf that never fires.
fn trigger_accepted(t: &str) -> bool {
    let source = json!({
        "id": "a", "namespace": "a.b",
        "tasks": [{ "id": "t", "type": "io.kestra.plugin.core.log.Log", "message": "m" }],
        "triggers": [{ "id": "g", "type": t }],
    })
    .to_string();
    let flow = parse(&source).expect("a one-trigger flow parses");
    !matches!(flow.triggers[0].kind, TriggerKind::Leaf { .. })
}

/// Test 1: the schema offers exactly the type spellings parse and publish
/// accept, for each combination of features.
#[test]
fn the_served_type_set_is_the_accepted_type_set_for_every_feature_combination() {
    let kestra_tasks = kestra_type_names("tasks");
    let kestra_triggers = kestra_type_names("triggers");
    for f in ALL {
        let schema = flow_schema(&f);
        let s = served(&schema);

        let mut candidates: BTreeSet<String> = s.task_spellings.keys().cloned().collect();
        candidates.extend(kestra_tasks.iter().cloned());
        candidates.extend(short_spellings(
            &kestra_tasks,
            "io.kestra.plugin.core.flow.",
        ));
        for row in registry::TASK_TYPES {
            candidates.insert(row.r#type.to_string());
            candidates.extend(row.aliases.iter().map(|a| a.to_string()));
        }
        candidates.extend(plugins::prefixed_types().map(str::to_string));
        let accepted: BTreeSet<String> = candidates
            .into_iter()
            .filter(|t| task_accepted(t, f))
            .collect();
        let offered: BTreeSet<String> = s.task_spellings.keys().cloned().collect();
        assert_eq!(
            offered,
            accepted,
            "{f:?}: offered-but-refused {:?}, accepted-but-not-offered {:?}",
            offered.difference(&accepted).collect::<Vec<_>>(),
            accepted.difference(&offered).collect::<Vec<_>>()
        );

        let mut candidates: BTreeSet<String> = s.trigger_spellings.keys().cloned().collect();
        candidates.extend(kestra_triggers.iter().cloned());
        candidates.extend(short_spellings(
            &kestra_triggers,
            "io.kestra.plugin.core.trigger.",
        ));
        let accepted: BTreeSet<String> = candidates
            .into_iter()
            .filter(|t| trigger_accepted(t))
            .collect();
        let offered: BTreeSet<String> = s.trigger_spellings.keys().cloned().collect();
        assert_eq!(offered, accepted, "{f:?}: triggers");
    }
}

/// Every Kestra FQCN egret accepts is served from Kestra's own definition
/// (so `trim.py`'s `KESTRA_TYPES` cannot fall behind the registry).
#[test]
fn every_accepted_kestra_class_is_backed_by_the_export() {
    let all = flow_schema(&ALL[3]);
    let s = served(&all);
    for canonical in s.tasks.keys().chain(s.triggers.keys()) {
        let fqcn = canonical.strip_prefix(plugins::PREFIX).unwrap_or(canonical);
        if fqcn.starts_with("io.kestra.") {
            assert!(
                kestra_definition(fqcn).is_some(),
                "{fqcn} is accepted but not in kestra-flow-schema.json; add it to trim.py"
            );
        }
    }
}

/// Test 2: a registry alias is listed beside its canonical spelling, and its
/// description names the canonical FQCN.
#[test]
fn aliases_are_listed_and_name_their_canonical_spelling() {
    let all = flow_schema(&ALL[3]);
    let s = served(&all);
    let log = &all["$defs"]["io.kestra.plugin.core.log.Log"];
    let consts = type_consts(log);
    assert_eq!(consts[0].0, "io.kestra.plugin.core.log.Log");
    let alias = consts
        .iter()
        .find(|(c, _)| c == "core.log")
        .expect("core.log is listed on the Log definition");
    assert!(
        alias.1.contains("io.kestra.plugin.core.log.Log"),
        "{alias:?}"
    );
    for row in registry::TASK_TYPES {
        for a in row.aliases {
            let d = s.task_spellings.get(*a).unwrap_or_else(|| panic!("{a}"));
            assert!(d.contains(row.r#type), "{a}: {d}");
        }
    }
}

fn all_type_spellings(schema: &Value) -> BTreeSet<String> {
    let s = served(schema);
    s.task_spellings
        .into_keys()
        .chain(s.trigger_spellings.into_keys())
        .collect()
}

/// Test 3: features filter the types out of the document, not only out of
/// the unions.
#[test]
fn features_remove_the_plugin_and_script_types() {
    for f in ALL {
        let schema = flow_schema(&f);
        let types = all_type_spellings(&schema);
        let defs: Vec<&String> = schema["$defs"].as_object().unwrap().keys().collect();
        let has_plugins = types.iter().any(|t| t.contains("io.kestra.plugin.jdbc."))
            || defs.iter().any(|d| d.contains("io.kestra.plugin.jdbc."));
        let has_scripts = types.iter().any(|t| scripts::kind_of(t).is_some());
        assert_eq!(
            has_plugins,
            f.kestra_plugins && f.containers,
            "{f:?}: plugin types"
        );
        assert_eq!(has_scripts, f.containers, "{f:?}: script types");
    }
}

/// Test 4: no served definition offers a property Step 2 measured as
/// ignored (or refused).
#[test]
fn no_served_property_is_one_egret_ignores_or_refuses() {
    let table = property_treatment();
    let schema = flow_schema(&ALL[3]);
    let s = served(&schema);
    let mut checked = 0;
    for name in s.tasks.values().chain(s.triggers.values()) {
        let Some(row) = table.get(name) else {
            continue; // egret-only, hand-written
        };
        let props = schema["$defs"][name]["properties"].as_object().unwrap();
        for p in props.keys() {
            let t = row.get(p);
            assert!(
                matches!(t, Some(Treatment::Honoured | Treatment::Warned)),
                "{name}.{p} is served but measured {t:?}"
            );
            checked += 1;
        }
    }
    let flow = table.get("Flow").expect("the flow document has a row");
    for p in schema["properties"].as_object().unwrap().keys() {
        assert!(
            matches!(flow.get(p), Some(Treatment::Honoured | Treatment::Warned)),
            "flow.{p} is served but measured {:?}",
            flow.get(p)
        );
    }
    // The seven base properties the brief names are measured ignored on a
    // non-script task, and so absent from it.
    let log = &schema["$defs"]["io.kestra.plugin.core.log.Log"]["properties"];
    for p in [
        "assets",
        "logLevel",
        "logToFile",
        "policyRefs",
        "taskCache",
        "version",
        "workerSelector",
    ] {
        assert!(log.get(p).is_none(), "Log offers {p}");
        assert_eq!(
            table["io.kestra.plugin.core.log.Log"].get(p),
            Some(&Treatment::Ignored)
        );
    }
    assert!(checked > 200, "only {checked} properties were checked");
}

/// Test 5: every served type with a known outputs list carries it.
#[test]
fn every_type_with_known_outputs_carries_x_egret_outputs() {
    let schema = flow_schema(&ALL[3]);
    let s = served(&schema);
    for name in s.tasks.values().chain(s.triggers.values()) {
        let fqcn = name.strip_prefix(plugins::PREFIX).unwrap_or(name);
        let known = kestra_definition(fqcn).is_some_and(|d| d.get("x-egret-outputs").is_some())
            || plugins::lookup(name).is_some()
            || name == "egret.sql";
        if known {
            assert!(
                schema["$defs"][name]["x-egret-outputs"].is_array(),
                "{name} has known outputs but serves none"
            );
        }
    }
    // Spot checks against Kestra's documentation of the class.
    let get = &schema["$defs"]["io.kestra.plugin.core.kv.Get"]["x-egret-outputs"];
    assert_eq!(get[0]["name"], "value");
    let q = &schema["$defs"]["kestra.io.kestra.plugin.jdbc.trino.Query"]["x-egret-outputs"];
    let names: Vec<&str> = q
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["row", "rows", "size", "uri"]);
}

/// Test 6: egret's own types are described, and require at least the
/// fields publish resolves.
#[test]
fn egret_only_types_are_described_with_their_required_properties() {
    let schema = flow_schema(&ALL[3]);
    for t in [
        "egret.sql",
        "egret.dataapi.call",
        "egret.dataset.refresh",
        "egret.report.send",
        "core.http",
        "core.slack",
    ] {
        let def = &schema["$defs"][t];
        assert!(def.is_object(), "{t} has no definition");
        let described = ["title", "markdownDescription", "description"]
            .iter()
            .any(|k| def[k].as_str().is_some_and(|s| !s.is_empty()));
        assert!(described, "{t} has no description");
        let required: BTreeSet<&str> = def["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let row = registry::known_type(t).expect("registry row");
        for rf in row.references {
            assert!(required.contains(rf.field), "{t} must require {}", rf.field);
            assert!(def["properties"][rf.field].is_object(), "{t}.{}", rf.field);
        }
        assert!(required.contains("id") && required.contains("type"), "{t}");
    }
    let sql = &schema["$defs"]["egret.sql"]["required"];
    assert_eq!(sql, &json!(["database", "sql", "id", "type"]));
}

/// The document's shape: draft 2020-12, the flow's own properties, task
/// lists through the task union -- including a flowable's nested list.
#[test]
fn the_document_is_a_draft_2020_12_flow_schema_with_task_unions() {
    let schema = flow_schema(&ALL[0]);
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    for p in ["id", "namespace", "tasks", "triggers", "inputs", "labels"] {
        assert!(schema["properties"][p].is_object(), "flow.{p}");
    }
    assert_eq!(
        schema["properties"]["tasks"]["items"]["$ref"],
        "#/$defs/egret.task"
    );
    assert_eq!(
        schema["$defs"]["io.kestra.plugin.core.flow.Sequential"]["properties"]["tasks"]["items"]
            ["$ref"],
        "#/$defs/egret.task"
    );
}
