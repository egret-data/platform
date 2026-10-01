//! W5b Task 3: the `kestra.<FQCN>` plugin-task model -- parsed against the
//! embedded `catalog.json`, refused at publish by name, and governed by its
//! own gate (feature `workflow_kestra_plugins`, which requires
//! `workflow_containers`, and the `kestra` entry's `allowed-types`) -- plus
//! the refusal of `pluginDefaults:` on every flow. One test per row of spec
//! §10 this task owns; the refusals are asserted against Task 1's
//! measurement, `oracle-exec/w5b-plugins.json` (`egret_expected`), where the
//! fixture holds the answer.
//!
//! The pure half needs nothing; the route half is DB-gated and self-skips ONLY
//! when `EGRET_ALLOW_DB_SKIP` is set (`egret_test_db`'s contract).

mod common;

use axum::http::StatusCode;
use egret_config::{PluginConfiguration, PluginConfigurationValues, PluginConfigurations};
use egret_workflow::plugins::{self, gate};
use egret_workflow::scripts::{self, RunnerKind};
use egret_workflow::spec;
use serde_json::{json, Value};

const PG_QUERY: &str = "io.kestra.plugin.jdbc.postgresql.Query";
const MYSQL_QUERY: &str = "io.kestra.plugin.jdbc.mysql.Query";

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w5b-plugins.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read w5b-plugins.json"))
        .expect("parse w5b-plugins.json")
}

fn prefixed(fqcn: &str) -> String {
    format!("{}{fqcn}", plugins::PREFIX)
}

/// The refusals `parse` answers for `spec` on `kestra.<fqcn>`; panics if it
/// parses.
fn refusals(fqcn: &str, spec: Value) -> Vec<plugins::PluginRefusal> {
    match plugins::parse(&prefixed(fqcn), &spec) {
        Ok(t) => panic!("{fqcn} {spec} parsed: {:?}", t.properties),
        Err(r) => r,
    }
}

/// The one refusal at `path`, asserting its code and a message substring.
fn assert_refused(errs: &[plugins::PluginRefusal], path: &str, code: &str, needle: &str) {
    let r = errs
        .iter()
        .find(|r| r.path == path)
        .unwrap_or_else(|| panic!("no refusal at {path:?}: {errs:?}"));
    assert_eq!(r.code, code, "{r:?}");
    assert!(
        r.message.contains(needle),
        "{needle:?} not in {:?}",
        r.message
    );
}

fn pg(extra: Value) -> Value {
    let mut base = json!({"url": "jdbc:postgresql://h/db", "sql": "select 1"});
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    base
}

fn errors_of(src: &str) -> Vec<spec::Finding> {
    let flow = spec::parse(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
    spec::validate(&flow).errors
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

#[test]
fn a_catalog_fqcn_with_the_prefix_parses() {
    let t = plugins::parse(
        "kestra.io.kestra.plugin.jdbc.postgresql.Query",
        &json!({"url": "jdbc:postgresql://h/db", "sql": "select 1"}),
    )
    .unwrap();
    assert_eq!(t.fqcn, "io.kestra.plugin.jdbc.postgresql.Query");
    assert_eq!(t.properties["sql"], "select 1");
    // Every class of the catalog is reachable through the prefix, and only
    // through it.
    for fqcn in plugins::catalog().classes.keys() {
        assert_eq!(
            plugins::lookup(&prefixed(fqcn)).map(|(f, _)| f),
            Some(fqcn.as_str())
        );
        assert!(plugins::lookup(fqcn).is_none(), "{fqcn} bare");
    }
    // W9a Task 9: W5b's five, and the four it excluded as needing storage.
    assert_eq!(plugins::catalog().classes.len(), 9);
    for fqcn in [
        "io.kestra.plugin.jdbc.postgresql.Batch",
        "io.kestra.plugin.jdbc.mysql.Batch",
        "io.kestra.plugin.jdbc.postgresql.CopyIn",
        "io.kestra.plugin.jdbc.postgresql.CopyOut",
    ] {
        assert!(plugins::catalog().classes.contains_key(fqcn), "{fqcn}");
    }
}

#[test]
fn a_bare_catalog_fqcn_is_refused_with_the_prefixed_hint() {
    assert_eq!(
        plugins::bare_hint(PG_QUERY).as_deref(),
        Some("kestra.io.kestra.plugin.jdbc.postgresql.Query")
    );
    assert_eq!(plugins::bare_hint("io.kestra.plugin.core.log.Log"), None);

    let errs = errors_of(&format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: q\n    type: {PG_QUERY}\n    url: jdbc:postgresql://h/db\n    sql: select 1\n"
    ));
    let f = errs
        .iter()
        .find(|f| f.code == plugins::CODE_TYPE_UNPREFIXED)
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert_eq!(f.path, "tasks.q");
    assert!(
        f.message
            .contains("kestra.io.kestra.plugin.jdbc.postgresql.Query"),
        "{}",
        f.message
    );

    // And a `kestra.` type that is not in the catalog is refused by name.
    let errs = errors_of(
        "id: f\nnamespace: a.b\ntasks:\n  - id: q\n    type: kestra.io.kestra.plugin.jdbc.postgresql.Trigger\n",
    );
    let f = errs
        .iter()
        .find(|f| f.code == plugins::CODE_TYPE_UNKNOWN)
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert!(
        f.message
            .contains("'io.kestra.plugin.jdbc.postgresql.Trigger' is not in the plugin catalog"),
        "{}",
        f.message
    );
}

#[test]
fn an_unknown_property_is_refused_by_name() {
    let errs = refusals(PG_QUERY, pg(json!({"sqll": "select 2"})));
    assert_refused(&errs, "sqll", plugins::CODE_PROPERTY_UNKNOWN, "'sqll'");
    assert_eq!(errs.len(), 1, "{errs:?}");
}

#[test]
fn a_missing_required_property_is_refused_by_name() {
    let errs = refusals(PG_QUERY, json!({"sql": "select 1"}));
    assert_refused(&errs, "url", plugins::CODE_PROPERTY_REQUIRED, "'url'");
    // `id` and `type` are required in the catalog too, but the `Task` model
    // consumes them; they are never demanded of the leaf spec.
    assert_eq!(errs.len(), 1, "{errs:?}");
    // An explicit `null` is absence.
    let errs = refusals(PG_QUERY, json!({"sql": "select 1", "url": null}));
    assert_refused(&errs, "url", plugins::CODE_PROPERTY_REQUIRED, "'url'");
}

fn w9a_fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read w9a-storage.json"))
        .expect("parse w9a-storage.json")
}

