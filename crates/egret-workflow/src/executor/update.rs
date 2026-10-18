//! `io.kestra.plugin.core.execution.{SetVariables,UnsetVariables,Labels}` —
//! the pure computation, no database.
//!
//! In Kestra these are `ExecutionUpdatableTask`s: the executor applies them to
//! the execution, no worker runs them. [`compute`] turns one task's leaf spec
//! into the execution's whole new `variables` object or label list; the caller
//! stores it.
//!
//! The rules are Kestra's: `SetVariables` merges as `MapUtils.deepMerge`
//! (maps merge, lists append the items not already there, a null new value
//! keeps the old one, anything else is replaced); `Labels` checks system keys
//! before empty values and merges as `Execution` does.

// Used from Task 3/4 of group C2 (the executor plan and the worker's render pass).
#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::labels::execution_labels;
use crate::models::property::{render_map, Property};
use crate::models::Label;
use crate::runners::{render_str, RenderContext};

pub const SET_VARIABLES: &str = "io.kestra.plugin.core.execution.SetVariables";
pub const UNSET_VARIABLES: &str = "io.kestra.plugin.core.execution.UnsetVariables";
pub const LABELS: &str = "io.kestra.plugin.core.execution.Labels";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateOp {
    Set,
    Unset,
    Labels,
}

impl UpdateOp {
    pub fn of(type_id: &str) -> Option<UpdateOp> {
        match type_id {
            SET_VARIABLES => Some(UpdateOp::Set),
            UNSET_VARIABLES => Some(UpdateOp::Unset),
            LABELS => Some(UpdateOp::Labels),
            _ => None,
        }
    }

    pub fn type_id(self) -> &'static str {
        match self {
            UpdateOp::Set => SET_VARIABLES,
            UpdateOp::Unset => UNSET_VARIABLES,
            UpdateOp::Labels => LABELS,
        }
    }
}

/// Read sets for `spec::schema::measure` (serde field names = Kestra's).
#[derive(Debug, Deserialize)]
pub struct SetSpec {
    pub variables: Option<Value>,
    #[serde(default)]
    pub overwrite: Option<Property<bool>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsetSpec {
    pub variables: Option<Property<Vec<String>>>,
    #[serde(default)]
    pub ignore_missing: Option<Property<bool>>,
}

#[derive(Debug, Deserialize)]
pub struct LabelsSpec {
    pub labels: Option<Value>,
}

/// What one update changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The execution's whole new `variables` object.
    Variables(Map<String, Value>),
    /// The execution's whole new label list (stored in `executions.labels`).
    Labels(Vec<Label>),
}

/// Compute one update. `spec` is the task's leaf spec as JSON text (from the
/// plan), `vars` the current `vars` object (`Value::Null` = none), `labels` the
/// current merged label list (flow then own).
pub fn compute(
    op: UpdateOp,
    spec: &str,
    cx: &RenderContext,
    vars: &Value,
    labels: &[Label],
) -> Result<Change, String> {
    let type_id = op.type_id();
    let raw: Value = serde_json::from_str(spec).map_err(|e| format!("{type_id}: {e}"))?;
    let current = vars.as_object().cloned().unwrap_or_default();
    match op {
        UpdateOp::Set => {
            let spec: SetSpec =
                serde_json::from_value(raw).map_err(|e| format!("{type_id}: {e}"))?;
            set(spec, cx, &current).map(Change::Variables)
        }
        UpdateOp::Unset => {
            let spec: UnsetSpec =
                serde_json::from_value(raw).map_err(|e| format!("{type_id}: {e}"))?;
            unset_all(spec, cx, current).map(Change::Variables)
        }
        UpdateOp::Labels => {
            let spec: LabelsSpec =
                serde_json::from_value(raw).map_err(|e| format!("{type_id}: {e}"))?;
            set_labels(spec, cx, labels).map(Change::Labels)
        }
    }
}

fn render_opt<T: serde::de::DeserializeOwned>(
    p: &Option<Property<T>>,
    name: &str,
    cx: &RenderContext,
) -> Result<Option<T>, String> {
    p.as_ref()
        .map(|p| p.render(cx).map_err(|e| format!("`{name}`: {e}")))
        .transpose()
}

