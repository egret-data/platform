//! W5a Task 2: the script-task model, its save-time refusals, and the ONE
//! container gate -- asserted against Task 1's measurement,
//! `oracle-exec/w5a-scripts.json` (`egret_expected`), wherever the fixture
//! holds the answer, and through the publish route for the refusals an author
//! meets.
//!
//! The pure half needs nothing; the route half is DB-gated and self-skips ONLY
//! when `EGRET_ALLOW_DB_SKIP` is set (`egret_test_db`'s contract).

mod common;

use std::collections::BTreeSet;

use axum::http::StatusCode;
use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
use egret_workflow::scripts::{
    self, default_image, gate, kind_of, parse, PullPolicy, RunnerKind, ScriptKind,
};
use serde_json::{json, Value};

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w5a-scripts.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read w5a-scripts.json"))
        .expect("parse w5a-scripts.json")
}

fn expected() -> Value {
    fixture()["egret_expected"].clone()
}

fn set(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn owned(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The four types from the fixture, each with its model kind. Asserts the
/// fixture holds exactly the four the model knows, so a loop over it cannot
/// be vacuous.
fn types() -> Vec<(String, ScriptKind, Value)> {
    let e = expected();
    let map = e["task_types"].as_object().unwrap();
    let out: Vec<(String, ScriptKind, Value)> = map
        .iter()
        .map(|(fqcn, v)| {
            let kind = kind_of(fqcn).unwrap_or_else(|| panic!("{fqcn} has no ScriptKind"));
            assert_eq!(kind.fqcn(), fqcn);
            (fqcn.clone(), kind, v.clone())
        })
        .collect();
    assert_eq!(out.len(), 4, "the fixture holds the four script types");
    out
}

/// The smallest spec that saves for `kind`.
fn minimal(kind: ScriptKind) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    match kind.body_property() {
        "commands" => m.insert("commands".into(), json!(["echo hi"])),
        _ => m.insert("script".into(), json!("print(1)")),
    };
    m
}

fn with(kind: ScriptKind, key: &str, value: Value) -> Value {
    let mut m = minimal(kind);
    m.insert(key.into(), value);
    Value::Object(m)
}

fn refusals(type_id: &str, spec: &Value) -> Vec<scripts::ScriptRefusal> {
    parse(type_id, spec).expect_err(&format!("{type_id} must refuse {spec}"))
}

// ---------------------------------------------------------------------------
// The property tables against the fixture
// ---------------------------------------------------------------------------

/// **The partition.** Every property the artifact accepts on a type is in
/// exactly one of implemented / handled-by-the-Task-model / refused, and each
/// of the three equals Task 1's list -- so no property can be silently dropped
/// from the model without this failing.
#[test]
fn the_three_property_sets_partition_what_kestra_accepts() {
    for (fqcn, kind, t) in types() {
        let implemented = owned(&kind.implemented_properties());
        let handled = owned(scripts::HANDLED_BY_TASK_MODEL);
        let refused = owned(&kind.refused_properties());
        assert_eq!(
            implemented,
            set(&t["implemented_in_w5a"]),
            "{fqcn} implemented"
        );
        assert_eq!(
            handled,
            set(&t["handled_by_egret_task_model"]),
            "{fqcn} handled"
        );
        assert_eq!(refused, set(&t["refused_in_w5a"]), "{fqcn} refused");
        assert!(implemented.is_disjoint(&handled), "{fqcn}");
        assert!(implemented.is_disjoint(&refused), "{fqcn}");
        assert!(handled.is_disjoint(&refused), "{fqcn}");
        let union: BTreeSet<String> = implemented
            .iter()
            .chain(&handled)
            .chain(&refused)
            .cloned()
            .collect();
        assert_eq!(union, set(&t["accepted_properties"]), "{fqcn} accepted");
        // The seven base-Task properties egret's model would drop are among
        // the refused (the carry from Task 1's review).
        assert!(
            set(&t["refused_core_task_properties"]).is_subset(&refused),
            "{fqcn}"
        );
    }
}

/// **Every property Task 1 lists as accepted-but-unimplemented is refused,
/// by name**, on every type, through `parse` itself (not the table).
#[test]
fn every_refused_property_is_refused_by_name() {
    let mut checked = 0;
    for (fqcn, kind, t) in types() {
        for prop in set(&t["refused_in_w5a"]) {
            let errs = refusals(&fqcn, &with(kind, &prop, json!("x")));
            assert!(
                errs.iter().any(|r| r.code == scripts::CODE_NOT_IMPLEMENTED
                    && r.path == prop
                    && r.message.contains(&prop)),
                "{fqcn}.{prop}: {errs:?}"
            );
            checked += 1;
        }
    }
    // 11 + 11 + 18 + 18.
    assert_eq!(checked, 58, "the loop saw every refused property");
}

/// The seven base-task properties, named one by one: these are the ones that
/// used to vanish silently, so each has its own assertion on its own name.
#[test]
fn the_seven_base_task_properties_are_refused_on_script_types() {
    for prop in [
        "assets",
        "logLevel",
        "logToFile",
        "policyRefs",
        "taskCache",
        "version",
        "workerSelector",
    ] {
        let errs = refusals(
            scripts::SHELL_COMMANDS,
            &with(ScriptKind::ShellCommands, prop, json!("INFO")),
        );
        assert_eq!(errs.len(), 1, "{prop}: {errs:?}");
        assert_eq!(errs[0].code, scripts::CODE_NOT_IMPLEMENTED, "{prop}");
        assert_eq!(errs[0].path, prop);
    }
}

/// What Kestra itself refuses at save (`Unrecognized field`) is refused as
/// unknown -- including W4's `image` on an FQCN spelling.
#[test]
fn an_unknown_property_is_refused_as_unknown() {
    for (fqcn, kind, _) in types() {
        for prop in ["notAProperty", "image"] {
            let errs = refusals(&fqcn, &with(kind, prop, json!("x")));
            assert_eq!(errs.len(), 1, "{fqcn}.{prop}: {errs:?}");
            assert_eq!(errs[0].code, scripts::CODE_UNKNOWN);
            assert_eq!(errs[0].path, prop);
        }
    }
}

/// `required_at_save`: `commands` or `script`; `taskRunner` is NOT required
/// (absent and `null` both save).
#[test]
fn the_body_is_required_and_task_runner_is_not() {
    for (fqcn, kind, t) in types() {
        let required = set(&t["required_at_save"]);
        assert_eq!(required, owned(&[kind.body_property()]), "{fqcn}");
        let errs = refusals(&fqcn, &json!({}));
        assert_eq!(errs.len(), 1, "{fqcn}: {errs:?}");
        assert_eq!(errs[0].code, scripts::CODE_REQUIRED);
        assert_eq!(errs[0].path, kind.body_property());
        // An explicit null is absent, as in Kestra.
        let errs = refusals(&fqcn, &json!({ kind.body_property(): null }));
        assert_eq!(errs[0].code, scripts::CODE_REQUIRED, "{fqcn}");

        assert!(t["task_runner_null_accepted_at_save"].as_bool().unwrap());
        let absent = parse(&fqcn, &Value::Object(minimal(kind))).unwrap();
        assert_eq!(absent.task_runner, None);
        let null = parse(&fqcn, &with(kind, "taskRunner", Value::Null)).unwrap();
        assert_eq!(null.task_runner, None);
    }
}

/// Defaults, from Task 1: the default images, `failFast` true, the default
/// interpreter left to the runner (`None`).
#[test]
fn defaults_match_the_measurement() {
    for (fqcn, kind, t) in types() {
        assert_eq!(
            default_image(kind),
            t["default_image"].as_str().unwrap(),
            "{fqcn}"
        );
        let parsed = parse(&fqcn, &Value::Object(minimal(kind))).unwrap();
        assert_eq!(parsed.image(), t["default_image"].as_str().unwrap());
        assert_eq!(parsed.container_image, None);
        assert_eq!(parsed.fail_fast, t["default_fail_fast"].as_bool().unwrap());
        assert_eq!(parsed.interpreter, None);
    }
    assert_eq!(expected()["fail_fast"]["default"], json!(true));
    // The two literal defaults, so a fixture edit cannot move them unseen.
    assert_eq!(default_image(ScriptKind::ShellCommands), "ubuntu");
    assert_eq!(default_image(ScriptKind::PythonScript), "python:3.13-slim");
}

/// `failFast: false` is read (the handler prepends `set -e` iff it is true).
#[test]
fn fail_fast_false_is_read() {
    let t = parse(
        scripts::SHELL_COMMANDS,
        &with(ScriptKind::ShellCommands, "failFast", json!(false)),
    )
    .unwrap();
    assert!(!t.fail_fast);
}

/// `inputFiles`: an inline map is implemented; a string (a storage URI) is
/// refused by name.
#[test]
fn input_files_inline_map_is_read_and_a_uri_is_refused() {
    let forms = &expected()["task_types"][scripts::SHELL_COMMANDS]["input_files_forms"];
    assert_eq!(forms["inline_map"], "implemented");
    assert_eq!(forms["string"], "refused");
    let t = parse(
        scripts::SHELL_COMMANDS,
        &with(
            ScriptKind::ShellCommands,
            "inputFiles",
            json!({"in.txt": "hello-input", "sub/nested.txt": "nested {{ flow.id }}"}),
        ),
    )
    .unwrap();
    assert_eq!(t.input_files["sub/nested.txt"], "nested {{ flow.id }}");
    let errs = refusals(
        scripts::SHELL_COMMANDS,
        &with(
            ScriptKind::ShellCommands,
            "inputFiles",
            json!("kestra:///ns/file.txt"),
        ),
    );
    assert_eq!(errs[0].code, scripts::CODE_INPUT_FILES_URI);
    assert_eq!(errs[0].path, "inputFiles");
}

/// `warningOnStdErr` is accepted (Task 1: no effect on state).
#[test]
fn warning_on_std_err_is_accepted() {
    assert_eq!(expected()["warning_on_stderr"]["effect_on_state"], "none");
    for v in [true, false] {
        let t = parse(
            scripts::SHELL_COMMANDS,
            &with(ScriptKind::ShellCommands, "warningOnStdErr", json!(v)),
        )
        .unwrap();
        assert_eq!(t.warning_on_std_err, v);
    }
}

// ---------------------------------------------------------------------------
// taskRunner
// ---------------------------------------------------------------------------

fn runner(kind: ScriptKind, r: Value) -> Value {
    with(kind, "taskRunner", r)
}

/// **The `Process` runner is refused at save**, by name; `RunnerKind` has no
/// variant for it, so this is the only thing a Process runner can become.
#[test]
fn the_process_runner_is_refused() {
    let e = expected();
    assert_eq!(
        e["task_runners"][scripts::PROCESS_RUNNER]["egret"],
        "refused"
    );
    for (fqcn, kind, _) in types() {
        let errs = refusals(
            &fqcn,
            &runner(kind, json!({"type": scripts::PROCESS_RUNNER})),
        );
        assert_eq!(errs.len(), 1, "{fqcn}: {errs:?}");
        assert_eq!(errs[0].code, scripts::CODE_RUNNER_REFUSED);
        assert_eq!(errs[0].path, "taskRunner.type");
        assert!(errs[0].message.contains(scripts::PROCESS_RUNNER));
    }
}

#[test]
fn an_unknown_runner_type_is_refused() {
    let errs = refusals(
        scripts::SHELL_COMMANDS,
        &runner(
            ScriptKind::ShellCommands,
            json!({"type": "io.kestra.plugin.scripts.runner.docker.Dockr"}),
        ),
    );
    assert_eq!(errs[0].code, scripts::CODE_RUNNER_UNKNOWN);
}

/// Docker: the defaults (`IF_NOT_PRESENT`, `delete: true`), the implemented
/// set read, every refused property refused by name, `cpu.cpus` and
/// `memory.memory` read, the other five `memory` sub-properties refused.
#[test]
fn the_docker_runner_matches_the_fixture() {
    let e = expected();
    let d = &e["task_runners"][scripts::DOCKER_RUNNER];
    assert_eq!(
        set(&d["implemented_in_w5a"]),
        owned(scripts::DOCKER_IMPLEMENTED)
    );
    assert_eq!(set(&d["refused_in_w5a"]), owned(scripts::DOCKER_REFUSED));
    let union: BTreeSet<String> = owned(scripts::DOCKER_IMPLEMENTED)
        .union(&owned(scripts::DOCKER_REFUSED))
        .cloned()
        .collect();
    assert_eq!(union, set(&d["accepted_properties"]));
    assert_eq!(
        set(&d["memory_properties"]["implemented_in_w5a"]),
        owned(scripts::DOCKER_MEMORY_IMPLEMENTED)
    );
    assert_eq!(
        set(&d["memory_properties"]["refused_in_w5a"]),
        owned(scripts::DOCKER_MEMORY_REFUSED)
    );
    assert_eq!(
        set(&d["cpu_properties"]["implemented_in_w5a"]),
        owned(scripts::DOCKER_CPU_IMPLEMENTED)
    );
    assert_eq!(d["defaults"]["pullPolicy"], "IF_NOT_PRESENT");
    assert_eq!(d["defaults"]["delete"], true);

    let k = ScriptKind::ShellCommands;
    let t = parse(
        scripts::SHELL_COMMANDS,
        &runner(k, json!({"type": scripts::DOCKER_RUNNER})),
    )
    .unwrap();
    let r = t.task_runner.unwrap();
    assert_eq!(r.kind, RunnerKind::Docker);
    assert_eq!(r.pull_policy, PullPolicy::IfNotPresent);
    assert!(r.delete);

    let t = parse(
        scripts::SHELL_COMMANDS,
        &runner(
            k,
            json!({"type": scripts::DOCKER_RUNNER, "pullPolicy": "NEVER", "delete": false,
                   "cpu": {"cpus": 1.5}, "memory": {"memory": "512MB"}}),
        ),
    )
    .unwrap();
    let r = t.task_runner.unwrap();
    assert_eq!(r.pull_policy, PullPolicy::Never);
    assert!(!r.delete);
    assert_eq!(r.cpu.as_deref(), Some("1.5"));
    assert_eq!(r.memory.as_deref(), Some("512MB"));

    for prop in set(&d["refused_in_w5a"]) {
        let errs = refusals(
            scripts::SHELL_COMMANDS,
            &runner(
                k,
                json!({"type": scripts::DOCKER_RUNNER, prop.clone(): "x"}),
            ),
        );
        assert_eq!(errs.len(), 1, "{prop}: {errs:?}");
        assert_eq!(
            errs[0].code,
            scripts::CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED,
            "{prop}"
        );
        assert_eq!(errs[0].path, format!("taskRunner.{prop}"));
    }
    for sub in set(&d["memory_properties"]["refused_in_w5a"]) {
        let errs = refusals(
            scripts::SHELL_COMMANDS,
            &runner(
                k,
                json!({"type": scripts::DOCKER_RUNNER, "memory": {sub.clone(): "1"}}),
            ),
        );
        assert_eq!(errs.len(), 1, "{sub}: {errs:?}");
        assert_eq!(errs[0].code, scripts::CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED);
        assert_eq!(errs[0].path, format!("taskRunner.memory.{sub}"));
    }
    let errs = refusals(
        scripts::SHELL_COMMANDS,
        &runner(k, json!({"type": scripts::DOCKER_RUNNER, "notAThing": 1})),
    );
    assert_eq!(errs[0].code, scripts::CODE_RUNNER_PROPERTY_UNKNOWN);
    let errs = refusals(
        scripts::SHELL_COMMANDS,
        &runner(
            k,
            json!({"type": scripts::DOCKER_RUNNER, "pullPolicy": "SOMETIMES"}),
        ),
    );
    assert_eq!(errs[0].code, scripts::CODE_INVALID);
}

/// Kubernetes: a closed set. `resources` accepts exactly the four paths;
/// anything else, at any level, is refused by path.
#[test]
fn the_kubernetes_runner_is_a_closed_set() {
    let e = expected();
    let kd = &e["task_runners"][scripts::KUBERNETES_RUNNER];
    assert_eq!(kd["egret"], "implemented");
    assert_eq!(
        set(&kd["implemented_in_w5a"]),
        owned(scripts::KUBERNETES_IMPLEMENTED)
    );
    assert_eq!(
        set(&kd["resources_implemented_paths"]),
        owned(scripts::KUBERNETES_RESOURCE_PATHS)
    );
    let k = ScriptKind::PythonScript;
    let t = parse(
        scripts::PYTHON_SCRIPT,
        &runner(
            k,
            json!({"type": scripts::KUBERNETES_RUNNER, "pullPolicy": "ALWAYS",
                   "resources": {"request": {"cpu": "250m", "memory": "256Mi"},
                                 "limit": {"cpu": 1, "memory": "1Gi"}}}),
        ),
    )
    .unwrap();
    let r = t.task_runner.unwrap();
    assert_eq!(r.kind, RunnerKind::Kubernetes);
    assert_eq!(r.pull_policy, PullPolicy::Always);
    assert!(r.delete);
    let res = r.resources.unwrap();
    assert_eq!(res.request.cpu.as_deref(), Some("250m"));
    assert_eq!(res.limit.cpu.as_deref(), Some("1"));
    assert_eq!(res.limit.memory.as_deref(), Some("1Gi"));

    for (bad, path) in [
        (json!({"namespace": "x"}), "taskRunner.namespace"),
        (
            json!({"serviceAccountName": "x"}),
            "taskRunner.serviceAccountName",
        ),
        (
            json!({"resources": {"requests": {}}}),
            "taskRunner.resources.requests",
        ),
        (
            json!({"resources": {"limit": {"nvidia.com/gpu": 1}}}),
            "taskRunner.resources.limit.nvidia.com/gpu",
        ),
    ] {
        let mut r = bad.as_object().unwrap().clone();
        r.insert("type".into(), json!(scripts::KUBERNETES_RUNNER));
        let errs = refusals(scripts::PYTHON_SCRIPT, &runner(k, Value::Object(r)));
        assert_eq!(errs.len(), 1, "{path}: {errs:?}");
        assert_eq!(errs[0].code, scripts::CODE_RUNNER_PROPERTY_NOT_IMPLEMENTED);
        assert_eq!(errs[0].path, path);
    }
}

// ---------------------------------------------------------------------------
// Aliases
// ---------------------------------------------------------------------------

/// The two W4 spellings are aliases of the fixture's FQCNs, in the model and
/// in publish's table; they accept W4's `image`, and refuse it beside
/// `containerImage`.
#[test]
fn the_aliases_are_the_fixtures() {
    let e = expected();
    let aliases = e["aliases"].as_object().unwrap();
    assert_eq!(aliases.len(), 2);
    for (alias, fqcn) in aliases {
        let fqcn = fqcn.as_str().unwrap();
        assert_eq!(kind_of(alias), kind_of(fqcn), "{alias}");
        let row = egret_workflow::spec::registry::known_type(alias).expect("alias row");
        assert_eq!(row.r#type, fqcn, "{alias}");
        assert!(row.aliases.contains(&alias.as_str()));
        assert!(row.references.is_empty());
    }
    for kind in ScriptKind::ALL {
        let row = egret_workflow::spec::registry::known_type(kind.fqcn()).expect("fqcn row");
        assert_eq!(row.r#type, kind.fqcn());
    }

    // W4's own shapes still parse under the aliases.
    let t = parse(
        scripts::ALIAS_CONTAINER,
        &json!({"image": "ubuntu:24.04", "commands": ["echo hi"], "interpreter": ["/bin/bash", "-c"]}),
    )
    .unwrap();
    assert_eq!(t.image(), "ubuntu:24.04");
    assert_eq!(t.kind, ScriptKind::ShellCommands);
    let t = parse(
        scripts::ALIAS_SCRIPT_PYTHON,
        &json!({"script": "print(1)", "beforeCommands": ["pip install x"]}),
    )
    .unwrap();
    assert_eq!(t.image(), "python:3.13-slim");
    let errs = refusals(
        scripts::ALIAS_CONTAINER,
        &json!({"image": "a", "containerImage": "b", "commands": ["x"]}),
    );
    assert_eq!(errs[0].code, scripts::CODE_IMAGE_CONFLICT);
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

fn configs(entries: &[(&str, &[&str])]) -> PluginConfigurations {
    PluginConfigurations(
        entries
            .iter()
            .map(|(t, imgs)| PluginConfiguration {
                r#type: t.to_string(),
                values: PluginConfigurationValues {
                    allowed_images: imgs.iter().map(|s| s.to_string()).collect(),
                },
            })
            .collect(),
    )
}

fn check(
    feature: bool,
    image: &str,
    runner: RunnerKind,
    pc: &PluginConfigurations,
) -> Result<(), gate::GateRefusal> {
    gate::check(&gate::GateInput {
        feature_enabled: feature,
        image,
        runner,
        plugin_configurations: pc,
    })
}

/// Image references normalise exactly as Task 1 measured / the spec decided.
#[test]
fn image_references_normalise_as_the_fixture_says() {
    let e = expected();
    let cases = e["image_reference_normalisation"]["cases"]
        .as_array()
        .unwrap();
    assert!(cases.len() >= 7);
    for c in cases {
        assert_eq!(
            gate::normalise_image_ref(c["written"].as_str().unwrap()),
            c["matched_as"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn the_gate_refuses_when_the_feature_is_off() {
    let pc = configs(&[(scripts::DOCKER_RUNNER, &["*"])]);
    assert_eq!(
        check(false, "ubuntu", RunnerKind::Docker, &pc),
        Err(gate::GateRefusal::FeatureDisabled)
    );
    assert_eq!(check(true, "ubuntu", RunnerKind::Docker, &pc), Ok(()));
}

#[test]
fn the_gate_refuses_an_image_outside_the_allowlist() {
    let pc = configs(&[(scripts::DOCKER_RUNNER, &["python:*"])]);
    let err = check(true, "ubuntu", RunnerKind::Docker, &pc).unwrap_err();
    assert_eq!(
        err,
        gate::GateRefusal::ImageNotAllowed {
            image: "ubuntu".into(),
            matched_as: "ubuntu:latest".into(),
            runner: scripts::DOCKER_RUNNER,
        }
    );
    assert!(err.to_string().contains("'ubuntu'"), "{err}");
    // No Docker Hub normalisation in either direction.
    assert!(check(
        true,
        "docker.io/library/python:3.13-slim",
        RunnerKind::Docker,
        &pc
    )
    .is_err());
}

#[test]
fn the_gate_refuses_everything_on_an_empty_list() {
    let pc = configs(&[(scripts::DOCKER_RUNNER, &[])]);
    for image in ["ubuntu", "python:3.13-slim", "*", ""] {
        assert_eq!(
            check(true, image, RunnerKind::Docker, &pc),
            Err(gate::GateRefusal::EmptyAllowlist {
                runner: scripts::DOCKER_RUNNER
            }),
            "{image}"
        );
    }
}

#[test]
fn the_gate_refuses_a_runner_with_no_entry() {
    let pc = configs(&[(scripts::DOCKER_RUNNER, &["*"])]);
    assert_eq!(
        check(true, "ubuntu", RunnerKind::Kubernetes, &pc),
        Err(gate::GateRefusal::NoPluginConfiguration {
            runner: scripts::KUBERNETES_RUNNER
        })
    );
    assert_eq!(
        check(
            true,
            "ubuntu",
            RunnerKind::Docker,
            &PluginConfigurations::default()
        ),
        Err(gate::GateRefusal::NoPluginConfiguration {
            runner: scripts::DOCKER_RUNNER
        })
    );
}

#[test]
fn the_gate_accepts_a_matching_glob_and_a_tagless_image() {
    let pc = configs(&[(
        scripts::KUBERNETES_RUNNER,
        &[
            "python:3.13-*",
            "ubuntu:latest",
            "registry.local:5000/team/*",
        ],
    )]);
    let k = RunnerKind::Kubernetes;
    assert_eq!(check(true, "python:3.13-slim", k, &pc), Ok(()));
    // Tagless -> `:latest` (Task 1: `containerImage: python` pulled
    // python:latest).
    assert_eq!(check(true, "ubuntu", k, &pc), Ok(()));
    assert_eq!(check(true, "registry.local:5000/team/img", k, &pc), Ok(()));
    assert!(
        check(true, "python", k, &pc).is_err(),
        "python -> python:latest"
    );
    let e = expected();
    assert_eq!(
        e["tagless_image"]["allowlist_matches_against"],
        "python:latest"
    );
    let pc = configs(&[(scripts::KUBERNETES_RUNNER, &["python:latest"])]);
    assert_eq!(check(true, "python", k, &pc), Ok(()));
}

/// **A star never crosses `/`**, so a glob cannot forge a registry host: a
/// tag glob does not match a host-and-port reference, and a registry prefix
/// does not match a longer host that starts with it.
#[test]
fn a_glob_cannot_reach_another_registry_host() {
    let k = RunnerKind::Kubernetes;
    let pc = configs(&[(
        scripts::KUBERNETES_RUNNER,
        &["python:3*", "python*", "registry.company.com/*"],
    )]);
    // The controls: each pattern does admit what it was written for.
    assert_eq!(check(true, "python:3.13-slim", k, &pc), Ok(()));
    assert_eq!(check(true, "registry.company.com/img:1", k, &pc), Ok(()));
    // `python:3000/x/img` is host `python`, port 3000.
    assert!(check(true, "python:3000/x/img", k, &pc).is_err());
    assert!(check(true, "python:3000/x/img:1", k, &pc).is_err());
    assert!(check(true, "python.attacker.io/x:1", k, &pc).is_err());
    assert!(check(true, "registry.company.com.evil.io/x", k, &pc).is_err());
    // One level only: a deeper path needs `/*/*`.
    assert!(check(true, "registry.company.com/team/img:1", k, &pc).is_err());
    assert!(!gate::glob_match("python:3*", "python:3000/x/img:latest"));
}

/// An unrendered image skips only the image match: the feature and the
/// allowlist's existence are still refused.
#[test]
fn check_unrendered_is_check_without_the_image() {
    let d = RunnerKind::Docker;
    let pc = configs(&[(scripts::DOCKER_RUNNER, &["python:*"])]);
    assert_eq!(gate::check_unrendered(true, d, &pc), Ok(()));
    assert_eq!(
        gate::check_unrendered(false, d, &pc),
        Err(gate::GateRefusal::FeatureDisabled)
    );
    assert!(gate::check_unrendered(true, d, &configs(&[(scripts::DOCKER_RUNNER, &[])])).is_err());
    assert!(gate::check_unrendered(true, RunnerKind::Kubernetes, &pc).is_err());
    assert!(gate::is_unrendered("{{ inputs.image }}"));
    assert!(!gate::is_unrendered("python:3.13-slim"));
}

/// The gate's feature is the catalog's, and it is off by default.
#[test]
fn the_feature_key_is_catalogued_off_by_default() {
    assert_eq!(
        egret_workspace::features::default_enabled(scripts::FEATURE_KEY),
        Some(false)
    );
}

// ---------------------------------------------------------------------------
// Validate reports parse refusals as errors, at every depth
// ---------------------------------------------------------------------------

#[test]
fn validate_reports_script_refusals_as_errors() {
    let flow = egret_workflow::spec::parse(
        "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: ok\n        type: core.log\n        message: x\n    errors:\n      - id: t\n        type: io.kestra.plugin.scripts.shell.Commands\n        commands: [x]\n        logLevel: DEBUG\n",
    )
    .unwrap();
    let report = egret_workflow::spec::validate(&flow);
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code == scripts::CODE_NOT_IMPLEMENTED && f.path == "tasks.s.t.logLevel"),
        "{report:?}"
    );
}

// ---------------------------------------------------------------------------
// Through the publish route
// ---------------------------------------------------------------------------

fn settings(entries: &[(&str, &[&str])]) -> scripts::ScriptSettings {
    scripts::ScriptSettings {
        plugin_configurations: configs(entries),
        default_runner: Some(RunnerKind::Docker),
    }
}

fn flow(ns: &str, task: &str) -> String {
    format!("id: f\nnamespace: {ns}\ntasks:\n  - id: t\n{task}")
}

/// Create a draft through the route and publish it; the publish response.
async fn publish(
    app: &common::App,
    ws: uuid::Uuid,
    user: egret_core::UserId,
    source: &str,
) -> (StatusCode, String) {
    let created = common::json(
        common::post(
            app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            json!({ "source": source }),
        )
        .await,
    )
    .await;
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"));
    let res = common::post(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/publish"),
        user,
        json!({}),
    )
    .await;
    let status = res.status();
    (status, common::body_text(res).await)
}

const SHELL: &str = "    type: io.kestra.plugin.scripts.shell.Commands\n    commands: [echo hi]\n";

#[tokio::test]
async fn publish_is_refused_with_each_named_error() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let allow = settings(&[
        (scripts::DOCKER_RUNNER, &["ubuntu:*"]),
        (scripts::KUBERNETES_RUNNER, &[]),
    ]);
    let app = common::app_with_scripts(&db, allow).await;

    // 1. No `workflow_containers` row: OFF by default.
    let (s, body) = publish(&app, ws, user, &flow("w5a.off", SHELL)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("'workflow_containers' feature is disabled"),
        "{body}"
    );

    common::enable_feature(&db, ws, "workflow_containers").await;

    // 2. Outside the allowlist.
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.image",
            &format!("{SHELL}    containerImage: alpine:3\n"),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("image 'alpine:3'") && body.contains("not in the 'allowed-images'"),
        "{body}"
    );

    // 3. An empty list (Kubernetes here).
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.empty",
            &format!(
                "{SHELL}    taskRunner:\n      type: {}\n",
                scripts::KUBERNETES_RUNNER
            ),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("'allowed-images' list for task runner"),
        "{body}"
    );

    // 4. The Process runner.
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.process",
            &format!(
                "{SHELL}    taskRunner:\n      type: {}\n",
                scripts::PROCESS_RUNNER
            ),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("is refused by egret"), "{body}");

    // 5. An unimplemented property, by name.
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.prop",
            &format!("{SHELL}    namespaceFiles:\n      enabled: true\n"),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("property 'namespaceFiles'"), "{body}");

    // And the control: the same task, allowed, publishes.
    let (s, body) = publish(&app, ws, user, &flow("w5a.ok", SHELL)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

/// With no default runner configured, a script task without `taskRunner:` is
/// refused (nothing would run it); with one, it is gated on that runner.
#[tokio::test]
async fn publish_without_a_default_runner_refuses_an_absent_task_runner() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let mut s = settings(&[(scripts::DOCKER_RUNNER, &["*"])]);
    s.default_runner = None;
    let app = common::app_with_scripts(&db, s).await;
    let (st, body) = publish(&app, ws, user, &flow("w5a.norunner", SHELL)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("no default container runner"), "{body}");
    let (st, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.explicit",
            &format!(
                "{SHELL}    taskRunner:\n      type: {}\n",
                scripts::DOCKER_RUNNER
            ),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// **The alias `egret.container` still publishes** -- W4's shape, `image:`
/// and all -- once the workspace has the feature and the image is allowed;
/// and `egret.script.python` with its default image.
#[tokio::test]
async fn the_w4_aliases_still_publish() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let app = common::app_with_scripts(
        &db,
        settings(&[(scripts::DOCKER_RUNNER, &["ubuntu:*", "python:3.13-slim"])]),
    )
    .await;
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.alias.container",
            "    type: egret.container\n    image: ubuntu\n    commands: [echo hi]\n    outputFiles: [out.txt]\n",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = publish(
        &app,
        ws,
        user,
        &flow(
            "w5a.alias.python",
            "    type: egret.script.python\n    script: print(1)\n",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

/// An image written as an expression is gated at run (Task 5); at publish it
/// still needs the feature and a non-empty allowlist.
#[tokio::test]
async fn an_unrendered_image_publishes_but_not_past_the_feature() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app =
        common::app_with_scripts(&db, settings(&[(scripts::DOCKER_RUNNER, &["python:*"])])).await;
    let task = format!("{SHELL}    containerImage: \"{{{{ inputs.image }}}}\"\n");
    let (s, body) = publish(&app, ws, user, &flow("w5a.tpl.off", &task)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("workflow_containers"), "{body}");
    common::enable_feature(&db, ws, "workflow_containers").await;
    let (s, body) = publish(&app, ws, user, &flow("w5a.tpl.on", &task)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

/// `publish` (no gate) is fail-closed: a script task never publishes through
/// it, even in a workspace with the feature on.
#[tokio::test]
async fn the_ungated_publish_entry_point_refuses_script_tasks() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(&repo, ws, user, &flow("w5a.closed", SHELL)).await;
    let refs = common::workflow_refs(&db);
    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_core::Error::InvalidInput(ref m) if m.contains("workflow_containers")),
        "{err:?}"
    );
}
