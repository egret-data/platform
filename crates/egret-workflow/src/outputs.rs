// kestra: core/src/main/java/io/kestra/core/runners/InputAndOutputImpl.java (renderOutputs)
// kestra: core/src/main/java/io/kestra/core/runners/FlowInputOutput.java (typedOutputs, parseData, parseType)
// kestra: core/src/main/java/io/kestra/core/exceptions/InputOutputValidationException.java (of(message, output))
//! The ONE outputs evaluator (package #6, spec D2): a `Loop`'s `outputs:`,
//! evaluated per iteration at the end of its sub-execution (D1), and a flow's
//! `outputs:`, evaluated when the execution ends (D6), both go through
//! [`evaluate`], so the two can never disagree.
//!
//! Pure: no database, no clock, no logging. The caller builds the render
//! context (the sub-execution's or the execution's surface,
//! `worker::render::Surface::context`) and writes [`Typed::warnings`] to the
//! execution's log.
//!
//! # What it does (`renderOutputs` then `typedOutputs`)
//!
//! 1. **Required** outputs (`required` absent or `true`) render together: the
//!    first that fails fails them all, and nothing is returned
//!    ([`OutputError`], naming that output).
//! 2. Each **optional** output (`required: false`) renders alone: a failure
//!    becomes `null` and a warning line (Kestra's
//!    `Failed to render optional flow output '<id>'. Output is ignored.`).
//! 3. Each value is coerced by its `type` with the input coercer of package
//!    #3 (`inputs_check::coerce_output`, Kestra's `parseType`). A `null`
//!    optional output stays `null` and is not coerced (`typedOutputs`). A
//!    coercion error fails the whole evaluation:
//!    ``Invalid value for output `<id>`. Cause: <cause>``.
//!
//! An empty list renders nothing and returns an empty map.
//!
//! # How a `value:` renders (`VariableRenderer.renderObject`)
//!
//! `FlowOutput::value` keeps its source as text (`Property`), a YAML map or
//! list re-serialised as JSON. A source that reads as a JSON object or array
//! is walked (`models::property::render_value`: string leaves rendered to
//! strings, other scalars kept); any other source is rendered as one string.
//! So `value: "{{ outputs.t.value }}"` is a string, as Kestra's `render`
//! gives, and the coercer turns it into the declared type.
//!
//! # SECRET and FILE
//!
//! A SECRET output's value is returned **in clear** in [`Typed::values`] and
//! its id listed in [`Typed::secret_ids`]: masking and sealing are the
//! caller's (spec D9), so that this module needs no key. A FILE output is
//! checked for shape only (its text); whether the URI is a file of this
//! execution is spec D10's check, not this module's.
//!
//! # Recorded divergences
//!
//! - **Every error names the output.** Kestra's render error and its
//!   non-`IllegalArgumentException` coercion errors (a JSON parse error)
//!   carry no id (`InputOutputValidationException.of(message)`); spec D2
//!   asks for the id in every one. A render error reads
//!   ``Unable to render output `<id>`. Cause: <render error>``.
//! - **A required output with no value** (`value:` absent, which save does
//!   not refuse) fails with ``Invalid value for output `<id>`. Cause: the
//!   output has no value``. Kestra's `@NotNull value` refuses it at save, and
//!   a `null` reaching `parseType` would be a `NullPointerException`.
//! - **No `type`** (refused at save and in the run-time gate, spec §2.2
//!   item 8) keeps the rendered value as is, as Kestra's `parseData` does
//!   for a `null` type -- defensive, should the gate ever be relaxed.
//! - DATETIME, DATE, TIME and DURATION keep the rendered text; Kestra
//!   normalises them through `Instant`/`LocalDate`/... (`inputs_check`'s
//!   own divergence, shared).

use serde_json::{Map, Value};

use crate::models::{FlowOutput, InputType};
use crate::runners::env::RenderContext;

/// The typed outputs of one evaluation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Typed {
    /// Output id -> typed value, in declared order. A failed optional output
    /// is `null`. SECRET values are in clear here (see the module doc).
    pub values: Map<String, Value>,
    /// The ids of the SECRET outputs whose value is not `null`, in declared
    /// order: what spec D9's caller masks and seals.
    pub secret_ids: Vec<String>,
    /// One line per optional output that failed to render, for the caller
    /// to log on the execution (Kestra's `runContext.logger().warn`).
    pub warnings: Vec<String>,
}