fn set(
    spec: SetSpec,
    cx: &RenderContext,
    current: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let raw = match &spec.variables {
        Some(v) if !v.is_null() => v,
        _ => return Err("`variables`: must not be null".to_string()),
    };
    let rendered = render_map(raw, "variables", cx)?;
    let overwrite = render_opt(&spec.overwrite, "overwrite", cx)?.unwrap_or(true);
    if !overwrite {
        let existing: Vec<&str> = rendered
            .keys()
            .filter(|k| current.contains_key(*k))
            .map(String::as_str)
            .collect();
        if !existing.is_empty() {
            return Err(format!(
                "`overwrite` is set to false and the following variables already exist: {}",
                existing.join(",")
            ));
        }
    }
    Ok(deep_merge(current, &rendered))
}

fn unset_all(
    spec: UnsetSpec,
    cx: &RenderContext,
    mut vars: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let Some(keys) = render_opt(&spec.variables, "variables", cx)? else {
        return Err("`variables`: must not be null".to_string());
    };
    let ignore_missing = render_opt(&spec.ignore_missing, "ignoreMissing", cx)?.unwrap_or(false);
    for key in &keys {
        unset(&mut vars, key, ignore_missing)?;
    }
    Ok(vars)
}

/// Kestra's `MapUtils.deepMerge`: a null new value keeps the old one, two maps
/// merge, two lists concatenate (new items not already present, in order),
/// anything else is replaced.
pub fn deep_merge(a: &Map<String, Value>, b: &Map<String, Value>) -> Map<String, Value> {
    let mut out = a.clone();
    for (key, new) in b {
        match (out.get(key), new) {
            (Some(_), Value::Null) => {}
            (Some(Value::Object(old)), Value::Object(new)) => {
                let merged = deep_merge(old, new);
                out.insert(key.clone(), Value::Object(merged));
            }
            (Some(Value::Array(old)), Value::Array(new)) => {
                let mut merged = old.clone();
                for item in new {
                    if !old.contains(item) {
                        merged.push(item.clone());
                    }
                }
                out.insert(key.clone(), Value::Array(merged));
            }
            _ => {
                out.insert(key.clone(), new.clone());
            }
        }
    }
    out
}

/// Remove a dotted path. A leaf that is absent is a no-op; a parent that is
/// not a map is an error unless `ignore_missing`.
pub fn unset(vars: &mut Map<String, Value>, key: &str, ignore_missing: bool) -> Result<(), String> {
    unset_at(vars, key, key, ignore_missing)
}

fn unset_at(
    vars: &mut Map<String, Value>,
    rest: &str,
    full: &str,
    ignore_missing: bool,
) -> Result<(), String> {
    let Some((prefix, suffix)) = rest.split_once('.') else {
        vars.remove(rest);
        return Ok(());
    };
    match vars.get_mut(prefix) {
        Some(Value::Object(child)) => unset_at(child, suffix, full, ignore_missing),
        _ if ignore_missing => Ok(()),
        _ => Err(format!(
            "`variables`: cannot unset `{full}`: `{prefix}` is not a map"
        )),
    }
}

