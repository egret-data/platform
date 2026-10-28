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
use egret_workflow::scripts::ScriptSettings;
use egret_workflow::spec::schema::{
    kestra_definition, measured, permissive_deployment, property_treatment, wire_fields_of,
    SchemaFeatures, Treatment,
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

/// The task types this binary serves (no-silent-stall §4.4): a deployment
/// with a container runner, so every script and plugin type is served and
/// the publish gates alone decide them, as before; the types no handler runs
/// (`core.http`, `egret.report.send`, ...) are served by no deployment.
/// `egret.sql` is served by every deployment since package #4 Task C5.
fn served_set() -> &'static egret_workflow::runtime::Served {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.container_runner = egret_config::ContainerRunner::Docker;
    egret_workflow::runtime::install_served_for_tests(egret_workflow::runtime::served_for(&cfg))
}

/// `spec::schema::flow_schema`, under [`served_set`].
fn flow_schema(f: &SchemaFeatures, d: &ScriptSettings) -> Value {
    served_set();
    egret_workflow::spec::schema::flow_schema(f, d)
}

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

/// Would publish accept a task of type `t` in a workspace with `f` on
/// deployment `d`, as far as the TYPE decides? Answered by publish's own
/// functions: the parser's dispatch, `registry::known_type`,
/// `publish::check_script_tasks` (on a script task whose image is an
/// expression, so only the type-level rules apply) and `plugins::gate::check`
/// (what `publish::check_plugin_tasks` calls once the properties parse),
/// and `spec::unrunnable` against the served set (publish step 3's
/// `task_type_not_served`).
fn task_accepted(t: &str, f: SchemaFeatures, d: &ScriptSettings) -> bool {
    let mut task = json!({ "id": "t", "type": t });
    if let Some(kind) = scripts::kind_of(t) {
        task[kind.body_property()] = match kind.body_property() {
            "commands" => json!(["true"]),
            _ => json!("true"),
        };
        task["containerImage"] = json!("{{ inputs.image }}");
    }
    let source = json!({ "id": "a", "namespace": "a.b", "tasks": [task] }).to_string();
    let flow = parse(&source).expect("a one-task flow parses");
    if !matches!(flow.tasks[0].kind, TaskKind::Leaf { .. }) {
        return true; // a typed flowable: no type refusal anywhere
    }
    if !egret_workflow::spec::unrunnable(&flow, served_set()).is_empty() {
        return false;
    }
    let gate = egret_workflow::ScriptGate {
        feature_enabled: f.containers,
        plugin_configurations: &d.plugin_configurations,
        default_runner: d.default_runner,
        kestra_plugins_enabled: f.kestra_plugins,
    };
    if registry::known_type(t).is_some() {
        return egret_workflow::publish::check_script_tasks(&flow, &gate).is_ok();
    }
    // Publish step 4 lets a catalog plugin type through; step 3c gates it.
    if let Some((fqcn, _)) = plugins::lookup(t) {
        return plugins::gate::check(&plugins::gate::GateInput {
            feature_enabled: f.kestra_plugins,
            containers_enabled: f.containers,
            plugin_configurations: &d.plugin_configurations,
            runner: d.default_runner,
            fqcn,
        })
        .is_ok();
    }
    false
}