/// A spec for `fqcn` holding every property the catalog requires (but the
/// ones the `Task` model consumes), each a plain string.
fn minimal(fqcn: &str) -> Value {
    let class = &plugins::catalog().classes[fqcn];
    let mut spec = serde_json::Map::new();
    for (name, prop) in &class.properties {
        if prop.required && !plugins::HANDLED_BY_TASK_MODEL.contains(&name.as_str()) {
            spec.insert(name.clone(), json!(format!("{name}-value")));
        }
    }
    Value::Object(spec)
}

/// **W9a Task 9 flips W5b's `fetch_type_store_is_refused_naming_w9`**: every
/// value W5b refused as needing storage -- each row of `w5b-plugins.json`'s
/// `refused_values`, `fetchType: STORE`, `store: true`, `inputFile` and
/// `from` -- now parses on its class, which is in the catalog. The run-time
/// proofs that these plugins RUN are the handler's
/// `fetch_type_store_runs_and_its_file_is_indexed` (spy) and
/// `plugin_host_live`'s storage chain (Docker, a real Postgres, MinIO).
#[test]
fn every_value_w5b_refused_for_storage_now_parses() {
    let uri = "kestra:///w9a/plugins/f/executions/00000000-0000-0000-0000-000000000001/tasks/q/\
               00000000-0000-0000-0000-000000000002/1.ion";
    let refused = fixture()["egret_expected"]["refused_values"].clone();
    let refused = refused.as_object().expect("W5b's table");
    let mut parsed = 0;
    for (fqcn, props) in refused {
        for (property, values) in props.as_object().expect("a class row") {
            let values: Vec<Value> = match values {
                Value::String(any) => {
                    assert_eq!(any, "any");
                    vec![json!(uri), json!("{{ outputs.q.uri }}")]
                }
                Value::Array(vs) => vs.clone(),
                other => panic!("{fqcn}.{property}: {other}"),
            };
            for v in values {
                let mut spec = minimal(fqcn);
                spec[property] = v.clone();
                plugins::parse(&prefixed(fqcn), &spec)
                    .unwrap_or_else(|e| panic!("{fqcn} {property}: {v}: {e:?}"));
                parsed += 1;
            }
        }
    }
    // 8 classes; 5 × (STORE, true) + 2 inputFile × 2 + 3 from × 2 = 20.
    assert_eq!(refused.len(), 8);
    assert_eq!(parsed, 20, "every row was tried");
    // `CopyOut` needed storage as a class (it always writes): it parses too.
    plugins::parse(
        &prefixed("io.kestra.plugin.jdbc.postgresql.CopyOut"),
        &minimal("io.kestra.plugin.jdbc.postgresql.CopyOut"),
    )
    .expect("CopyOut parses");
}

/// **What egret still refuses for storage is W9a Task 1's re-measurement**,
/// `w9a-storage.json`'s `plugin_storage.still_refused_values` -- which is
/// empty (every W5b row ran its storage part on the artifact), so no refusal
/// table exists any more. Was: `REFUSED_VALUES` equals `w5b-plugins.json`'s
/// `refused_values`.
#[test]
fn nothing_is_refused_for_storage_as_w9a_task_1_measured() {
    let doc = w9a_fixture();
    let ps = &doc["egret_expected"]["plugin_storage"];
    assert_eq!(ps["basis"], "measured");
    assert_eq!(ps["still_refused_values"], json!({}));
    // The control: the same fixture records each re-admitted class as run.
    for fqcn in [
        "io.kestra.plugin.jdbc.postgresql.CopyOut",
        "io.kestra.plugin.jdbc.postgresql.CopyIn",
        "io.kestra.plugin.jdbc.postgresql.Batch",
        "io.kestra.plugin.jdbc.mysql.Batch",
    ] {
        assert!(ps[fqcn].is_object(), "{fqcn} measured");
    }
}

