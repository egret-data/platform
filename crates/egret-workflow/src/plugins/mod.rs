//! W5b: Kestra plugin tasks (`type: kestra.<FQCN>`), run by the plugin host
//! image built from `plugin-host/` (spec
//! `docs/en/specs/2026-09-29-workflow-w5b-kestra-plugin-host-design.md` §3).
//!
//! [`CATALOG_JSON`] is `EgretPluginHost describe`'s output over the jars
//! `plugin-host/plugins.lock` names, committed and embedded so publish needs
//! no JVM. Regenerate it whenever `plugins.lock` changes:
//!
//! ```bash
//! cd plugin-host
//! ./fetch-plugins.sh target/plugins
//! mvn -q package -DskipTests
//! java -cp 'target/egret-plugin-host.jar:target/lib/*:target/plugins/*' \
//!   io.kestra.core.runners.EgretPluginHost describe target/plugins \
//!   > ../crates/egret-workflow/src/plugins/catalog.json
//! ```
//!
//! The host's own tests need the same jars: on a fresh checkout, run
//! `plugin-host/fetch-plugins.sh plugin-host/target/plugins` before
//! `mvn -f plugin-host/pom.xml verify` (`just test-plugin-host` does both).

//!
//! # The task model (W5b spec §4)
//!
//! A task whose `type:` starts with [`PREFIX`] is a plugin task; the rest
//! must be a FQCN in the catalog. [`parse`] is pure: it reads a leaf task's
//! opaque `spec` map (what is left after `models::Task`'s deserializer took
//! [`HANDLED_BY_TASK_MODEL`]) and answers a [`PluginTask`] or every
//! [`PluginRefusal`] it found. `spec::validate` reports those as errors, so
//! they block publish and show on a draft; [`gate`] is the governance check
//! publish (and, from Task 6, the handler at run) calls after it.
//!
//! Refused by name, each with its own code:
//! * a `kestra.` FQCN not in the catalog ([`CODE_TYPE_UNKNOWN`]); a bare
//!   catalog FQCN, with the prefixed spelling as the hint
//!   ([`CODE_TYPE_UNPREFIXED`], reported by `spec::validate`);
//! * a property the catalog does not list for the class
//!   ([`CODE_PROPERTY_UNKNOWN`]); a missing required one
//!   ([`CODE_PROPERTY_REQUIRED`]);
//! * `taskRunner` and the seven base-task properties egret's `Task` model
//!   does not read ([`REFUSED_BASE_PROPERTIES`], as on script tasks;
//!   [`CODE_PROPERTY_NOT_IMPLEMENTED`]) -- the catalog lists the seven, so
//!   they must be refused before the catalog is consulted;
//! * a literal of the wrong type where the schema is plain
//!   ([`CODE_PROPERTY_INVALID`]). A schema egret cannot read (`anyOf`,
//!   `$ref`, `const`, ...) and a `{{ }}` value are accepted and checked by the
//!   host after rendering (exit 2).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::models::{Flow, Task, TaskKind};

pub mod gate;

/// The `type:` prefix that marks a Kestra plugin task (user decision: the
/// only spelling; "runs in the JVM host").
pub const PREFIX: &str = "kestra.";

/// The feature key that gates plugin tasks (`egret_workspace::features`).
/// Off by default, and it requires `workflow_containers`.
pub const FEATURE_KEY: &str = "workflow_kestra_plugins";

/// The `plugin_configurations` entry type whose `allowed-types` governs
/// plugin tasks.
pub const CONFIG_TYPE: &str = egret_config::KESTRA_CONFIG_TYPE;

/// `{"lockSha256", "kestra", "classes": {FQCN: {"properties": {name:
/// {"schema", "required"}}, "outputs": [..]}}}`.
pub(crate) const CATALOG_JSON: &str = include_str!("catalog.json");

/// The embedded catalog, parsed.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    #[serde(rename = "lockSha256")]
    pub lock_sha256: String,
    pub classes: BTreeMap<String, PluginClass>,
}