/// The deployments the type-set test runs against: everything allowed; a
/// `kestra` allow-list naming the PostgreSQL classes only; no container
/// runner; and the default settings (no `plugin_configurations` at all).
fn deployments() -> Vec<(&'static str, ScriptSettings)> {
    use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
    let mut restricted = permissive_deployment();
    restricted
        .plugin_configurations
        .0
        .push(PluginConfiguration {
            r#type: egret_config::KESTRA_CONFIG_TYPE.to_string(),
            values: PluginConfigurationValues {
                allowed_types: Some(vec!["io.kestra.plugin.jdbc.postgresql.*".to_string()]),
                ..Default::default()
            },
        });
    let mut no_runner = permissive_deployment();
    no_runner.default_runner = None;
    vec![
        ("permissive", permissive_deployment()),
        ("allow-list", restricted),
        ("no runner", no_runner),
        (
            "default",
            ScriptSettings {
                plugin_configurations: PluginConfigurations::default(),
                default_runner: None,
            },
        ),
    ]
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

/// The spellings of `ForEach`, which egret refuses as the validation error
/// `unsupported_foreach` (the user's ruling of 2026-10-12).
const FOREACH_SPELLINGS: [&str; 2] = ["io.kestra.plugin.core.flow.ForEach", "core.foreach"];

/// The 2026-10-12 ruling: `ForEach` is refused (by validate, and by publish
/// before that as an unknown type), so no feature combination offers it,
/// while `Loop` -- what the refusal tells the author to use -- is offered
/// under both spellings. Without the `Loop` half this passes under a schema
/// that offers no flowable at all.
#[test]
fn foreach_is_never_offered_and_loop_is() {
    for (name, d) in deployments() {
        for f in ALL {
            let s = served(&flow_schema(&f, &d));
            for t in FOREACH_SPELLINGS {
                assert!(!task_accepted(t, f, &d), "{name} {f:?}: {t} is accepted");
                assert!(
                    !s.task_spellings.contains_key(t),
                    "{name} {f:?}: {t} is offered"
                );
            }
            for t in ["io.kestra.plugin.core.flow.Loop", "core.loop"] {
                assert!(
                    s.task_spellings.contains_key(t),
                    "{name} {f:?}: {t} is not offered"
                );
            }
        }
    }
}

/// Test 1: the schema offers exactly the type spellings parse and publish
/// accept, for each combination of features.
#[test]
fn the_served_type_set_is_the_accepted_type_set_for_every_feature_combination() {
    let kestra_tasks = kestra_type_names("tasks");
    let kestra_triggers = kestra_type_names("triggers");
    for (name, d) in deployments() {
        for f in ALL {
            let schema = flow_schema(&f, &d);
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
            // `ForEach` is in no export and no registry, so it would never be
            // a candidate and this equality could say nothing about it. It
            // is named so the equality covers it (the 2026-10-12 ruling).
            candidates.extend(FOREACH_SPELLINGS.iter().map(|t| t.to_string()));
            let accepted: BTreeSet<String> = candidates
                .into_iter()
                .filter(|t| task_accepted(t, f, &d))
                .collect();
            let offered: BTreeSet<String> = s.task_spellings.keys().cloned().collect();
            assert_eq!(
                offered,
                accepted,
                "{name} {f:?}: offered-but-refused {:?}, accepted-but-not-offered {:?}",
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
            assert_eq!(offered, accepted, "{name} {f:?}: triggers");
        }
    }
}

/// Every Kestra FQCN egret accepts is served from Kestra's own definition
/// (so `trim.py`'s `KESTRA_TYPES` cannot fall behind the registry).
#[test]
fn every_accepted_kestra_class_is_backed_by_the_export() {
    let all = flow_schema(&ALL[3], &permissive_deployment());
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
    let all = flow_schema(&ALL[3], &permissive_deployment());
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
        let schema = flow_schema(&f, &permissive_deployment());
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
    let schema = flow_schema(&ALL[3], &permissive_deployment());
    let mut checked = 0;
    let mut nested = 0;
    // Every served definition with a measured row: the types AND the nested
    // definitions (inputs, retry, the trigger and flow sub-objects).
    for (name, def) in schema["$defs"].as_object().unwrap() {
        let Some(row) = table.get(name) else {
            continue; // egret-only, hand-written, or passed through (README)
        };
        if !name.contains(".plugin.") && !name.starts_with("kestra.") {
            nested += 1;
        }
        let Some(props) = def["properties"].as_object() else {
            continue;
        };
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
    assert!(checked > 300, "only {checked} properties were checked");
    assert!(nested > 30, "only {nested} nested definitions were checked");
}

/// Fix round 1, I1: the five properties egret parses and then refuses at a
/// later stage are not offered.
#[test]
fn properties_egret_refuses_after_parsing_are_not_served() {
    let schema = flow_schema(&ALL[3], &permissive_deployment());
    let defs = &schema["$defs"];
    for (def, prop) in [
        ("io.kestra.plugin.core.trigger.Webhook", "wait"),
        ("io.kestra.plugin.core.trigger.Webhook", "returnOutputs"),
        (
            "io.kestra.plugin.core.trigger.Webhook",
            "responseContentType",
        ),
        ("io.kestra.plugin.core.trigger.Webhook", "responseCode"),
        ("io.kestra.plugin.core.trigger.Schedule", "lateMaximumDelay"),
    ] {
        assert!(defs[def]["properties"].is_object(), "{def} is served");
        assert!(
            defs[def]["properties"].get(prop).is_none(),
            "{def} offers {prop}, which egret refuses"
        );
        assert!(
            measured().refused_downstream[def].contains_key(prop),
            "{def}.{prop} names its refusal site"
        );
    }
}

/// The 2026-10-11 ruling: a `Pause` declaring `behavior`, `onPause` or
/// `errors` is a validation error (`unsupported_pause_property`), so the
/// measurement classifies the three `refused` and the editor stops offering
/// them. (`tasks`, the fourth property `pause_gap` names, is not in Kestra's
/// documented `Pause` schema, so there is no row to classify.) The control is
/// `pauseDuration` and `timeout`, which are honoured and still offered.
#[test]
fn a_pause_does_not_offer_the_properties_egret_refuses() {
    let schema = flow_schema(&ALL[3], &permissive_deployment());
    let pause = "io.kestra.plugin.core.flow.Pause";
    let props = &schema["$defs"][pause]["properties"];
    assert!(props.is_object(), "Pause is served");
    for prop in ["behavior", "onPause", "errors"] {
        assert_eq!(
            measured().properties[pause].get(prop),
            Some(&Treatment::Refused),
            "{prop}"
        );
        assert!(
            props.get(prop).is_none(),
            "Pause offers {prop}, which egret refuses"
        );
    }
    assert_eq!(measured().properties[pause].get("tasks"), None);
    for prop in ["pauseDuration", "timeout"] {
        assert_eq!(
            measured().properties[pause].get(prop),
            Some(&Treatment::Honoured),
            "{prop}"
        );
        assert!(props.get(prop).is_some(), "Pause must still offer {prop}");
    }
}

/// Fix round 1, I2: an enum value refused at firing is not offered.
#[test]
fn webhook_fetch_type_does_not_offer_store() {
    let schema = flow_schema(&ALL[3], &permissive_deployment());
    let fetch =
        &schema["$defs"]["io.kestra.plugin.core.trigger.Webhook"]["properties"]["fetchType"];
    let values: Vec<&str> = fetch["enum"]
        .as_array()
        .expect("fetchType keeps its enum")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(values, ["NONE", "FETCH"], "{fetch}");
    assert!(
        fetch["markdownDescription"]
            .as_str()
            .unwrap()
            .ends_with("**Not accepted by egret:** `STORE`."),
        "the hover says STORE is refused: {fetch}"
    );
}

/// Fix round 1, I2: the flow's `inputs` offer exactly the keys
/// `models::Input` keeps -- across all input definitions, and no definition
/// offers a key outside them.
#[test]
fn the_inputs_offer_exactly_the_fields_models_input_accepts() {
    let schema = flow_schema(&ALL[3], &permissive_deployment());
    let fields: BTreeSet<String> = wire_fields_of::<egret_workflow::models::Input>()
        .into_iter()
        .map(str::to_string)
        .collect();
    let refs: Vec<String> = schema["properties"]["inputs"]["items"]["anyOf"]
        .as_array()
        .expect("inputs.items is an anyOf of the input definitions")
        .iter()
        .map(|r| {
            r["$ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("#/$defs/")
                .to_string()
        })
        .collect();
    assert!(refs.len() > 15, "{refs:?}");
    let mut union = BTreeSet::new();
    for name in &refs {
        let def = &schema["$defs"][name];
        let target = def["allOf"][0]["$ref"]
            .as_str()
            .map(|r| r.trim_start_matches("#/$defs/").to_string());
        let def = match &target {
            Some(t) => &schema["$defs"][t],
            None => def,
        };
        for p in def["properties"].as_object().unwrap().keys() {
            assert!(
                fields.contains(p),
                "{name} offers {p}, which models::Input drops"
            );
            union.insert(p.clone());
        }
    }
    assert_eq!(union, fields);
    // The ones the review named, by name. Inputs package A1 modelled them, so
    // they are offered now; `jsonSchema` stays out (spec D12: not modelled,
    // `not_honoured`).
    let select = &schema["$defs"]["io.kestra.core.models.flows.input.SelectInput-2"]["properties"];
    for p in [
        "values",
        "allowCustomValue",
        "expression",
        "isRadio",
        "autoSelectFirst",
    ] {
        assert!(select.get(p).is_some(), "SELECT does not offer {p}");
    }
    let string = &schema["$defs"]["io.kestra.core.models.flows.input.StringInput-2"]["properties"];
    assert!(string.get("validator").is_some());
    let int = &schema["$defs"]["io.kestra.core.models.flows.input.IntInput-2"]["properties"];
    assert!(int.get("min").is_some() && int.get("max").is_some());
    let json = &schema["$defs"]["io.kestra.core.models.flows.input.JsonInput-2"]["properties"];
    assert!(json.get("jsonSchema").is_none(), "JSON offers jsonSchema");
}

/// Test 5: every served type with a known outputs list carries it.
#[test]
fn every_type_with_known_outputs_carries_x_egret_outputs() {
    let schema = flow_schema(&ALL[3], &permissive_deployment());
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

/// The three egret-only types. `egret.sql` is served by every deployment
/// since package #4 Task C5; the other two ([`UNSERVED`]) by none. Package #4
/// Task C6 removed `egret.dataset.refresh`, `core.http` and `core.slack`
/// (spec D10), which were six and five until then.
const EGRET_ONLY: [&str; 3] = ["egret.sql", "egret.dataapi.call", "egret.report.send"];

/// The egret-only types no deployment serves.
const UNSERVED: [&str; 2] = ["egret.dataapi.call", "egret.report.send"];

/// Test 6: egret's own types are described, and require at least the
/// fields publish resolves -- in the schema of a deployment that serves them
/// (none serves the two [`UNSERVED`] ones, so the test names such a set;
/// their definitions are kept for the day a handler is wired).
#[test]
fn egret_only_types_are_described_with_their_required_properties() {
    let with_them = egret_workflow::runtime::Served::from_type_ids(
        served_set()
            .type_ids()
            .chain(EGRET_ONLY)
            .map(str::to_string),
    );
    let schema =
        egret_workflow::spec::schema::served_with(&ALL[3], &permissive_deployment(), &with_them)
            .value
            .clone();
    for t in EGRET_ONLY {
        let def = &schema["$defs"][t];
        assert!(def.is_object(), "{t} has no definition");
        let described = ["title", "markdownDescription", "description"]
            .iter()
            .any(|k| def[k].as_str().is_some_and(|s| !s.is_empty()));
        assert!(described, "{t} has no description");
        let required = all_required(def);
        let row = registry::known_type(t).expect("registry row");
        for rf in row.references {
            assert!(required.contains(rf.field), "{t} must require {}", rf.field);
            assert!(def["properties"][rf.field].is_object(), "{t}.{}", rf.field);
        }
        assert!(required.contains("id") && required.contains("type"), "{t}");
    }
    assert_eq!(
        schema["$defs"]["egret.sql"]["required"],
        json!(["id", "type"])
    );
    assert_eq!(
        schema["$defs"]["egret.sql"]["then"]["required"],
        json!(["database", "sql"])
    );
}

/// No-silent-stall §4.4: under this binary's set none of the two unserved
/// types is offered, for any features or deployment, and `Log` and (since
/// package #4 Task C5) `egret.sql` still are.
#[test]
fn no_type_the_deployment_does_not_serve_is_offered() {
    for (name, d) in deployments() {
        for f in ALL {
            let s = served(&flow_schema(&f, &d));
            for t in UNSERVED {
                assert!(!s.task_spellings.contains_key(t), "{name} {f:?}: {t}");
            }
            assert!(s.task_spellings.contains_key("egret.sql"), "{name} {f:?}");
            assert!(
                s.task_spellings
                    .contains_key("io.kestra.plugin.core.log.Log"),
                "{name} {f:?}"
            );
        }
    }
}

/// A served type's required properties: `id` and `type` outright, the rest
/// behind `if: type is a string` (`then.required`), so that `monaco-yaml`
/// completes every `type:` spelling.
fn all_required(def: &Value) -> BTreeSet<&str> {
    [&def["required"], &def["then"]["required"]]
        .into_iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The document's shape: draft 2020-12, the flow's own properties, task
/// lists through the task union -- including a flowable's nested list.
#[test]
fn the_document_is_a_draft_2020_12_flow_schema_with_task_unions() {
    let schema = flow_schema(&ALL[0], &permissive_deployment());
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