#[test]
fn task_runner_and_the_seven_base_properties_are_refused() {
    let seven = [
        ("assets", json!({"inputs": []})),
        ("logLevel", json!("INFO")),
        ("logToFile", json!(true)),
        ("policyRefs", json!(["p"])),
        ("taskCache", json!({"enabled": true})),
        ("version", json!("1")),
        ("workerSelector", json!({"tags": ["a"]})),
    ];
    assert_eq!(plugins::REFUSED_BASE_PROPERTIES.len(), 7);
    for (key, value) in seven {
        let errs = refusals(PG_QUERY, pg(json!({ key: value })));
        assert_refused(
            &errs,
            key,
            plugins::CODE_PROPERTY_NOT_IMPLEMENTED,
            &format!("property '{key}' is accepted by Kestra on {PG_QUERY} but is not implemented by egret"),
        );
    }
    let errs = refusals(
        PG_QUERY,
        pg(json!({"taskRunner": {"type": scripts::DOCKER_RUNNER}})),
    );
    assert_refused(
        &errs,
        "taskRunner",
        plugins::CODE_PROPERTY_NOT_IMPLEMENTED,
        "workflow.container_runner",
    );
}

#[test]
fn a_literal_of_the_wrong_plain_type_is_refused_and_an_expression_is_not() {
    // `sql` is a plain `string`; a list is not one.
    let errs = refusals(
        PG_QUERY,
        json!({"url": "jdbc:postgresql://h/db", "sql": ["a"]}),
    );
    assert_refused(&errs, "sql", plugins::CODE_PROPERTY_INVALID, "string");
    // `parameters` is a plain `object`; `outputFiles` a plain `array`.
    let errs = refusals(
        PG_QUERY,
        pg(json!({"parameters": "p", "outputFiles": {"a": 1}})),
    );
    assert_refused(
        &errs,
        "parameters",
        plugins::CODE_PROPERTY_INVALID,
        "object",
    );
    assert_refused(
        &errs,
        "outputFiles",
        plugins::CODE_PROPERTY_INVALID,
        "array",
    );
    // `fetchType` is an enum.
    let errs = refusals(PG_QUERY, pg(json!({"fetchType": "ALL"})));
    assert_refused(
        &errs,
        "fetchType",
        plugins::CODE_PROPERTY_INVALID,
        "FETCH_ONE",
    );

    // An expression is judged by the host after rendering, whatever the type.
    plugins::parse(
        &prefixed(PG_QUERY),
        &pg(json!({"parameters": "{{ inputs.p }}", "outputFiles": "{{ x }}", "fetchType": "{{ inputs.t }}"})),
    )
    .unwrap();
    // A schema egret cannot read (`anyOf`) is not type-checked at all.
    plugins::parse(&prefixed(PG_QUERY), &pg(json!({"fetchSize": [1]}))).unwrap();
    // Scalars coerce into a string, as Jackson's do.
    plugins::parse(&prefixed(PG_QUERY), &pg(json!({"password": 1234}))).unwrap();
}

#[test]
fn no_native_fqcn_is_in_the_catalog() {
    for row in spec::registry::TASK_TYPES {
        assert!(
            !plugins::catalog().classes.contains_key(row.r#type),
            "{}",
            row.r#type
        );
        for alias in row.aliases {
            assert!(!plugins::catalog().classes.contains_key(*alias), "{alias}");
        }
    }
    // The control: the catalog is not empty, so the loop above compared
    // against something.
    assert!(plugins::catalog().classes.contains_key(PG_QUERY));
}

#[test]
fn plugin_defaults_is_refused_on_every_flow_even_empty() {
    let log_only = "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    // The control: without the key, the Log-only flow is clean.
    assert!(errors_of(log_only).is_empty());

    let with_plugin = format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: q\n    type: kestra.{PG_QUERY}\n    url: jdbc:postgresql://h/db\n    sql: select 1\n"
    );
    assert!(
        errors_of(&with_plugin).is_empty(),
        "{:?}",
        errors_of(&with_plugin)
    );

    let list = "pluginDefaults:\n  - type: io.kestra.plugin.core.log.Log\n    values:\n      level: WARN\n";
    for (name, src) in [
        ("list, plugin task", format!("{with_plugin}{list}")),
        (
            "empty list, Log-only",
            format!("{log_only}pluginDefaults: []\n"),
        ),
        ("list, Log-only", format!("{log_only}{list}")),
        ("null, Log-only", format!("{log_only}pluginDefaults:\n")),
    ] {
        let errs = errors_of(&src);
        let f = errs
            .iter()
            .find(|f| f.code == plugins::CODE_REMOVED_PLUGIN_DEFAULTS)
            .unwrap_or_else(|| panic!("{name}: {errs:?}"));
        assert_eq!(f.path, "pluginDefaults", "{name}");
        assert_eq!(
            f.message,
            "`pluginDefaults` was removed in Kestra 2.0; set the properties on each task",
            "{name}"
        );
    }
    // Kestra answers all three measured shapes by naming the field.
    let k = &fixture()["egret_expected"]["plugin_defaults_refusal"];
    assert_eq!(k["status"], 422);
    assert!(k["detail_prefix"]
        .as_str()
        .unwrap()
        .contains("Unrecognized field \"pluginDefaults\""));
}