/// One task class of the catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginClass {
    pub properties: BTreeMap<String, PropertySpec>,
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// One property of a [`PluginClass`]: Kestra's JSON schema for it, and
/// whether it is required.
#[derive(Debug, Clone, Deserialize)]
pub struct PropertySpec {
    pub schema: Value,
    #[serde(default)]
    pub required: bool,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(CATALOG_JSON)
        .unwrap_or_else(|e| panic!("plugins/catalog.json does not parse as a Catalog: {e}"))
});

/// The embedded catalog. Panics on a malformed `catalog.json`, which the
/// unit tests below would catch first.
pub fn catalog() -> &'static Catalog {
    &CATALOG
}

static PREFIXED_TYPES: LazyLock<Vec<String>> = LazyLock::new(|| {
    catalog()
        .classes
        .keys()
        .map(|fqcn| format!("{PREFIX}{fqcn}"))
        .collect()
});

/// Every catalog class as a flow writes it, `kestra.<FQCN>`, sorted: the
/// keys the plugin handler is registered under (`handlers::kestra::register`).
/// `'static`, because a `TaskHandler::type_id` is.
pub fn prefixed_types() -> impl Iterator<Item = &'static str> {
    PREFIXED_TYPES.iter().map(String::as_str)
}

/// The catalog class a `kestra.<FQCN>` type names: `(FQCN, class)`. `None`
/// for a type without the prefix (a bare FQCN is NOT a plugin task) or a
/// FQCN the catalog does not hold.
pub fn lookup(type_id: &str) -> Option<(&'static str, &'static PluginClass)> {
    let fqcn = type_id.strip_prefix(PREFIX)?;
    catalog()
        .classes
        .get_key_value(fqcn)
        .map(|(k, v)| (k.as_str(), v))
}

/// The prefixed spelling, `kestra.<type_id>`, when `type_id` is a bare
/// catalog FQCN; `None` otherwise.
pub fn bare_hint(type_id: &str) -> Option<String> {
    catalog()
        .classes
        .contains_key(type_id)
        .then(|| format!("{PREFIX}{type_id}"))
}

/// A parsed plugin task: its class and its properties as written (the
/// leaf spec, minus nulls). Rendering and the rest happen at run (Task 6).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginTask {
    pub fqcn: &'static str,
    pub properties: Map<String, Value>,
}

/// Why a plugin task cannot be saved. `path` is relative to the task
/// (`sqll`, `fetchType`); empty for the task itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRefusal {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

/// A `kestra.` type the catalog does not hold.
pub const CODE_TYPE_UNKNOWN: &str = "plugin_type_unknown";
/// A catalog FQCN written without [`PREFIX`].
pub const CODE_TYPE_UNPREFIXED: &str = "plugin_type_unprefixed";
/// The catalog does not list the property for the class.
pub const CODE_PROPERTY_UNKNOWN: &str = "plugin_property_unknown";
pub const CODE_PROPERTY_REQUIRED: &str = "plugin_property_required";
/// Kestra accepts the property; egret does not implement it.
pub const CODE_PROPERTY_NOT_IMPLEMENTED: &str = "plugin_property_not_implemented";
/// A literal that is not of the schema's plain type (or not in its enum).
pub const CODE_PROPERTY_INVALID: &str = "plugin_property_invalid";
/// `secret()` written anywhere but a `kestra.*` task's properties -- a
/// script task, `runIf`, an input, a `Log`, a trigger (fix round 1, I2; spec
/// §6). Refused at save and publish; the run-time refusal stays.
pub const CODE_SECRET_OUTSIDE_PLUGIN_TASK: &str = "secret_outside_plugin_task";
/// A `secret()` call on a `kestra.*` task that passes `namespace` or `full`
/// in the source (fix round 1, I2; spec §6).
pub const CODE_SECRET_ARGUMENT_REFUSED: &str = "secret_argument_refused";
/// W9c Task 4 (W9c spec §7): `secret()` written anywhere in the `value` of
/// an `io.kestra.plugin.core.kv.Set` or `Put`. Reported INSTEAD of
/// [`CODE_SECRET_OUTSIDE_PLUGIN_TASK`] for that property, so the author reads
/// the reason that applies; it holds even if `secret()` is later allowed
/// outside `kestra.*` tasks.
pub const CODE_SECRET_IN_KV_VALUE: &str = "secret_in_kv_value";