fn set_labels(
    spec: LabelsSpec,
    cx: &RenderContext,
    current: &[Label],
) -> Result<Vec<Label>, String> {
    let raw = match &spec.labels {
        Some(v) if !v.is_null() => v,
        _ => return Err("`labels`: must not be null".to_string()),
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    match raw {
        Value::String(source) => {
            let text = render_str(source, cx).map_err(|e| format!("`labels`: {e}"))?;
            let map: Map<String, Value> = serde_json::from_str(&text)
                .map_err(|e| format!("`labels`: `{text}` is not a JSON object of strings: {e}"))?;
            for (key, value) in map {
                let value = match value {
                    Value::String(s) => s,
                    v @ (Value::Number(_) | Value::Bool(_)) => v.to_string(),
                    _ => return Err(format!("`labels.{key}`: expected a string")),
                };
                put(&mut pairs, key, value);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let bad = || format!("`labels[{i}]`: expected a map with `key` and `value`");
                let obj = item.as_object().ok_or_else(bad)?;
                let key = obj.get("key").and_then(Value::as_str).ok_or_else(bad)?;
                let value = obj.get("value").ok_or_else(bad)?;
                let key = render_str(key, cx).map_err(|e| format!("`labels[{i}].key`: {e}"))?;
                let value = render_value(value, &key, cx)?;
                put(&mut pairs, key, value);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                let rendered = render_str(key, cx).map_err(|e| format!("`labels`: {e}"))?;
                if pairs.iter().any(|(k, _)| *k == rendered) {
                    return Err(format!("`labels`: two keys render to `{rendered}`"));
                }
                let value = render_value(value, &rendered, cx)?;
                pairs.push((rendered, value));
            }
        }
        _ => return Err("`labels`: expected a string, a list or a map".to_string()),
    }
    if let Some((k, v)) = pairs.iter().find(|(k, _)| k.starts_with("system.")) {
        return Err(format!(
            "System labels can only be set by Kestra itself, offending label: {k}={v}"
        ));
    }
    if let Some((k, _)) = pairs.iter().find(|(_, v)| v.is_empty()) {
        return Err(format!(
            "Label values cannot be empty, offending label: {k}"
        ));
    }
    let new: Vec<Label> = pairs
        .into_iter()
        .map(|(key, value)| Label { key, value })
        .collect();
    Ok(execution_labels(current, &new))
}

/// A later duplicate key wins, at the first one's position.
fn put(pairs: &mut Vec<(String, String)>, key: String, value: String) {
    match pairs.iter_mut().find(|(k, _)| *k == key) {
        Some(existing) => existing.1 = value,
        None => pairs.push((key, value)),
    }
}

/// Kestra's `renderValue`: a map is rendered and written as compact JSON, any
/// other value is rendered from its text.
fn render_value(value: &Value, key: &str, cx: &RenderContext) -> Result<String, String> {
    let field = format!("labels.{key}");
    match value {
        Value::Null => Err(format!("`{field}`: must not be null")),
        Value::Object(_) => {
            let map = render_map(value, &field, cx)?;
            serde_json::to_string(&map).map_err(|e| format!("`{field}`: {e}"))
        }
        Value::String(s) => render_str(s, cx).map_err(|e| format!("`{field}`: {e}")),
        other => render_str(&other.to_string(), cx).map_err(|e| format!("`{field}`: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn cx() -> RenderContext {
        let mut cx = RenderContext::new();
        cx.insert("inputs", json!({"k": "key", "v": "val", "n": 3}));
        cx
    }

    #[test]
    fn deep_merge_merges_maps_appends_new_list_items_and_keeps_old_on_null() {
        let a = obj(json!({"m": {"x": 1, "y": 2}, "l": [1, 2], "s": "old", "keep": 1}));
        let b = obj(json!({"m": {"y": 3, "z": 4}, "l": [2, 3], "s": "new", "keep": null}));
        assert_eq!(
            Value::Object(deep_merge(&a, &b)),
            json!({"m": {"x": 1, "y": 3, "z": 4}, "l": [1, 2, 3], "s": "new", "keep": 1})
        );
    }

    #[test]
    fn set_variables_on_no_variables_is_the_rendered_map() {
        let change = compute(
            UpdateOp::Set,
            r#"{"variables":{"a":"{{ inputs.n }}"}}"#,
            &cx(),
            &Value::Null,
            &[],
        )
        .unwrap();
        assert_eq!(change, Change::Variables(obj(json!({"a": "3"}))));
    }

    #[test]
    fn set_variables_with_overwrite_false_names_every_existing_key_in_order() {
        let err = compute(
            UpdateOp::Set,
            r#"{"variables":{"message":"Hello","name":"L","b":1},"overwrite":false}"#,
            &cx(),
            &json!({"name": "World", "b": 0}),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "`overwrite` is set to false and the following variables already exist: name,b"
        );
    }

    #[test]
    fn set_variables_without_variables_fails() {
        assert!(compute(UpdateOp::Set, r#"{}"#, &cx(), &Value::Null, &[])
            .unwrap_err()
            .contains("variables"));
    }

    #[test]
    fn unset_removes_a_dotted_leaf_and_a_missing_leaf_is_a_no_op() {
        let mut v = obj(json!({"a": {"b": {"c": 1, "d": 2}}, "list": []}));
        unset(&mut v, "a.b.c", false).unwrap();
        unset(&mut v, "list", false).unwrap();
        unset(&mut v, "absent", false).unwrap();
        assert_eq!(Value::Object(v), json!({"a": {"b": {"d": 2}}}));
    }

    #[test]
    fn unset_through_a_missing_parent_fails_unless_ignore_missing() {
        let mut v = obj(json!({"a": 1}));
        assert_eq!(
            unset(&mut v, "a.b", false).unwrap_err(),
            "`variables`: cannot unset `a.b`: `a` is not a map"
        );
        assert_eq!(
            unset(&mut v, "x.y", false).unwrap_err(),
            "`variables`: cannot unset `x.y`: `x` is not a map"
        );
        unset(&mut v, "x.y", true).unwrap();
        assert_eq!(Value::Object(v), json!({"a": 1}));
    }

    #[test]
    fn unset_variables_compute_applies_each_key_to_the_current_object() {
        let change = compute(
            UpdateOp::Unset,
            r#"{"variables":["a","b.c"],"ignoreMissing":true}"#,
            &cx(),
            &json!({"a": 1, "b": {"c": 2, "d": 3}}),
            &[],
        )
        .unwrap();
        assert_eq!(change, Change::Variables(obj(json!({"b": {"d": 3}}))));
    }

    #[test]
    fn labels_in_three_forms_override_the_current_list() {
        let current = vec![
            Label {
                key: "flowLabelKey".into(),
                value: "f".into(),
            },
            Label {
                key: "over".into(),
                value: "old".into(),
            },
        ];
        let from_string = compute(
            UpdateOp::Labels,
            r#"{"labels":"{\"a\":\"1\",\"a\":\"2\",\"n\":5}"}"#,
            &cx(),
            &Value::Null,
            &current,
        )
        .unwrap();
        let from_list = compute(UpdateOp::Labels, r#"{"labels":[{"key":"{{ inputs.k }}","value":"x"},{"key":"key","value":"{{ inputs.v }}"}]}"#, &cx(), &Value::Null, &current).unwrap();
        let from_map = compute(
            UpdateOp::Labels,
            r#"{"labels":{"over":"new","nested":{"name":"done"}}}"#,
            &cx(),
            &Value::Null,
            &current,
        )
        .unwrap();
        let kv = |c: Change| match c {
            Change::Labels(l) => l.into_iter().map(|l| (l.key, l.value)).collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(
            kv(from_string),
            [
                ("flowLabelKey", "f"),
                ("over", "old"),
                ("a", "2"),
                ("n", "5")
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert_eq!(
            kv(from_list),
            [("flowLabelKey", "f"), ("over", "old"), ("key", "val")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert_eq!(
            kv(from_map),
            [
                ("flowLabelKey", "f"),
                ("over", "new"),
                ("nested", "{\"name\":\"done\"}")
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
    }

    #[test]
    fn labels_refuse_a_system_key_before_an_empty_value() {
        let err = compute(
            UpdateOp::Labels,
            r#"{"labels":{"e":"","system.x":"v"}}"#,
            &cx(),
            &Value::Null,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "System labels can only be set by Kestra itself, offending label: system.x=v"
        );
        let err = compute(
            UpdateOp::Labels,
            r#"{"labels":"{ \"fromStringKey\": \"\", \"\": \"value2\" }"}"#,
            &cx(),
            &Value::Null,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "Label values cannot be empty, offending label: fromStringKey"
        );
    }

    #[test]
    fn labels_map_with_two_keys_rendering_alike_fails() {
        let err = compute(
            UpdateOp::Labels,
            r#"{"labels":{"key":"a","{{ inputs.k }}":"b"}}"#,
            &cx(),
            &Value::Null,
            &[],
        )
        .unwrap_err();
        assert_eq!(err, "`labels`: two keys render to `key`");
    }

    #[test]
    fn labels_string_form_refuses_a_nested_object() {
        assert!(compute(
            UpdateOp::Labels,
            r#"{"labels":"{\"a\":{\"b\":1}}"}"#,
            &cx(),
            &Value::Null,
            &[]
        )
        .is_err());
    }

    #[test]
    fn labels_string_form_with_a_duplicate_key_keeps_the_last_value() {
        let parsed: Map<String, Value> =
            serde_json::from_str(r#"{ "fromStringKey": "value1", "fromStringKey": "value2" }"#)
                .unwrap();
        assert_eq!(Value::Object(parsed), json!({"fromStringKey": "value2"}));
    }

    #[test]
    fn op_maps_its_type_id_both_ways() {
        for op in [UpdateOp::Set, UpdateOp::Unset, UpdateOp::Labels] {
            assert_eq!(UpdateOp::of(op.type_id()), Some(op));
        }
        assert_eq!(UpdateOp::of("io.kestra.plugin.core.log.Log"), None);
    }
}