#[test]
fn a_plugin_task_under_errors_is_parsed_too() {
    let src = format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: l\n        type: io.kestra.plugin.core.log.Log\n        message: x\n    errors:\n      - id: e\n        type: kestra.{PG_QUERY}\n        url: jdbc:postgresql://h/db\n        sql: select 1\n        sqll: x\n"
    );
    let flow = spec::parse(&src).unwrap();
    let found: Vec<String> = plugins::plugin_tasks(&flow)
        .into_iter()
        .map(|(p, ..)| p)
        .collect();
    assert_eq!(found, vec!["tasks.s.e"]);
    let errs = spec::validate(&flow).errors;
    let f = errs
        .iter()
        .find(|f| f.code == plugins::CODE_PROPERTY_UNKNOWN)
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert_eq!(f.path, "tasks.s.e.sqll");
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

fn kestra_entry(types: Option<&[&str]>) -> PluginConfiguration {
    PluginConfiguration {
        r#type: plugins::CONFIG_TYPE.to_string(),
        values: PluginConfigurationValues {
            allowed_images: None,
            allowed_types: types.map(|t| t.iter().map(|s| s.to_string()).collect()),
        },
    }
}

fn runner_entry(types: Option<&[&str]>) -> PluginConfiguration {
    PluginConfiguration {
        r#type: scripts::DOCKER_RUNNER.to_string(),
        values: PluginConfigurationValues {
            allowed_images: Some(vec!["*".into()]),
            allowed_types: types.map(|t| t.iter().map(|s| s.to_string()).collect()),
        },
    }
}

fn open_input<'a>(pc: &'a PluginConfigurations, fqcn: &'a str) -> gate::GateInput<'a> {
    gate::GateInput {
        feature_enabled: true,
        containers_enabled: true,
        plugin_configurations: pc,
        runner: Some(RunnerKind::Docker),
        fqcn,
    }
}

#[test]
fn no_kestra_entry_allows_the_catalog_and_an_empty_list_allows_none() {
    // No `kestra` entry (a runner entry does not count): every catalog type.
    let none = PluginConfigurations(vec![runner_entry(None)]);
    assert_eq!(none.allowed_types(), None);
    for fqcn in plugins::catalog().classes.keys() {
        assert_eq!(gate::check(&open_input(&none, fqcn)), Ok(()), "{fqcn}");
    }
    // An empty list: none.
    let empty = PluginConfigurations(vec![kestra_entry(Some(&[]))]);
    let refused = gate::check(&open_input(&empty, PG_QUERY)).unwrap_err();
    assert_eq!(
        refused,
        gate::GateRefusal::TypeNotAllowed {
            fqcn: PG_QUERY.to_string()
        }
    );
    assert!(refused.to_string().contains("allowed-types"), "{refused}");
    // An entry with no `allowed-types` key names the type: an empty list.
    let keyless = PluginConfigurations(vec![kestra_entry(None)]);
    assert_eq!(keyless.allowed_types(), Some(vec![]));
    assert!(gate::check(&open_input(&keyless, PG_QUERY)).is_err());
}

#[test]
fn allowed_types_glob_spans_package_segments() {
    let jdbc = PluginConfigurations(vec![kestra_entry(Some(&["io.kestra.plugin.jdbc.*"]))]);
    for fqcn in plugins::catalog().classes.keys() {
        assert_eq!(gate::check(&open_input(&jdbc, fqcn)), Ok(()), "{fqcn}");
    }
    let pg_only = PluginConfigurations(vec![kestra_entry(Some(&[
        "io.kestra.plugin.jdbc.postgresql.*",
    ]))]);
    assert_eq!(gate::check(&open_input(&pg_only, PG_QUERY)), Ok(()));
    assert!(matches!(
        gate::check(&open_input(&pg_only, MYSQL_QUERY)),
        Err(gate::GateRefusal::TypeNotAllowed { .. })
    ));
}

#[test]
fn the_feature_requires_workflow_containers() {
    let pc = PluginConfigurations::default();
    let mut input = open_input(&pc, PG_QUERY);
    assert_eq!(gate::check(&input), Ok(()));

    input.containers_enabled = false;
    let r = gate::check(&input).unwrap_err();
    assert_eq!(r, gate::GateRefusal::ContainersDisabled);
    assert!(r.to_string().contains("'workflow_containers'"), "{r}");

    input.feature_enabled = false;
    let r = gate::check(&input).unwrap_err();
    assert_eq!(r, gate::GateRefusal::FeatureDisabled);
    assert!(r.to_string().contains("'workflow_kestra_plugins'"), "{r}");

    let mut input = open_input(&pc, PG_QUERY);
    input.runner = None;
    let r = gate::check(&input).unwrap_err();
    assert_eq!(r, gate::GateRefusal::NoRunner);
    assert!(r.to_string().contains("workflow.container_runner"), "{r}");

    // And the workspace catalog cannot hold the feature on with containers off.
    use egret_workspace::features::{default_enabled, violates_dependencies};
    assert_eq!(default_enabled(plugins::FEATURE_KEY), Some(false));
    let disabled: std::collections::HashSet<String> =
        [scripts::FEATURE_KEY.to_string()].into_iter().collect();
    assert_eq!(
        violates_dependencies(&disabled),
        Some("workflow_kestra_plugins requires workflow_containers to be enabled")
    );
}