/// [`CODE_SECRET_IN_KV_VALUE`]'s message (W9c spec §7's sentence, verbatim),
/// naming the task id and the property path (`tasks.<id>.value…`).
pub fn secret_in_kv_value_message(task_id: &str, path: &str) -> String {
    format!(
        "secret() is refused in the value of task '{task_id}' ({path}): the secret's value \
         would be stored in the KV store, where anyone who can read this namespace's KV \
         (`workflow:read`) could read it, unmasked; pass the secret to the task that needs it \
         instead."
    )
}

/// A top-level `pluginDefaults:` (spec §4.1), on any flow.
pub const CODE_REMOVED_PLUGIN_DEFAULTS: &str = "removed_plugin_defaults";
/// The one message for [`CODE_REMOVED_PLUGIN_DEFAULTS`], verbatim from spec
/// §4.1.
pub const REMOVED_PLUGIN_DEFAULTS_MESSAGE: &str =
    "`pluginDefaults` was removed in Kestra 2.0; set the properties on each task";

/// Read by egret's `Task` model before the leaf `spec` exists
/// (`models::task`'s deserializer); never demanded, never refused here.
pub const HANDLED_BY_TASK_MODEL: &[&str] = &[
    "allowFailure",
    "allowWarning",
    "description",
    "disabled",
    "id",
    "retry",
    "runIf",
    "timeout",
    "type",
];

/// The seven base-`Task` properties egret's `Task` model does not read,
/// refused on plugin tasks as on script tasks (`scripts::COMMON_REFUSED`).
/// The catalog lists every one of them for every class.
pub const REFUSED_BASE_PROPERTIES: &[&str] = &[
    "assets",
    "logLevel",
    "logToFile",
    "policyRefs",
    "taskCache",
    "version",
    "workerSelector",
];

// W9a Task 9: no property value is refused for needing internal storage.
// W5b's `REFUSED_VALUES` (Task 1's `w5b-plugins.json` `refused_values`) is
// gone with `NeedsStorage`, `Literal`, `needs_storage`, `refused_values_json`
// and `CODE_PROPERTY_NEEDS_STORAGE`: W9a Task 1 re-measured every row on the
// artifact and `w9a-storage.json`'s `plugin_storage.still_refused_values` is
// `{}` (`tests/plugins_gate.rs` asserts it stays empty).

fn refusal(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> PluginRefusal {
    PluginRefusal {
        code,
        path: path.into(),
        message: message.into(),
    }
}

/// A `{{ }}` / `{% %}` string: judged by the host after rendering.
fn is_expression(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(crate::scripts::gate::is_unrendered)
}

/// The plain JSON type a schema declares, or `None` for a schema egret does
/// not read (`anyOf`, `oneOf`, `allOf`, `$ref`, `const`, `not`, no `type`,
/// a `type` list).
fn plain_type(schema: &Value) -> Option<&str> {
    let obj = schema.as_object()?;
    if ["anyOf", "oneOf", "allOf", "$ref", "const", "not"]
        .iter()
        .any(|k| obj.contains_key(*k))
    {
        return None;
    }
    let t = obj.get("type")?.as_str()?;
    matches!(
        t,
        "string" | "integer" | "number" | "boolean" | "array" | "object"
    )
    .then_some(t)
}

/// Why a literal `value` does not fit a plain `schema`, or `None`. Scalars
/// coerce the way Jackson's do (a number or boolean into a string, a numeric
/// string into a number, `"true"`/`"false"` into a boolean), so this refuses
/// only what Kestra would refuse too.
fn type_problem(schema: &Value, value: &Value) -> Option<String> {
    let t = plain_type(schema)?;
    let fits = match t {
        "string" => matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)),
        "integer" => match value {
            Value::Number(n) => n.is_i64() || n.is_u64(),
            Value::String(s) => s.trim().parse::<i64>().is_ok(),
            _ => false,
        },
        "number" => match value {
            Value::Number(_) => true,
            Value::String(s) => s.trim().parse::<f64>().is_ok(),
            _ => false,
        },
        "boolean" => match value {
            Value::Bool(_) => true,
            Value::String(s) => s == "true" || s == "false",
            _ => false,
        },
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    };
    if !fits {
        return Some(format!("expected {t}, got {}", json_kind(value)));
    }
    if let (Some(allowed), Value::String(s)) = (schema.get("enum").and_then(Value::as_array), value)
    {
        if !allowed.iter().any(|a| a.as_str() == Some(s.as_str())) {
            let names: Vec<&str> = allowed.iter().filter_map(Value::as_str).collect();
            return Some(format!("'{s}' is not one of {}", names.join(", ")));
        }
    }
    None
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a map",
    }
}