/// The output that failed, with the whole message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputError {
    pub output_id: String,
    pub message: String,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OutputError {}

/// Render and type `outputs` against `cx`. See the module doc.
pub fn evaluate(cx: &RenderContext, outputs: &[FlowOutput]) -> Result<Typed, OutputError> {
    let mut typed = Typed::default();
    if outputs.is_empty() {
        return Ok(typed);
    }

    // `renderOutputs`: the required ones together (one failure fails them
    // all), then each optional one alone.
    let mut rendered: Map<String, Value> = Map::new();
    for output in outputs.iter().filter(|o| is_required(o)) {
        let value = render(cx, output).map_err(|cause| OutputError {
            output_id: output.id.clone(),
            message: format!("Unable to render output `{}`. Cause: {cause}", output.id),
        })?;
        rendered.insert(output.id.clone(), value);
    }
    for output in outputs.iter().filter(|o| !is_required(o)) {
        let value = render(cx, output).unwrap_or_else(|cause| {
            typed.warnings.push(format!(
                "Failed to render optional flow output '{}'. Output is ignored. Cause: {cause}",
                output.id
            ));
            Value::Null
        });
        rendered.insert(output.id.clone(), value);
    }

    // `typedOutputs`, in declared order.
    for output in outputs {
        let current = rendered.get(&output.id).cloned().unwrap_or(Value::Null);
        let value = typed_value(output, current)?;
        if output.r#type == Some(InputType::Secret) && !value.is_null() {
            typed.secret_ids.push(output.id.clone());
        }
        typed.values.insert(output.id.clone(), value);
    }
    Ok(typed)
}

/// `required` absent or `true` (`Output.required`'s javadoc).
fn is_required(output: &FlowOutput) -> bool {
    output.required != Some(false)
}

/// One output's `value:` rendered, or the render error's text. An absent
/// `value:` is `null`.
fn render(cx: &RenderContext, output: &FlowOutput) -> Result<Value, String> {
    let Some(property) = &output.value else {
        return Ok(Value::Null);
    };
    let source = property.source();
    match serde_json::from_str::<Value>(source) {
        Ok(structured @ (Value::Object(_) | Value::Array(_))) => {
            crate::models::property::render_value(&structured, "value", cx)
        }
        _ => crate::render_str(source, cx)
            .map(Value::String)
            .map_err(|e| e.to_string()),
    }
}

/// `typedOutputs`' body for one output: a `null` optional output stays
/// `null`; otherwise `parseData`.
fn typed_value(output: &FlowOutput, current: Value) -> Result<Value, OutputError> {
    let fail = |cause: String| OutputError {
        output_id: output.id.clone(),
        message: format!("Invalid value for output `{}`. Cause: {cause}", output.id),
    };
    if current.is_null() {
        return if is_required(output) {
            Err(fail("the output has no value".into()))
        } else {
            Ok(Value::Null)
        };
    }
    let Some(t) = output.r#type else {
        return Ok(current);
    };
    match t {
        // `parseType`'s FILE arm reads `current.toString()` as a URI; the
        // URI's scope is spec D10's check (shape only here).
        InputType::File => Ok(Value::String(crate::inputs_check::java_text(&current))),
        // `parseType` throws `IllegalStateException` for these two.
        InputType::Form | InputType::ReusableInputs => Err(fail(format!(
            "an output cannot be of type {}",
            serde_json::to_value(t)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        ))),
        _ => crate::inputs_check::coerce_output(t, &current).map_err(fail),
    }
}

/// The most one `Loop` run's `outputs` list may serialise to in
/// `workspace`, in bytes (spec D4, Q5): [`MAX_LOOP_OUTPUTS_BYTES`]
/// (16 MiB), unless a test lowered it for its own workspace
/// ([`lower_loop_list_cap_for_tests`]), so a test of the cap needs no 16 MiB
/// of data and no other test sees the lowered value.
///
/// [`MAX_LOOP_OUTPUTS_BYTES`]: crate::runners::env::MAX_LOOP_OUTPUTS_BYTES
pub(crate) fn loop_list_cap(workspace: uuid::Uuid) -> usize {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(n) = lowered_caps()
        .lock()
        .ok()
        .and_then(|m| m.get(&workspace).copied())
    {
        return n;
    }
    let _ = workspace;
    crate::runners::env::MAX_LOOP_OUTPUTS_BYTES
}