#[tokio::test]
async fn allowed_types_on_a_runner_entry_is_refused_at_boot() {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.plugin_configurations = PluginConfigurations(vec![runner_entry(Some(&["*"]))]);
    let e = cfg.validate_plugin_configurations().unwrap_err();
    assert!(
        e.contains("'allowed-types'") && e.contains(scripts::DOCKER_RUNNER),
        "{e}"
    );

    // `allowed-images` on the `kestra` entry, even an empty list.
    let mut entry = kestra_entry(Some(&["*"]));
    entry.values.allowed_images = Some(vec![]);
    cfg.plugin_configurations = PluginConfigurations(vec![entry]);
    let e = cfg.validate_plugin_configurations().unwrap_err();
    assert!(
        e.contains("'allowed-images'") && e.contains("plugin_host.image"),
        "{e}"
    );

    // The boot path runs it first, before `container_runner = none` returns.
    cfg.plugin_configurations = PluginConfigurations(vec![runner_entry(Some(&["*"]))]);
    let pool = sqlx::PgPool::connect_lazy("postgres://nobody@localhost:1/none").unwrap();
    let Err(e) = egret_workflow::runtime::handlers_for(&cfg, pool.clone(), no_secrets()).await
    else {
        panic!("handlers_for must refuse the configuration");
    };
    assert!(e.to_string().contains("'allowed-types'"), "{e}");

    // The control: both in their right place pass, at both sites.
    cfg.plugin_configurations =
        PluginConfigurations(vec![runner_entry(None), kestra_entry(Some(&["*"]))]);
    cfg.validate_plugin_configurations().unwrap();
    assert!(
        egret_workflow::runtime::handlers_for(&cfg, pool, no_secrets())
            .await
            .is_ok()
    );
}

// ---------------------------------------------------------------------------
// Through the publish route
// ---------------------------------------------------------------------------

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

fn plugin_flow(ns: &str, fqcn: &str) -> String {
    format!(
        "id: f\nnamespace: {ns}\ntasks:\n  - id: q\n    type: kestra.{fqcn}\n    url: jdbc:postgresql://h/db\n    sql: select 1\n"
    )
}

#[tokio::test]
async fn publish_refuses_a_plugin_task_with_the_feature_off() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app_with_scripts(
        &db,
        scripts::ScriptSettings {
            plugin_configurations: PluginConfigurations(vec![kestra_entry(Some(&[
                "io.kestra.plugin.jdbc.postgresql.*",
            ]))]),
            default_runner: Some(RunnerKind::Docker),
        },
    )
    .await;

    // 1. Neither feature row: the plugin feature is named first.
    let (s, body) = publish(&app, ws, user, &plugin_flow("w5b.off", PG_QUERY)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("'workflow_kestra_plugins' feature is disabled"),
        "{body}"
    );

    // 2. Containers on, plugins still off.
    common::enable_feature(&db, ws, scripts::FEATURE_KEY).await;
    let (s, body) = publish(&app, ws, user, &plugin_flow("w5b.off2", PG_QUERY)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("'workflow_kestra_plugins' feature is disabled"),
        "{body}"
    );

    common::enable_feature(&db, ws, plugins::FEATURE_KEY).await;

    // 3. Outside `allowed-types`.
    let (s, body) = publish(&app, ws, user, &plugin_flow("w5b.type", MYSQL_QUERY)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains(MYSQL_QUERY) && body.contains("allowed-types"),
        "{body}"
    );

    // 4. `pluginDefaults:` blocks publish.
    let (s, body) = publish(
        &app,
        ws,
        user,
        &format!(
            "{}pluginDefaults: []\n",
            plugin_flow("w5b.defaults", PG_QUERY)
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("`pluginDefaults` was removed in Kestra 2.0"),
        "{body}"
    );

    // 5. A bare FQCN, with the hint.
    let (s, body) = publish(
        &app,
        ws,
        user,
        &plugin_flow("w5b.bare", PG_QUERY).replace("type: kestra.", "type: "),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains(&prefixed(PG_QUERY)), "{body}");

    // The control: the same task, allowed, publishes -- through step 4's
    // catalog arm, which accepts a type `TASK_TYPES` does not hold.
    let (s, body) = publish(&app, ws, user, &plugin_flow("w5b.ok", PG_QUERY)).await;
    assert_eq!(s, StatusCode::OK, "{body}");

    // And with no runner configured, it is refused by name.
    let app = common::app_with_scripts(
        &db,
        scripts::ScriptSettings {
            plugin_configurations: PluginConfigurations::default(),
            default_runner: None,
        },
    )
    .await;
    let (s, body) = publish(&app, ws, user, &plugin_flow("w5b.norunner", PG_QUERY)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("no container runner is configured"), "{body}");
}

// ---------------------------------------------------------------------------
// `pluginDefaults:` at run time (fix round 1, user ruling 2A): refused at
// save and publish only; a version that already carries it -- published
// before W5b, when egret dropped the key silently -- still plans and runs,
// and the key applies nothing.
// ---------------------------------------------------------------------------

/// A `Return` with no `format:` outputs `{}`; the defaults would give it a
/// `value` if anything applied them.
const RETURN_ONLY: &str =
    "id: f\nnamespace: a.b\ntasks:\n  - id: r\n    type: io.kestra.plugin.core.debug.Return\n";
const DEFAULTS: &str = "pluginDefaults:\n  - type: io.kestra.plugin.core.debug.Return\n    values:\n      format: FROM_DEFAULTS\n";