/// Parse a plugin task's leaf `spec` (see the module doc) into a
/// [`PluginTask`], or every reason it cannot be saved.
///
/// `type_id` is the task's `type:` as written. Without [`PREFIX`] it is not
/// a plugin task: one refusal at path `type` ([`CODE_TYPE_UNPREFIXED`] with
/// the hint for a bare catalog FQCN, [`CODE_TYPE_UNKNOWN`] otherwise).
pub fn parse(type_id: &str, spec: &Value) -> Result<PluginTask, Vec<PluginRefusal>> {
    let Some((fqcn, class)) = lookup(type_id) else {
        return Err(vec![type_refusal(type_id)]);
    };
    let empty = Map::new();
    let map = match spec {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => {
            return Err(vec![refusal(
                CODE_PROPERTY_INVALID,
                "",
                "a plugin task's properties must be a map",
            )])
        }
    };

    let mut errs = Vec::new();
    let mut properties = Map::new();
    for (key, value) in map {
        let key = key.as_str();
        // An explicit `null` is Kestra's "absent" for every property.
        if value.is_null() || HANDLED_BY_TASK_MODEL.contains(&key) {
            continue;
        }
        if key == "taskRunner" {
            errs.push(refusal(
                CODE_PROPERTY_NOT_IMPLEMENTED,
                key,
                format!(
                    "property 'taskRunner' is not accepted on {fqcn}: plugin tasks run on the \
                     runner this egret is configured with (workflow.container_runner); remove it"
                ),
            ));
            continue;
        }
        if REFUSED_BASE_PROPERTIES.contains(&key) {
            errs.push(refusal(
                CODE_PROPERTY_NOT_IMPLEMENTED,
                key,
                format!(
                    "property '{key}' is accepted by Kestra on {fqcn} but is not implemented by \
                     egret (W5b); remove it"
                ),
            ));
            continue;
        }
        let Some(prop) = class.properties.get(key) else {
            errs.push(refusal(
                CODE_PROPERTY_UNKNOWN,
                key,
                format!("unknown property '{key}' on {fqcn}: the plugin does not accept it"),
            ));
            continue;
        };
        if !is_expression(value) {
            if let Some(problem) = type_problem(&prop.schema, value) {
                errs.push(refusal(
                    CODE_PROPERTY_INVALID,
                    key,
                    format!("property '{key}' on {fqcn}: {problem}"),
                ));
                continue;
            }
        }
        properties.insert(key.to_string(), value.clone());
    }
    for (name, prop) in &class.properties {
        if prop.required
            && !HANDLED_BY_TASK_MODEL.contains(&name.as_str())
            && map.get(name).is_none_or(Value::is_null)
        {
            errs.push(refusal(
                CODE_PROPERTY_REQUIRED,
                name.clone(),
                format!("property '{name}' is required on {fqcn}"),
            ));
        }
    }
    if errs.is_empty() {
        Ok(PluginTask { fqcn, properties })
    } else {
        Err(errs)
    }
}