#[cfg(any(test, feature = "test-util"))]
fn lowered_caps() -> &'static std::sync::Mutex<std::collections::HashMap<uuid::Uuid, usize>> {
    static CAPS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<uuid::Uuid, usize>>,
    > = std::sync::OnceLock::new();
    CAPS.get_or_init(Default::default)
}

/// Test-only: lower [`loop_list_cap`] to `bytes` for `workspace` alone.
#[cfg(any(test, feature = "test-util"))]
pub fn lower_loop_list_cap_for_tests(workspace: uuid::Uuid, bytes: usize) {
    if let Ok(mut m) = lowered_caps().lock() {
        m.insert(workspace, bytes);
    }
}

#[cfg(test)]
mod tests {
    //! Against survey §2 items 3-4 (`renderOutputs` + `typedOutputs`).
    use serde_json::json;

    use super::{evaluate, OutputError, Typed};
    use crate::models::FlowOutput;
    use crate::runners::env::RenderContext;

    fn cx() -> RenderContext {
        RenderContext::from_json_object(&json!({
            "outputs": {
                "t": {"value": "3", "word": "x", "list": [1, 2]},
            },
            "item": {"value": "a", "index": 0},
        }))
        .expect("an object")
    }

    /// Outputs written as YAML, as a flow or a `Loop` declares them.
    fn outputs(yaml: &str) -> Vec<FlowOutput> {
        serde_norway::from_str(yaml).expect("valid outputs YAML")
    }

    fn eval(yaml: &str) -> Result<Typed, OutputError> {
        evaluate(&cx(), &outputs(yaml))
    }

    #[test]
    fn two_required_one_fails_names_it_and_returns_nothing() {
        let err = eval(
            r#"
- id: ok
  type: STRING
  value: "{{ item.value }}"
- id: broken
  type: STRING
  value: "{{ outputs.missing.value }}"
"#,
        )
        .unwrap_err();
        assert_eq!(err.output_id, "broken");
        assert!(
            err.message
                .starts_with("Unable to render output `broken`. Cause: "),
            "{}",
            err.message
        );
    }

    #[test]
    fn one_optional_fails_is_null_with_a_warning_and_the_others_are_typed() {
        let typed = eval(
            r#"
- id: n
  type: INT
  value: "{{ outputs.t.value }}"
- id: maybe
  type: STRING
  required: false
  value: "{{ outputs.missing.value }}"
- id: s
  type: STRING
  required: false
  value: "{{ item.value }}"
"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::Value::Object(typed.values.clone()),
            json!({"n": 3, "maybe": null, "s": "a"})
        );
        // Declared order, not required-first.
        assert_eq!(typed.values.keys().collect::<Vec<_>>(), ["n", "maybe", "s"]);
        assert_eq!(typed.warnings.len(), 1, "{:?}", typed.warnings);
        assert!(
            typed.warnings[0]
                .starts_with("Failed to render optional flow output 'maybe'. Output is ignored."),
            "{:?}",
            typed.warnings
        );
        assert!(typed.secret_ids.is_empty());
    }

    #[test]
    fn int_from_text_coerces() {
        let typed = eval("- {id: n, type: INT, value: \"3\"}").unwrap();
        assert_eq!(typed.values["n"], json!(3));
        assert!(typed.values["n"].is_i64());
    }

    #[test]
    fn int_from_a_word_fails_with_the_coercers_text() {
        let err = eval("- {id: n, type: INT, value: \"{{ outputs.t.word }}\"}").unwrap_err();
        assert_eq!(err.output_id, "n");
        assert_eq!(
            err.message,
            "Invalid value for output `n`. Cause: Cannot convert value 'x' to Integer"
        );
    }

    #[test]
    fn a_coercion_failure_of_an_optional_output_fails_too() {
        // `typedOutputs` throws for any non-null value that does not coerce,
        // required or not; only a render failure is forgiven.
        let err = eval("- {id: n, type: INT, required: false, value: \"x\"}").unwrap_err();
        assert_eq!(err.output_id, "n");
    }

