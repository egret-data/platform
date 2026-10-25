//! KI-121: the properties a flow's YAML writes that egret drops.
//!
//! The model is not `deny_unknown_fields` (a Kestra flow pasted in must
//! parse), so serde drops a key it has no field for: a typo (`mesage:`), a
//! key no Kestra class has, or a property Kestra documents that egret does
//! not act on (`property-treatment.json` measures those as `ignored`, among
//! them `Pause.onResume` and a retry's `warningOnRetry`). Each saved without
//! a word. This walk reads the RAW document, where the keys are still there,
//! and reports each such key as a warning: the flow still saves and runs, but
//! the editor says what it will not do.
//!
//! The table is the committed measurement ([`super::schema::property_treatment`]),
//! so what is warned about here is exactly what the served schema leaves out:
//!
//! - a key the object's row does not list at all: `unknown_property`;
//! - a key the row measures `ignored`: `ignored_property`.
//!
//! `honoured` and `warned` keys say nothing here (a `warned` one already has
//! its `not_yet_enforced` warning), nor do `refused` ones, which a validation
//! error or a publish gate already names. An object whose type resolves to no
//! row (an egret-only type, a plugin whose row is named otherwise) is not
//! checked: there is no list to check it against.
//!
//! Objects checked: the flow document, every task at every depth (the paths
//! are [`super::path`]'s, `tasks.a.b`), a task's or the flow's `retry`, and
//! every trigger.

use serde_json::Value;

use super::schema::{property_treatment, Treatment, FLOWABLE_TYPES, FLOW_ROW, TRIGGER_TYPES};
use super::validate::Finding;

/// The keys under which a task holds child tasks: a list of tasks, a
/// `Switch`'s `cases` (a map of lists), a `Dag`'s `tasks` (a list of
/// `{task, dependsOn}` nodes).
const CHILD_LISTS: &[&str] = &[
    "tasks", "then", "else", "errors", "finally", "defaults", "cases",
];

/// The flow's four root lists, as [`super::path::root_lists`] names them.
const ROOT_LISTS: &[&str] = &["tasks", "errors", "finally", "afterExecution"];

/// Warnings for the properties of `source` that egret drops. `source` is
/// assumed to have parsed already; a document that is not a YAML mapping
/// gives no warnings (the parse error is the report).
pub fn property_warnings(source: &str) -> Vec<Finding> {
    let Ok(doc) = serde_norway::from_str::<Value>(source) else {
        return Vec::new();
    };
    let Some(flow) = doc.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    check(&mut out, FLOW_ROW, "the flow", flow, "");
    if let Some(retry) = flow.get("retry") {
        check_retry(&mut out, retry, "retry");
    }
    for list in ROOT_LISTS {
        if let Some(Value::Array(tasks)) = flow.get(*list) {
            for task in tasks {
                walk_task(&mut out, task, list);
            }
        }
    }
    if let Some(Value::Array(triggers)) = flow.get("triggers") {
        for trigger in triggers {
            let Some(t) = trigger.as_object() else {
                continue;
            };
            let Some(id) = t.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(row) = t.get("type").and_then(Value::as_str).and_then(trigger_row) {
                check(
                    &mut out,
                    row,
                    &format!("trigger '{id}'"),
                    t,
                    &format!("triggers.{id}"),
                );
            }
        }
    }
    out
}