/// The refusal for a `type:` that [`lookup`] does not resolve.
fn type_refusal(type_id: &str) -> PluginRefusal {
    if let Some(hint) = bare_hint(type_id) {
        return refusal(
            CODE_TYPE_UNPREFIXED,
            "",
            format!(
                "type '{type_id}' is a Kestra plugin task, which egret runs in its plugin host; \
                 write it as '{hint}'"
            ),
        );
    }
    let Some(fqcn) = type_id.strip_prefix(PREFIX) else {
        return refusal(
            CODE_TYPE_UNKNOWN,
            "type",
            format!("'{type_id}' is not a Kestra plugin task type (they start with '{PREFIX}')"),
        );
    };
    let native = crate::spec::registry::known_type(fqcn).is_some();
    refusal(
        CODE_TYPE_UNKNOWN,
        "type",
        if native {
            format!(
                "'{type_id}': '{fqcn}' is not in the plugin catalog because egret runs it \
                 natively; write '{fqcn}' without the '{PREFIX}' prefix"
            )
        } else {
            format!(
                "'{type_id}': '{fqcn}' is not in the plugin catalog (the classes of the jars \
                 plugin-host/plugins.lock names)"
            )
        },
    )
}

/// Every leaf task in `flow`, with its validation path, in all four root
/// lists and at every depth -- through [`Task::children`] **and**
/// [`Task::error_children`], as `scripts::script_tasks` walks (and
/// `spec::validate`'s own walk, since no-silent-stall Task 1).
/// `spec::unrunnable` walks this too, so its paths are these.
pub(crate) fn leaves(flow: &Flow) -> Vec<(String, &Task, &Value)> {
    fn walk<'a>(task: &'a Task, path: String, out: &mut Vec<(String, &'a Task, &'a Value)>) {
        if let TaskKind::Leaf { spec } = &task.kind {
            out.push((path.clone(), task, spec));
        }
        for child in task.children().into_iter().chain(task.error_children()) {
            walk(child, crate::spec::path::task_path(&path, &child.id), out);
        }
    }
    let mut out = Vec::new();
    for (name, list) in crate::spec::path::root_lists(flow) {
        for task in list {
            walk(task, crate::spec::path::task_path(name, &task.id), &mut out);
        }
    }
    out
}

/// Every plugin task (`type:` starting with [`PREFIX`], in the catalog or
/// not) in `flow`, with its validation path -- see [`leaves`] for the walk.
/// What `spec::validate` and `publish` both use.
pub fn plugin_tasks(flow: &Flow) -> Vec<(String, &Task, &Value)> {
    leaves(flow)
        .into_iter()
        .filter(|(_, t, _)| t.r#type.starts_with(PREFIX))
        .collect()
}

/// Every leaf whose `type:` is a bare catalog FQCN, with its path and the
/// refusal (the prefixed hint). What `spec::validate` reports.
pub(crate) fn bare_plugin_types(flow: &Flow) -> Vec<(String, &Task, PluginRefusal)> {
    leaves(flow)
        .into_iter()
        .filter(|(_, t, _)| bare_hint(&t.r#type).is_some())
        .map(|(p, t, _)| {
            let r = type_refusal(&t.r#type);
            (p, t, r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn the_committed_catalog_was_generated_from_the_committed_lockfile() {
        let lock = include_bytes!("../../../../plugin-host/plugins.lock");
        let want = sha256_hex(lock);
        let got: serde_json::Value = serde_json::from_str(CATALOG_JSON).unwrap();
        assert_eq!(
            got["lockSha256"], want,
            "plugins.lock changed; regenerate catalog.json"
        );
        // And the typed view reads the same file.
        assert_eq!(catalog().lock_sha256, want);
    }

    #[test]
    fn every_catalog_class_lists_type_as_a_const_and_every_refused_base_property() {
        for (fqcn, class) in &catalog().classes {
            assert_eq!(class.properties["type"].schema["const"], *fqcn);
            for p in REFUSED_BASE_PROPERTIES {
                assert!(class.properties.contains_key(*p), "{fqcn} lacks {p}");
            }
        }
    }

    #[test]
    fn a_plain_type_is_read_only_from_a_plain_schema() {
        assert_eq!(
            plain_type(&serde_json::json!({"type": "string"})),
            Some("string")
        );
        for s in [
            serde_json::json!({"anyOf": [{"type": "integer"}]}),
            serde_json::json!({"$ref": "#/$defs/x", "type": "object"}),
            serde_json::json!({"const": "x"}),
            serde_json::json!({"type": ["string", "null"]}),
        ] {
            assert_eq!(plain_type(&s), None, "{s}");
        }
    }
}