#[test]
fn a_version_carrying_plugin_defaults_still_builds_the_same_plan() {
    let with = format!("{RETURN_ONLY}{DEFAULTS}");
    // The draft/publish half still refuses it (the control that the source
    // really carries the key).
    assert!(errors_of(&with)
        .iter()
        .any(|f| f.code == plugins::CODE_REMOVED_PLUGIN_DEFAULTS));
    // The run-time half does not, and builds exactly the plan the source
    // without the key builds.
    let planned = egret_workflow::executor::build_plan(&with)
        .unwrap_or_else(|e| panic!("build_plan refused a published pluginDefaults: {e}"));
    assert_eq!(
        planned,
        egret_workflow::executor::build_plan(RETURN_ONLY).unwrap()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_version_carrying_plugin_defaults_runs_and_the_defaults_apply_nothing(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(
        &pool,
        ws,
        None,
        &format!("{RETURN_ONLY}{DEFAULTS}"),
        json!({}),
    )
    .await
    .expect("an execution of a source carrying pluginDefaults starts");
    let handlers = egret_workflow::worker::Handlers::with_core_tasks();
    for _ in 0..16 {
        if egret_workflow::worker::tick(&pool, 8, &handlers)
            .await
            .expect("tick")
            == 0
        {
            break;
        }
    }
    let (state, outputs): (String, Option<Value>) = sqlx::query_as(
        "SELECT state, outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'r'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("the task ran");
    assert_eq!(state, "SUCCESS");
    assert_eq!(
        outputs,
        Some(json!({})),
        "pluginDefaults must apply nothing"
    );
    let history: Value =
        sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(history, json!(["CREATED", "RUNNING", "SUCCESS"]));
}

/// W5b: a `SecretSource` for registries these tests build but never run a
/// plugin task through.
struct NoSecrets;

#[async_trait::async_trait]
impl egret_workflow::runners::secrets::SecretSource for NoSecrets {
    async fn get(
        &self,
        _: uuid::Uuid,
        name: &str,
    ) -> Result<
        std::collections::BTreeMap<String, String>,
        egret_workflow::runners::secrets::SecretError,
    > {
        Err(egret_workflow::runners::secrets::SecretError::NotFound { name: name.into() })
    }
}

fn no_secrets() -> std::sync::Arc<dyn egret_workflow::runners::secrets::SecretSource> {
    std::sync::Arc::new(NoSecrets)
}

// ---------------------------------------------------------------------------
// Fix round 1, I3: `workflow:use_secrets`. A flow whose content can call
// `secret()` is written -- saved (create, update) or published -- only by a
// principal holding it. Saving is gated too because every execution path
// runs the DRAFT (`workflow.flows.source`), not a published version.
// ---------------------------------------------------------------------------

const USE_SECRETS: &str = "workflow:use_secrets";

fn secret_flow(ns: &str) -> String {
    format!(
        "id: f\nnamespace: {ns}\ntasks:\n  - id: q\n    type: kestra.{PG_QUERY}\n    \
         url: jdbc:postgresql://h/db\n    username: u\n    \
         password: \"{{{{ secret('db', subkey='password') }}}}\"\n    sql: select 1\n"
    )
}

async fn another_user(db: &egret_metastore::Db) -> egret_core::UserId {
    let uid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", uuid::Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");
    egret_core::UserId(uid)
}

fn assert_names_the_permission(status: StatusCode, body: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains(USE_SECRETS), "{body}");
    assert!(body.contains("secret()"), "{body}");
}

#[tokio::test]
async fn saving_or_publishing_a_flow_that_calls_secret_needs_workflow_use_secrets() {
    let Some(db) = common::db().await else { return };
    let (ws, admin) = common::seed_workspace(&db).await;
    let editor = another_user(&db).await;
    common::grant(&db, ws, editor, &["workflow:read", "workflow:manage"]).await;
    common::grant(
        &db,
        ws,
        admin,
        &["workflow:read", "workflow:manage", USE_SECRETS],
    )
    .await;
    common::enable_feature(&db, ws, scripts::FEATURE_KEY).await;
    common::enable_feature(&db, ws, plugins::FEATURE_KEY).await;
    let app = common::app_with_scripts(
        &db,
        scripts::ScriptSettings {
            plugin_configurations: PluginConfigurations(vec![kestra_entry(Some(&[
                "io.kestra.plugin.jdbc.postgresql.*",
            ]))]),
            default_runner: Some(RunnerKind::Docker),
        },
    )
    .await;
    let flows = format!("/api/ws/{ws}/workflows/flows");

    // 1. The editor cannot CREATE one, and nothing is stored.
    let res = common::post(
        &app,
        &flows,
        editor,
        json!({ "source": secret_flow("i3.a") }),
    )
    .await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.flows WHERE workspace_id = $1 AND namespace = 'i3.a'",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stored, 0, "a refused create stores nothing");

    // 2. Nor UPDATE a flow of its own to call one.
    let plain = common::json(
        common::post(
            &app,
            &flows,
            editor,
            json!({ "source": plugin_flow("i3.b", PG_QUERY) }),
        )
        .await,
    )
    .await;
    let plain_id = plain["id"].as_str().unwrap().to_string();
    let res = common::put(
        &app,
        &format!("{flows}/{plain_id}"),
        editor,
        json!({ "source": secret_flow("i3.b") }),
    )
    .await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);

    // 3. The holder can create, update and publish one.
    let created = common::json(
        common::post(
            &app,
            &flows,
            admin,
            json!({ "source": secret_flow("i3.c") }),
        )
        .await,
    )
    .await;
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    let res = common::put(
        &app,
        &format!("{flows}/{id}"),
        admin,
        json!({ "source": secret_flow("i3.c").replace("select 1", "select 2") }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        common::body_text(res).await
    );
    let res = common::post(&app, &format!("{flows}/{id}/publish"), admin, json!({})).await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        common::body_text(res).await
    );

    // 4. The editor cannot edit the holder's flow while it still calls
    //    secret() -- the edit could redirect the value -- nor publish it.
    let res = common::put(
        &app,
        &format!("{flows}/{id}"),
        editor,
        json!({ "source": secret_flow("i3.c").replace("select 1", "select 3") }),
    )
    .await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);
    let res = common::post(&app, &format!("{flows}/{id}/publish"), editor, json!({})).await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);

    // 5. A draft stored without the gate (before this migration, or by a
    //    writer that bypassed the route) cannot be published by the editor.
    let seeded = common::seed_flow(&db, ws, editor, &secret_flow("i3.d"), "draft").await;
    let res = common::post(
        &app,
        &format!("{flows}/{seeded}/publish"),
        editor,
        json!({}),
    )
    .await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);

    // 6. Removing the call needs nothing more than workflow:manage, and
    //    validating (which writes nothing) is not gated.
    let res = common::put(
        &app,
        &format!("{flows}/{id}"),
        editor,
        json!({ "source": plugin_flow("i3.c", PG_QUERY) }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        common::body_text(res).await
    );
    let res = common::post(
        &app,
        &format!("{flows}/{id}/validate"),
        editor,
        json!({ "source": secret_flow("i3.c") }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        common::body_text(res).await
    );
}

/// The migration seeds `workflow:use_secrets` to `ws_admin` and to no other
/// workspace role -- `ws_editor` holds `workflow:manage` but not this.
#[tokio::test]
async fn use_secrets_is_seeded_to_ws_admin_only() {
    let Some(db) = common::db().await else { return };
    let holders: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE rp.permission = $1 AND r.name IN ('ws_admin','ws_editor','ws_viewer') \
         ORDER BY r.name",
    )
    .bind(USE_SECRETS)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(holders, vec!["ws_admin".to_string()]);
    assert!(egret_core::rbac::is_known(USE_SECRETS));
}