fn walk_task(out: &mut Vec<Finding>, task: &Value, prefix: &str) {
    let Some(t) = task.as_object() else { return };
    let Some(id) = t.get("id").and_then(Value::as_str) else {
        return;
    };
    let path = super::path::task_path(prefix, id);
    if let Some(row) = t.get("type").and_then(Value::as_str).and_then(task_row) {
        check(out, row, &format!("task '{id}'"), t, &path);
    }
    if let Some(retry) = t.get("retry") {
        check_retry(out, retry, &format!("{path}.retry"));
    }
    for key in CHILD_LISTS {
        match t.get(*key) {
            Some(Value::Array(children)) => {
                for child in children {
                    // A `Dag` node holds its task under `task`.
                    match child.get("task") {
                        Some(inner) if child.get("type").is_none() => walk_task(out, inner, &path),
                        _ => walk_task(out, child, &path),
                    }
                }
            }
            Some(Value::Object(cases)) if *key == "cases" => {
                for children in cases.values() {
                    for child in children.as_array().into_iter().flatten() {
                        walk_task(out, child, &path);
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_retry(out: &mut Vec<Finding>, retry: &Value, path: &str) {
    let Some(r) = retry.as_object() else { return };
    let row = match r.get("type").and_then(Value::as_str) {
        Some("constant") => "io.kestra.core.models.tasks.retrys.Constant-2",
        Some("exponential") => "io.kestra.core.models.tasks.retrys.Exponential-2",
        Some("random") => "io.kestra.core.models.tasks.retrys.Random-2",
        _ => return,
    };
    check(out, row, "the retry", r, path);
}

/// One object against its row.
fn check(
    out: &mut Vec<Finding>,
    row: &str,
    what: &str,
    object: &serde_json::Map<String, Value>,
    path: &str,
) {
    let Some(props) = property_treatment().get(row) else {
        return;
    };
    for key in object.keys() {
        let at = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match props.get(key) {
            None => out.push(Finding {
                code: "unknown_property",
                message: format!(
                    "{what}: `{key}` is not a property of {}; egret ignores it",
                    if row == FLOW_ROW { "a flow" } else { row }
                ),
                path: at,
            }),
            Some(Treatment::Ignored) => out.push(Finding {
                code: "ignored_property",
                message: format!("{what}: `{key}` is accepted, but egret does not act on it"),
                path: at,
            }),
            Some(Treatment::Honoured | Treatment::Warned | Treatment::Refused) => {}
        }
    }
}

/// The row of a task `type`: a flowable's, a registry type's canonical
/// spelling (an alias resolves to it), or the type itself when the table
/// has a row of that name (a script task).
fn task_row(t: &str) -> Option<&'static str> {
    if let Some(row) = row_in(FLOWABLE_TYPES, t) {
        return Some(row);
    }
    if let Some(spec) = super::registry::known_type(t) {
        return Some(spec.r#type);
    }
    property_treatment()
        .get_key_value(t)
        .map(|(k, _)| k.as_str())
}

fn trigger_row(t: &str) -> Option<&'static str> {
    row_in(TRIGGER_TYPES, t)
}

fn row_in(
    table: &'static [(&'static str, &'static [&'static str])],
    t: &str,
) -> Option<&'static str> {
    table
        .iter()
        .find(|(canonical, aliases)| *canonical == t || aliases.contains(&t))
        .map(|(canonical, _)| *canonical)
}

#[cfg(test)]
mod tests {
    use super::property_warnings;

    fn codes(source: &str) -> Vec<(String, &'static str)> {
        property_warnings(source)
            .into_iter()
            .map(|f| (f.path, f.code))
            .collect()
    }

    #[test]
    fn an_unknown_top_level_key_is_a_warning() {
        let w = codes("id: f\nnamespace: a.b\ncolour: blue\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
        assert_eq!(w, vec![("colour".to_string(), "unknown_property")]);
    }

    #[test]
    fn an_unknown_task_property_is_a_warning_at_any_depth() {
        let w = codes(
            "id: f\nnamespace: a.b\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: t\n        type: core.log\n        message: hi\n        mesage: typo\n",
        );
        assert_eq!(
            w,
            vec![("tasks.outer.t.mesage".to_string(), "unknown_property")]
        );
    }

    #[test]
    fn warning_on_retry_and_pause_on_resume_are_ignored_properties() {
        let w = codes(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      maxAttempts: 2\n      warningOnRetry: true\n  - id: gate\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n    onResume:\n      - id: ok\n        type: BOOLEAN\n",
        );
        assert_eq!(
            w,
            vec![
                (
                    "tasks.t.retry.warningOnRetry".to_string(),
                    "ignored_property"
                ),
                ("tasks.gate.onResume".to_string(), "ignored_property"),
            ]
        );
    }

    #[test]
    fn an_unknown_trigger_property_is_a_warning() {
        let w = codes(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\ntriggers:\n  - id: s\n    type: core.schedule\n    cron: \"0 * * * *\"\n    crn: x\n",
        );
        assert_eq!(w, vec![("triggers.s.crn".to_string(), "unknown_property")]);
    }

    #[test]
    fn a_flow_of_known_properties_has_no_warning() {
        let w = codes(
            "id: f\nnamespace: a.b\ndescription: d\nlabels:\n  a: b\ninputs:\n  - id: i\n    type: STRING\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Switch\n    value: x\n    cases:\n      x:\n        - id: t\n          type: core.log\n          message: hi\n    defaults:\n      - id: d\n        type: core.log\n        message: hi\n",
        );
        assert!(w.is_empty(), "{w:?}");
    }
}

#[cfg(test)]
mod corpus {
    /// The oracle's valid Kestra flows (`oracle-exec/corpus/valids`, 323 of
    /// them) write only properties Kestra documents, so none may be called
    /// unknown: the check is the treatment table, and a false `unknown_property`
    /// here would be a row the walk resolves wrongly. Measured 2026-10-25: 0
    /// unknown, 45 `ignored_property` (all keys the table measures `ignored`).
    #[test]
    fn no_valid_kestra_flow_has_an_unknown_property() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/corpus/valids");
        let mut checked = 0;
        let mut unknown = Vec::new();
        for entry in std::fs::read_dir(dir).expect("the corpus") {
            let path = entry.expect("an entry").path();
            let src = std::fs::read_to_string(&path).expect("a flow");
            checked += 1;
            for f in super::property_warnings(&src) {
                if f.code == "unknown_property" {
                    unknown.push(format!("{}: {}", path.display(), f.message));
                }
            }
        }
        assert!(checked > 300, "the corpus is where it was: {checked} files");
        assert!(unknown.is_empty(), "{unknown:#?}");
    }
}