    #[test]
    fn json_from_text_and_from_a_map() {
        let typed = eval(
            r#"
- id: text
  type: JSON
  value: '{{ outputs.t.list }}'
- id: map
  type: JSON
  value:
    a: "{{ item.value }}"
    b: 2
- id: literal
  type: JSON
  value: '"just a string"'
"#,
        )
        .unwrap();
        assert_eq!(typed.values["text"], json!([1, 2]));
        // A map is walked: string leaves rendered, other scalars kept.
        assert_eq!(typed.values["map"], json!({"a": "a", "b": 2}));
        // Unlike an input's stored form, not re-stringified.
        assert_eq!(typed.values["literal"], json!("just a string"));
    }

    #[test]
    fn json_that_does_not_parse_fails_naming_the_output() {
        let err = eval("- {id: j, type: JSON, value: \"{{ outputs.t.word }}\"}").unwrap_err();
        assert_eq!(err.output_id, "j");
        assert!(
            err.message
                .starts_with("Invalid value for output `j`. Cause: "),
            "{}",
            err.message
        );
    }

    #[test]
    fn boolean_coerces_and_refuses() {
        let typed = eval("- {id: b, type: BOOL, value: \"TRUE\"}").unwrap();
        assert_eq!(typed.values["b"], json!(true));
        let err = eval("- {id: b, type: BOOL, value: \"yes\"}").unwrap_err();
        assert_eq!(
            err.message,
            "Invalid value for output `b`. Cause: Cannot convert value 'yes' to Boolean"
        );
    }

    #[test]
    fn datetime_is_checked_and_kept_as_text() {
        let typed = eval("- {id: d, type: DATETIME, value: \"2026-10-29T10:00:00Z\"}").unwrap();
        assert_eq!(typed.values["d"], json!("2026-10-29T10:00:00Z"));
        let err = eval("- {id: d, type: DATETIME, value: \"yesterday\"}").unwrap_err();
        assert_eq!(
            err.message,
            "Invalid value for output `d`. Cause: Cannot convert value 'yesterday' to Instant"
        );
    }

    #[test]
    fn file_is_its_text_shape_only() {
        let typed = eval(
            "- {id: f, type: FILE, value: \"kestra:///ns/flow/executions/e/tasks/t/r/out.ion\"}",
        )
        .unwrap();
        assert_eq!(
            typed.values["f"],
            json!("kestra:///ns/flow/executions/e/tasks/t/r/out.ion")
        );
    }

    #[test]
    fn a_secret_is_reported_by_id_and_returned_in_clear() {
        let typed = eval(
            r#"
- id: plain
  type: STRING
  value: "p"
- id: s
  type: SECRET
  value: "{{ item.value }}"
- id: absent
  type: SECRET
  required: false
  value: "{{ outputs.missing.value }}"
"#,
        )
        .unwrap();
        assert_eq!(typed.secret_ids, vec!["s".to_string()]);
        assert_eq!(typed.values["s"], json!("a"));
        assert_eq!(typed.values["absent"], json!(null));
    }

    #[test]
    fn an_empty_list_returns_an_empty_map_without_rendering() {
        // An empty context: rendering anything against it would fail.
        let empty = RenderContext::from_json_object(&json!({})).unwrap();
        let typed = evaluate(&empty, &[]).unwrap();
        assert_eq!(typed, Typed::default());
    }

    #[test]
    fn a_required_output_with_no_value_fails() {
        let err = eval("- {id: n, type: STRING}").unwrap_err();
        assert_eq!(
            err.message,
            "Invalid value for output `n`. Cause: the output has no value"
        );
        // Optional: null, no coercion.
        let typed = eval("- {id: n, type: STRING, required: false}").unwrap();
        assert_eq!(typed.values["n"], json!(null));
    }

    #[test]
    fn no_type_keeps_the_rendered_value() {
        let typed = eval("- {id: n, value: \"{{ outputs.t.value }}\"}").unwrap();
        assert_eq!(typed.values["n"], json!("3"));
    }

    #[test]
    fn a_map_value_that_fails_to_render_names_the_output() {
        let err = eval(
            r#"
- id: m
  type: JSON
  value:
    k: "{{ outputs.missing.value }}"
"#,
        )
        .unwrap_err();
        assert_eq!(err.output_id, "m");
        assert!(
            err.message
                .starts_with("Unable to render output `m`. Cause: `value.k`: "),
            "{}",
            err.message
        );
    }
}