// ---------------------------------------------------------------------------
// Fix round 1, I2: the publish-time refusals of spec §6 -- `secret()`
// anywhere but a `kestra.*` task's properties, and a `namespace=`/`full=`
// written in a call on one. One function, `spec::validate`, so save (the
// draft's report) and publish (which refuses on any error) agree.
// ---------------------------------------------------------------------------

const SECRET: &str = "\"{{ secret('db', subkey='password') }}\"";

fn codes_at(src: &str, code: &str) -> Vec<String> {
    errors_of(src)
        .into_iter()
        .filter(|f| f.code == code)
        .map(|f| f.path)
        .collect()
}

#[test]
fn secret_outside_a_plugin_tasks_properties_is_refused_by_name() {
    let pg = format!("type: kestra.{PG_QUERY}\n    url: jdbc:postgresql://h/db\n    sql: select 1");
    let cases = [
        (
            "a script task's commands",
            format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.scripts.shell.Commands\n    \
                 containerImage: alpine\n    commands:\n      - echo {SECRET}\n"
            ),
            "tasks.s.commands[0]",
        ),
        (
            "a plugin task's runIf",
            format!("id: f\nnamespace: a.b\ntasks:\n  - id: q\n    {pg}\n    runIf: {SECRET}\n"),
            "tasks.q.runIf",
        ),
        (
            "an input's defaults",
            format!(
                "id: f\nnamespace: a.b\ninputs:\n  - id: i\n    type: STRING\n    defaults: {SECRET}\n\
                 tasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
            ),
            "inputs.i.defaults",
        ),
        (
            "a Log under a flowable's errors",
            format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    \
                 tasks:\n      - id: l\n        type: io.kestra.plugin.core.log.Log\n        message: x\n    \
                 errors:\n      - id: e\n        type: io.kestra.plugin.core.log.Log\n        message: {SECRET}\n"
            ),
            "tasks.s.errors.e.message",
        ),
        (
            "a trigger",
            format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
                 triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: {SECRET}\n"
            ),
            "triggers.w.key",
        ),
    ];
    for (name, src, path) in cases {
        let errs = errors_of(&src);
        let f = errs
            .iter()
            .find(|f| f.code == plugins::CODE_SECRET_OUTSIDE_PLUGIN_TASK)
            .unwrap_or_else(|| panic!("{name}: {errs:?}"));
        assert_eq!(f.path, path, "{name}");
        assert!(f.message.contains("secret()"), "{name}: {}", f.message);
        assert!(
            f.message
                .contains("resolves only on `kestra.*` plugin tasks"),
            "{name}: {}",
            f.message
        );
        // Save-only: a stored draft that carries one still plans, and the
        // render refuses the call at run as before.
        let flow = spec::parse(&src).unwrap();
        assert!(
            !spec::validate_for_run(&flow)
                .errors
                .iter()
                .any(|f| f.code == plugins::CODE_SECRET_OUTSIDE_PLUGIN_TASK),
            "{name}"
        );
    }

    // The control: the same call in a plugin task's property is allowed,
    // including one nested under `errors:`.
    let ok = format!(
        "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    \
         tasks:\n      - id: q\n        {}\n        password: {SECRET}\n    \
         errors:\n      - id: e\n        {}\n        password: {SECRET}\n",
        pg.replace("\n    ", "\n        "),
        pg.replace("\n    ", "\n        ")
    );
    assert!(errors_of(&ok).is_empty(), "{:?}", errors_of(&ok));
}

#[test]
fn namespace_and_full_written_in_a_call_are_refused_at_publish() {
    let with = |call: &str| {
        format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: q\n    type: kestra.{PG_QUERY}\n    \
             url: jdbc:postgresql://h/db\n    sql: select 1\n    password: \"{call}\"\n"
        )
    };
    for (call, arg) in [
        ("{{ secret('db', namespace='other') }}", "namespace"),
        ("{{ secret(key='db', full=true) }}", "full"),
        ("{{ secret('db', 'other.ns') }}", "namespace"),
    ] {
        let errs = errors_of(&with(call));
        let f = errs
            .iter()
            .find(|f| f.code == plugins::CODE_SECRET_ARGUMENT_REFUSED)
            .unwrap_or_else(|| panic!("{call}: {errs:?}"));
        assert_eq!(f.path, "tasks.q.password", "{call}");
        assert!(
            f.message.contains(&format!("'{arg}'")),
            "{call}: {}",
            f.message
        );
    }
    // `subkey=` is Kestra's, and allowed.
    assert!(codes_at(
        &with("{{ secret('db', subkey='password') }}"),
        plugins::CODE_SECRET_ARGUMENT_REFUSED
    )
    .is_empty());
}

/// Fix round 2, C1: the three spellings the round-1 detector lexed
/// differently from the renderer. Each is a real `secret()` call (see
/// `runners::secrets::tests::the_round_one_misses_really_call_secret`), so an
/// editor without `workflow:use_secrets` cannot save one.
#[tokio::test]
async fn the_round_one_detector_misses_are_refused_to_an_editor() {
    let Some(db) = common::db().await else { return };
    let (ws, editor) = common::seed_workspace(&db).await;
    common::grant(&db, ws, editor, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    for (i, call) in [
        r#"{{ {'a': {}} ~ secret('db') }}"#,
        r#"{% raw %}{{ \"{% endraw %}{{ secret('db') }}"#,
        r#"{% raw %}{{ '{% endraw %}{{ secret('db') }}"#,
    ]
    .into_iter()
    .enumerate()
    {
        let src = format!(
            "id: f\nnamespace: c1.n{i}\ntasks:\n  - id: q\n    type: kestra.{PG_QUERY}\n    \
             url: jdbc:postgresql://h/db\n    password: \"{call}\"\n    sql: select 1\n"
        );
        let res = common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            editor,
            json!({ "source": src }),
        )
        .await;
        let (s, body) = (res.status(), common::body_text(res).await);
        assert_names_the_permission(s, &body);
    }
}

/// Fix round 2, m1: the BUILT-IN memberships, not a test role. `ws_admin`
/// saves and publishes a flow that calls `secret()`; `ws_editor` is refused
/// by name; the platform admin (the unrestricted Cedar template) can.
#[tokio::test]
async fn built_in_roles_ws_admin_and_platform_admin_may_use_secrets_and_ws_editor_may_not() {
    let Some(db) = common::db().await else { return };
    let (ws, admin) = common::seed_workspace(&db).await;
    let editor = another_user(&db).await;
    let platform = another_user(&db).await;
    for (user, role) in [(admin, "ws_admin"), (editor, "ws_editor")] {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'workspace', $2 FROM roles \
             WHERE scope = 'workspace' AND name = $3",
        )
        .bind(user.0)
        .bind(ws)
        .bind(role)
        .execute(&db.pool)
        .await
        .expect("grant built-in role");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
         WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(platform.0)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    common::enable_feature(&db, ws, scripts::FEATURE_KEY).await;
    common::enable_feature(&db, ws, plugins::FEATURE_KEY).await;
    let app = common::app_with_scripts(
        &db,
        scripts::ScriptSettings {
            plugin_configurations: PluginConfigurations(vec![kestra_entry(Some(&[
                "io.kestra.plugin.jdbc.postgresql.*",
            ]))]),
            default_runner: Some(RunnerKind::Docker),
        },
    )
    .await;
    let flows = format!("/api/ws/{ws}/workflows/flows");

    let res = common::post(
        &app,
        &flows,
        editor,
        json!({ "source": secret_flow("m1.e") }),
    )
    .await;
    let (s, body) = (res.status(), common::body_text(res).await);
    assert_names_the_permission(s, &body);

    for (user, ns) in [(admin, "m1.a"), (platform, "m1.p")] {
        let created = common::json(
            common::post(&app, &flows, user, json!({ "source": secret_flow(ns) })).await,
        )
        .await;
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{ns}: {created}"))
            .to_string();
        let res = common::post(&app, &format!("{flows}/{id}/publish"), user, json!({})).await;
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "{ns}: {}",
            common::body_text(res).await
        );
    }
}
