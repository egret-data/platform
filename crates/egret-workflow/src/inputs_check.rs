// kestra: core/src/main/java/io/kestra/core/runners/FlowInputOutput.java (resolveInputs, resolveInputValue, parseType)
// kestra: core/src/main/java/io/kestra/core/models/flows/input/*.java (validate, render, getDefaults)
// kestra: core/src/main/java/io/kestra/core/utils/TypeConverter.java
// kestra: core/src/main/java/io/kestra/core/exceptions/InputOutputValidationException.java
// jdk: java.time.{Duration,Instant,LocalDate,LocalTime}#parse/#toString, java.lang.Float#toString
//! The start-time input check (package #3, spec D1-D6, D9, D10, D12, D13):
//! the flow's declared `inputs:` against the posted object, giving the
//! **coerced** object that is stored (D2) or the **first** error in declared
//! order (D1).
//!
//! Pure: no database and no clock. The one thing it cannot do alone is
//! render a template, so the caller hands it an [`InputRenderer`] for
//! `dependsOn.condition` and a SELECT / MULTISELECT `expression` (Kestra's
//! `runContext.renderTyped`). The start path, the subflow child and (B1) the
//! validate endpoint share it and can never disagree.
//!
//! # Per input (`FlowInputOutput.resolveInputValue`)
//!
//! Inputs are visited in declared order; an input's `dependsOn.inputs` are
//! resolved first, recursively.
//!
//! 1. **Enabled?** Every known dependency (an unknown id is ignored, the
//!    input's own id too) must be enabled and non-null. Then a
//!    `dependsOn.condition` is rendered with `inputs` bound to the
//!    dependencies' values; only a boolean `true` enables. A render failure
//!    disables the input and records ``Invalid condition: <msg>`` on it.
//!    A disabled input is dropped, skips `required`, and its errors are not
//!    reported at start (Kestra filters disabled inputs before it throws);
//!    [`resolve_all`] still lists them. A dependency cycle (refused at save,
//!    Task A5) disables the inputs on it rather than recursing forever.
//! 2. **Options.** A SELECT / MULTISELECT `expression` is rendered with the
//!    same bindings; its non-null items replace `values`.
//! 3. An explicit `null`, or `""` for any type but STRING, SELECT, EMAIL and
//!    SECRET (D5, `isTextType`), is **absent**.
//! 4. Absent with a `defaults`: a literal default (no `{{`/`{%`) is parsed and
//!    validated like a posted value, and stays absent in the result -- the
//!    render still fills it (D2). A templated default is not checked here.
//!    `autoSelectFirst` with no `defaults` picks the first option, and that
//!    value **is** stored: the render knows nothing of `autoSelectFirst`.
//! 5. Absent with no default: `Missing required input:<id>` when `required`
//!    (default true), otherwise absent (Kestra keeps a `null`; egret leaves the
//!    key out, which renders the same).
//! 6. Parse, then validate. A failure is
//!    ``Invalid value for input `<id>`. Cause: <cause>`` (D3); a MULTISELECT
//!    gives one such line per bad element, joined by `\n`.
//!
//! Undeclared keys are dropped (Kestra drops them with a WARN).
//!
//! # What is stored
//!
//! INT, FLOAT and BOOL are stored as typed JSON. DATETIME, DATE, TIME and
//! DURATION are validated and bound-checked but stored **as posted**
//! (`P1D` stays `P1D`, `10:00:00` stays `10:00:00`). JSON, YAML and ION are
//! stored as the parsed value. ARRAY and MULTISELECT are stored as the list
//! of parsed elements.
//!
//! # Not here
//!
//! FILE is checked by `storage::inputs::check_file_inputs`; FORM and
//! REUSABLE_INPUTS are not supported. All three pass through unchanged.
//! `jsonSchema` is **not checked** (D12): no JSON Schema validator is in the
//! dependency tree, so the key stays a `not_honoured` warning (`spec::keys`).
//!
//! # Recorded divergences
//!
//! - **BOOL** accepts only a JSON boolean or `true`/`false` in any case;
//!   Kestra's `Boolean.parseBoolean` turns anything else into `false` (D4).
//!   The refusal reads ``Cannot convert value '<v>' to Boolean``.
//! - **FLOAT** refuses `NaN`, `Infinity` and values out of `float` range
//!   (D4); Java's hexadecimal float syntax is refused too.
//! - **INT** accepts a JSON number with no fractional part (`1.0`) -- Kestra's
//!   API is multipart, so it never sees a JSON number (D4).
//! - **DATETIME** is `ISO_INSTANT` with a four-digit year and seconds; Java's
//!   `24:00:00` end-of-day and `:60` leap second are refused.
//! - **validator** is `fancy-regex` with a backtrack limit, not
//!   `java.util.regex` with a 10 s timeout (D10). A pattern that does not
//!   compile (save time refuses it, Task A5) gives ``invalid pattern [<v>]``.
//! - **URI** syntax errors follow `java.net.URI`'s message shapes
//!   (`Illegal character in <part> at index <n>: <uri>`) for the common
//!   cases only; a missing scheme is Kestra's `Invalid URI format.` exactly.
//! - **JSON / YAML / ION / ARRAY parse errors** are the prefixed form with the
//!   parser's own message (`serde_json`, `serde_norway`, `ion-rs`), not
//!   Kestra's code-block wrapper around Jackson's (D3). An ARRAY / MULTISELECT
//!   value that is neither a list nor a string holding a JSON list gives
//!   ``Cannot convert value '<v>' to List`` (Kestra: a `ClassCastException`
//!   text with no prefix).
//! - **ION** is read with egret's Ion reader (`storage::ion`, the preview's
//!   JSON table: a timestamp is its Ion text, a decimal a double); Kestra's
//!   `IonObjectMapper` gives Java objects. Only the first value is read.
//! - **MULTISELECT membership** compares each parsed element's text, so an
//!   `itemType: INT` element `1` matches the option `"1"`; Kestra compares an
//!   `Integer` to a `String` and refuses every element.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use serde::Deserialize as _;
use serde_json::{Map, Value};

use crate::models::{Input, InputType};

/// The first input that failed, with Kestra's full message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputError {
    pub input_id: String,
    /// The whole text, e.g. ``Invalid value for input `n`. Cause: it must be
    /// more than `1` `` or `Missing required input:n`. Several lines (a
    /// MULTISELECT with several bad elements) are joined by `\n`.
    pub message: String,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InputError {}

/// The backtrack budget a `validator` match may spend before it is reported
/// as Kestra's timeout. `fancy-regex`'s own default, named so it is a choice
/// rather than an accident: one million steps is well under a second.
pub const VALIDATOR_BACKTRACK_LIMIT: usize = 1_000_000;

/// Kestra's `runContext.renderTyped(expression)` for an input: `inputs` is
/// what the template sees as `inputs` -- the input's dependencies and their
/// values (an unset one is `null`). The caller adds the rest of the context
/// (flow, execution, ...). `Err` is the renderer's message.
///
/// Used for `dependsOn.condition` (only `Value::Bool(true)` enables), a
/// SELECT / MULTISELECT `expression` (must give a list), and a templated
/// `defaults` that another input depends on (its value, never validated).
pub trait InputRenderer {
    fn render_typed(&self, template: &str, inputs: &Map<String, Value>) -> Result<Value, String>;
}

impl<F> InputRenderer for F
where
    F: Fn(&str, &Map<String, Value>) -> Result<Value, String>,
{
    fn render_typed(&self, template: &str, inputs: &Map<String, Value>) -> Result<Value, String> {
        self(template, inputs)
    }
}

/// One input's outcome -- Kestra's `InputAndValue`.
#[derive(Debug, Clone, PartialEq)]
pub struct InputOutcome {
    pub id: String,
    /// What is stored; `None` leaves the key out.
    pub value: Option<Value>,
    /// What a dependent sees as this input's value: the stored value, a
    /// literal default's parsed value, or -- on an error -- the value as
    /// posted (Kestra keeps the raw value on an errored input).
    pub seen: Option<Value>,
    pub enabled: bool,
    /// The value came from `defaults` or `autoSelectFirst`.
    pub is_default: bool,
    /// Full messages, in Kestra's form.
    pub errors: Vec<String>,
}

/// Check `posted` against `inputs` and return the coerced object, or the
/// first error in declared order among the **enabled** inputs (spec D1).
///
/// `posted` that is not an object is treated as `{}`.
pub fn resolve(
    inputs: &[Input],
    posted: &Value,
    renderer: &dyn InputRenderer,
) -> Result<Map<String, Value>, InputError> {
    let mut out = Map::new();
    for outcome in resolve_all(inputs, posted, renderer) {
        if !outcome.enabled {
            continue;
        }
        if !outcome.errors.is_empty() {
            return Err(InputError {
                input_id: outcome.id,
                message: outcome.errors.join("\n"),
            });
        }
        if let Some(v) = outcome.value {
            out.insert(outcome.id, v);
        }
    }
    Ok(out)
}

/// Every declared input's outcome, in declared order (a repeated id keeps its
/// first declaration, as Kestra's `toMap` merge does). The validate endpoint
/// (Task B1) lists these; [`resolve`] folds over them.
pub fn resolve_all(
    inputs: &[Input],
    posted: &Value,
    renderer: &dyn InputRenderer,
) -> Vec<InputOutcome> {
    let mut index = HashMap::new();
    let mut unique = Vec::new();
    for input in inputs {
        if !index.contains_key(input.id.as_str()) {
            index.insert(input.id.as_str(), unique.len());
            unique.push(input);
        }
    }
    let depended = inputs
        .iter()
        .filter_map(|i| i.depends_on.as_ref())
        .flat_map(|d| d.inputs.iter().map(String::as_str))
        .collect();
    let mut resolver = Resolver {
        states: unique.iter().map(|_| State::Pending).collect(),
        inputs: unique,
        index,
        depended,
        posted: posted.as_object(),
        renderer,
    };
    for k in 0..resolver.inputs.len() {
        resolver.visit(k);
    }
    resolver
        .states
        .into_iter()
        .filter_map(|s| match s {
            State::Done(o) => Some(o),
            State::Pending | State::Visiting => None,
        })
        .collect()
}

enum State {
    Pending,
    /// On the current dependency path: reaching it again is a cycle.
    Visiting,
    Done(InputOutcome),
}

struct Resolver<'a> {
    inputs: Vec<&'a Input>,
    index: HashMap<&'a str, usize>,
    /// Ids some input names in `dependsOn.inputs`: only their templated
    /// default is worth rendering.
    depended: HashSet<&'a str>,
    posted: Option<&'a Map<String, Value>>,
    renderer: &'a dyn InputRenderer,
    states: Vec<State>,
}

impl Resolver<'_> {
    /// `(enabled, seen)` of input `k`, resolving it first if need be; `None`
    /// when `k` is already on the path (a cycle).
    fn visit(&mut self, k: usize) -> Option<(bool, Option<Value>)> {
        match &self.states[k] {
            State::Done(o) => return Some((o.enabled, o.seen.clone())),
            State::Visiting => return None,
            State::Pending => {}
        }
        self.states[k] = State::Visiting;
        let outcome = self.decide(k);
        let summary = (outcome.enabled, outcome.seen.clone());
        self.states[k] = State::Done(outcome);
        Some(summary)
    }

    fn decide(&mut self, k: usize) -> InputOutcome {
        let input = self.inputs[k];
        let raw = self
            .posted
            .and_then(|m| m.get(&input.id))
            .filter(|v| !v.is_null());
        let mut out = InputOutcome {
            id: input.id.clone(),
            value: None,
            seen: raw.cloned(),
            enabled: true,
            is_default: false,
            errors: Vec::new(),
        };
        if matches!(input.r#type, InputType::Form | InputType::ReusableInputs) {
            out.value = raw.cloned();
            return out;
        }

        // 1. Enabled? (`resolveAllDependentInputs`, then the condition.)
        let mut deps = Map::new();
        if let Some(depends_on) = &input.depends_on {
            let mut enabled = true;
            for id in &depends_on.inputs {
                if *id == input.id {
                    continue;
                }
                let Some(&j) = self.index.get(id.as_str()) else {
                    continue; // unknown: ignored at run time (refused at save)
                };
                let (dep_enabled, seen) = self.visit(j).unwrap_or((false, None));
                enabled &= dep_enabled && seen.is_some();
                deps.insert(id.clone(), seen.unwrap_or(Value::Null));
            }
            if enabled {
                if let Some(condition) = &depends_on.condition {
                    match self.renderer.render_typed(condition, &deps) {
                        Ok(Value::Bool(true)) => {}
                        Ok(_) => enabled = false,
                        Err(msg) => {
                            out.errors.push(format!("Invalid condition: {msg}"));
                            enabled = false;
                        }
                    }
                }
            }
            if !enabled {
                out.enabled = false;
                return out;
            }
        }

        // 2. Options from `expression` (`RenderableInput.mayRenderInput`).
        let rendered;
        let input = match (input.r#type, &input.expression) {
            (InputType::Select | InputType::Multiselect, Some(expression)) => {
                match self.render_options(input, expression, &deps) {
                    Ok(values) => {
                        rendered = Input {
                            values: Some(values),
                            ..input.clone()
                        };
                        &rendered
                    }
                    Err(message) => {
                        out.errors.push(message);
                        return out;
                    }
                }
            }
            _ => input,
        };

        // 3. Absent?
        let value = raw.filter(|v| match v {
            Value::String(s) if s.is_empty() => is_text_type(input.r#type),
            _ => true,
        });
        let fail = |causes: Vec<String>| causes.iter().map(|c| invalid(input, c)).collect();

        if let Some(value) = value {
            match check_value(input, value) {
                Ok(v) => {
                    out.seen = Some(v.clone());
                    out.value = Some(v);
                }
                Err(causes) => out.errors = fail(causes),
            }
            return out;
        }

        // 4. Defaults.
        if let Some(default) = &input.defaults {
            out.is_default = true;
            let source = default.source();
            if is_templated(source) {
                // Not validated (D2); rendered only for a dependent to see.
                out.seen = if self.depended.contains(input.id.as_str()) {
                    self.renderer
                        .render_typed(source, &deps)
                        .ok()
                        .filter(|v| !v.is_null())
                } else {
                    None
                };
            } else {
                match check_value(input, &Value::String(source.to_string())) {
                    Ok(v) => out.seen = Some(v),
                    Err(causes) => out.errors = fail(causes),
                }
            }
            return out;
        }
        match auto_select_first(input) {
            Ok(Some(first)) => {
                out.is_default = true;
                match check_value(input, &first) {
                    Ok(v) => {
                        out.seen = Some(v.clone());
                        out.value = Some(v);
                    }
                    Err(causes) => out.errors = fail(causes),
                }
                return out;
            }
            Ok(None) => {}
            Err(cause) => {
                out.errors = fail(vec![cause]);
                return out;
            }
        }

        // 5. Required.
        if input.required {
            out.errors
                .push(format!("Missing required input:{}", input.id));
        } else {
            out.seen = None;
        }
        out
    }

    /// `SelectInput.renderExpressionValues` / `MultiselectInput`'s: the
    /// rendered list's non-null items, or the full (render-error) message.
    fn render_options(
        &self,
        input: &Input,
        expression: &str,
        deps: &Map<String, Value>,
    ) -> Result<Vec<Value>, String> {
        // A ConstraintViolationException's message is `<path>: <message>`.
        let violation = |msg: String| invalid(input, &format!("{}: {msg}", input.id));
        let result = self
            .renderer
            .render_typed(expression.trim(), deps)
            .map_err(|e| violation(format!("Cannot render 'expression'. Cause: {e}")))?;
        let Value::Array(items) = result else {
            return Err(violation(
                "Invalid expression result. Expected a list of values".into(),
            ));
        };
        let items: Vec<Value> = items.into_iter().filter(|v| !v.is_null()).collect();
        // `ValueOption.from` throws on an object with no `value`, inside the render.
        for item in &items {
            option_value(item).map_err(|cause| invalid(input, &cause))?;
        }
        Ok(items)
    }
}

/// ``Invalid value for input `<id>`. Cause: <cause>`` --
/// `InputOutputValidationException.of(message, input)`.
fn invalid(input: &Input, cause: &str) -> String {
    format!("Invalid value for input `{}`. Cause: {cause}", input.id)
}

/// `FlowInputOutput.isTextType`: the types for which `""` is a value.
fn is_text_type(t: InputType) -> bool {
    matches!(
        t,
        InputType::String | InputType::Select | InputType::Email | InputType::Secret
    )
}

/// A default whose source would be rendered: not checked at start (D2).
fn is_templated(source: &str) -> bool {
    source.contains("{{") || source.contains("{%")
}

/// `SelectInput.getDefaults` / `MultiselectInput.getDefaults` with no
/// `defaults`: the first option when `autoSelectFirst`.
fn auto_select_first(input: &Input) -> Result<Option<Value>, String> {
    if input.auto_select_first != Some(true) {
        return Ok(None);
    }
    let Some(first) = input.values.as_ref().and_then(|v| v.first()) else {
        return Ok(None);
    };
    let first = Value::String(option_value(first)?);
    Ok(match input.r#type {
        InputType::Select => Some(first),
        InputType::Multiselect => Some(Value::Array(vec![first])),
        _ => None,
    })
}

/// The element type an input's list is parsed with: MULTISELECT defaults to
/// STRING (`@Builder.Default`), ARRAY has none (its list passes through).
fn item_type_of(input: &Input) -> Option<InputType> {
    match input.r#type {
        InputType::Multiselect => Some(input.item_type.unwrap_or(InputType::String)),
        InputType::Array => input.item_type,
        _ => None,
    }
}

/// `parseType` then `Input.validate`: the value to store, or the causes.
fn check_value(input: &Input, value: &Value) -> Result<Value, Vec<String>> {
    let parsed = parse_type(input.r#type, item_type_of(input), value, Some(input))
        .map_err(|cause| vec![cause])?;
    validate(input, &parsed)?;
    Ok(parsed)
}

/// `FlowInputOutput.parseType`. `top` is the declared input when `value` is
/// its whole value (a SECRET checks its `validator` here, as Kestra does);
/// `None` for an ARRAY / MULTISELECT element.
fn parse_type(
    t: InputType,
    item_type: Option<InputType>,
    value: &Value,
    top: Option<&Input>,
) -> Result<Value, String> {
    match t {
        InputType::String | InputType::Select | InputType::Email => {
            Ok(Value::String(java_to_string(value)))
        }
        InputType::Secret => {
            let s = java_to_string(value);
            if let Some(input) = top {
                // Thrown from inside `parseType` as a ConstraintViolationException,
                // whose message is `<path>: <msg>` (survey §1.2 (b)).
                check_validator(input.validator.as_deref(), &s)
                    .map_err(|cause| format!("{}: {cause}", input.id))?;
            }
            Ok(Value::String(s))
        }
        InputType::Int => to_int(value).map(Value::from),
        InputType::Float => to_float(value).map(float_value),
        InputType::Boolean => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
            other => Err(cannot_convert(other, "Boolean")),
        },
        // Parsed to check, stored as posted.
        InputType::Datetime => {
            to_bounded(value, "Instant", parse_instant).map(|_| as_posted(value))
        }
        InputType::Date => to_bounded(value, "LocalDate", parse_date).map(|_| as_posted(value)),
        InputType::Time => to_bounded(value, "LocalTime", parse_time).map(|_| as_posted(value)),
        InputType::Duration => {
            to_bounded(value, "Duration", JavaDuration::parse).map(|_| as_posted(value))
        }
        InputType::Uri => {
            let s = java_to_string(value);
            check_uri(&s)?;
            Ok(Value::String(s))
        }
        InputType::Json => structured(value, parse_json),
        InputType::Yaml => structured(value, |s| {
            serde_norway::from_str::<Value>(s).map_err(|e| e.to_string())
        }),
        InputType::Ion => structured(value, parse_ion),
        InputType::Array | InputType::Multiselect => {
            let list = match value {
                Value::Array(items) => items.clone(),
                Value::String(s) => match parse_json(s)? {
                    Value::Array(items) => items,
                    _ => return Err(cannot_convert(value, "List")),
                },
                other => return Err(cannot_convert(other, "List")),
            };
            let Some(item) = item_type else {
                return Ok(Value::Array(list));
            };
            list.iter()
                .map(|element| {
                    let parsed = if element.is_null() {
                        Err(String::new())
                    } else {
                        parse_type(item, None, element, None)
                    };
                    parsed.map_err(|_| {
                        format!(
                            "Unable to parse array element as `{}` on `{}`",
                            type_name(item),
                            java_to_string(element)
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        InputType::File | InputType::Form | InputType::ReusableInputs => Ok(value.clone()),
    }
}

/// The text the user posted, kept for storage (DATETIME, DATE, TIME,
/// DURATION).
fn as_posted(value: &Value) -> Value {
    Value::String(java_to_string(value))
}

/// JSON / YAML / ION: a map or a list passes through, anything else is read
/// from its text.
fn structured(
    value: &Value,
    parse: impl Fn(&str) -> Result<Value, String>,
) -> Result<Value, String> {
    match value {
        Value::Object(_) | Value::Array(_) => Ok(value.clone()),
        other => parse(&java_to_string(other)),
    }
}

/// `JacksonMapper.toObject` / `toList`: one value, the rest of the text not
/// looked at (Jackson's default `FAIL_ON_TRAILING_TOKENS = false`).
fn parse_json(s: &str) -> Result<Value, String> {
    let mut de = serde_json::Deserializer::from_str(s);
    Value::deserialize(&mut de).map_err(|e| e.to_string())
}

/// `ION_MAPPER.readValue`: the first Ion value, through egret's guarded Ion
/// reader and its JSON table (`storage::ion`).
fn parse_ion(s: &str) -> Result<Value, String> {
    let (rows, _) = crate::storage::ion::values_to_json(s.as_bytes(), 1)?;
    rows.into_iter()
        .next()
        .ok_or_else(|| "No content to map due to end-of-input".to_string())
}

/// The type as Kestra prints it (`INT`, `BOOL`, `DATETIME`).
fn type_name(t: InputType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `Input.validate` on the parsed value.
fn validate(input: &Input, parsed: &Value) -> Result<(), Vec<String>> {
    let one = |r: Result<(), String>| r.map_err(|cause| vec![cause]);
    let text = || java_to_string(parsed);
    match input.r#type {
        InputType::String => one(check_validator(input.validator.as_deref(), &text())),
        InputType::Email => {
            if EMAIL.is_match(&text()) {
                Ok(())
            } else {
                one(Err("The input must be a valid email".into()))
            }
        }
        InputType::Select => one(check_select(input, &text())),
        InputType::Multiselect => check_multiselect(input, parsed),
        InputType::Int => one(check_int_bounds(input, parsed)),
        InputType::Float => one(check_float_bounds(input, parsed)),
        InputType::Datetime => one(to_bounded(parsed, "Instant", parse_instant).and_then(|t| {
            check_after_before(
                &t,
                input.after.as_deref(),
                input.before.as_deref(),
                "Instant",
                parse_instant,
                format_instant,
            )
        })),
        InputType::Date => one(to_bounded(parsed, "LocalDate", parse_date).and_then(|d| {
            check_after_before(
                &d,
                input.after.as_deref(),
                input.before.as_deref(),
                "LocalDate",
                parse_date,
                |d: &chrono::NaiveDate| d.format("%Y-%m-%d").to_string(),
            )
        })),
        InputType::Time => one(to_bounded(parsed, "LocalTime", parse_time).and_then(|t| {
            check_after_before(
                &t,
                input.after.as_deref(),
                input.before.as_deref(),
                "LocalTime",
                parse_time,
                format_time,
            )
        })),
        InputType::Duration => one(check_duration_bounds(input, parsed)),
        // JSON's `jsonSchema` is not checked (D12, module doc).
        _ => Ok(()),
    }
}

fn check_int_bounds(input: &Input, parsed: &Value) -> Result<(), String> {
    let n = to_int(parsed)?;
    if let Some(min) = &input.min {
        let min = to_int(min)?;
        if n < min {
            return Err(format!("it must be more than `{min}`"));
        }
    }
    if let Some(max) = &input.max {
        let max = to_int(max)?;
        if n > max {
            return Err(format!("it must be less than `{max}`"));
        }
    }
    Ok(())
}

fn check_float_bounds(input: &Input, parsed: &Value) -> Result<(), String> {
    let x = to_float(parsed)?;
    if let Some(min) = &input.min {
        let min = to_float(min)?;
        if x < min {
            return Err(format!("it must be more than `{}`", java_float(min)));
        }
    }
    if let Some(max) = &input.max {
        let max = to_float(max)?;
        if x > max {
            return Err(format!("it must be less than `{}`", java_float(max)));
        }
    }
    Ok(())
}

/// `DurationInput.validate`: the cause starts with a capital I (Kestra's).
fn check_duration_bounds(input: &Input, parsed: &Value) -> Result<(), String> {
    let d = to_bounded(parsed, "Duration", JavaDuration::parse)?;
    if let Some(min) = &input.min {
        let min = to_bounded(min, "Duration", JavaDuration::parse)?;
        if d < min {
            return Err(format!("It must be more than `{min}`"));
        }
    }
    if let Some(max) = &input.max {
        let max = to_bounded(max, "Duration", JavaDuration::parse)?;
        if d > max {
            return Err(format!("It must be less than `{max}`"));
        }
    }
    Ok(())
}

/// `MultiselectInput.validate`: every element against the options unless
/// `allowCustomValue`, whether or not the input is required; one violation
/// per distinct bad element (Kestra collects them in a `HashSet`), in
/// element order. With no `values` (and no `expression`) nothing is checked.
fn check_multiselect(input: &Input, parsed: &Value) -> Result<(), Vec<String>> {
    if input.allow_custom_value {
        return Ok(());
    }
    let Some(values) = &input.values else {
        return Ok(());
    };
    let options = values
        .iter()
        .map(option_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|cause| vec![cause])?;
    let Value::Array(items) = parsed else {
        return Ok(());
    };
    let listed = options.join(", ");
    let mut causes = Vec::new();
    for item in items {
        let s = java_to_string(item);
        if !options.contains(&s) {
            let cause = format!("value `{s}` doesn't match the values `[{listed}]`");
            if !causes.contains(&cause) {
                causes.push(cause);
            }
        }
    }
    if causes.is_empty() {
        Ok(())
    } else {
        Err(causes)
    }
}

/// ``Cannot convert value '<v>' to <Class>`` -- `TypeConverter`'s message.
fn cannot_convert(value: &Value, class: &str) -> String {
    format!(
        "Cannot convert value '{}' to {class}",
        java_to_string(value)
    )
}

// ---------------------------------------------------------------------------
// STRING / SECRET `validator`, EMAIL, SELECT

/// `RegexUtils.matches`: a FULL match (`Matcher.matches`), D10.
fn check_validator(validator: Option<&str>, s: &str) -> Result<(), String> {
    let Some(v) = validator else {
        return Ok(());
    };
    // `\A(?:..)\z` is `Matcher.matches`: the whole text, with alternation
    // backtracking into a longer branch when a shorter one leaves text over.
    let re = fancy_regex::RegexBuilder::new(&format!(r"\A(?:{v})\z"))
        .backtrack_limit(VALIDATOR_BACKTRACK_LIMIT)
        .build()
        .map_err(|_| format!("invalid pattern [{v}]"))?;
    match re.is_match(s) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("it must match the pattern `{v}`")),
        Err(_) => Err(format!(
            "the validator pattern `{v}` timed out — it may be too complex"
        )),
    }
}

/// `EmailInput.EMAIL_PATTERN`, under `Pattern.matches` (whole text). The
/// `’` (U+2019) in the class is Kestra's, not an apostrophe.
static EMAIL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\A(?:|[a-zA-Z0-9_!#$%&’*+/=?`{|}~^.-]+@[a-zA-Z0-9.-]+)\z")
        .expect("a fixed pattern")
});

/// `SelectInput.validate`: membership only when `required` (Kestra's quirk,
/// D4), unless `allowCustomValue`. With no `values` (and no `expression`, whose
/// options have replaced `values` by now) nothing is checked.
fn check_select(input: &Input, s: &str) -> Result<(), String> {
    if !input.required || input.allow_custom_value {
        return Ok(());
    }
    let Some(values) = &input.values else {
        return Ok(());
    };
    let options = values
        .iter()
        .map(option_value)
        .collect::<Result<Vec<_>, _>>()?;
    if options.iter().any(|o| o == s) {
        return Ok(());
    }
    Err(format!(
        "it must match the values `[{}]`",
        options.join(", ")
    ))
}

/// `ValueOption.from(raw).value()`: a scalar's `toString`, or an object's
/// `value`.
fn option_value(raw: &Value) -> Result<String, String> {
    match raw {
        Value::Object(m) => match m.get("value") {
            Some(v) if !v.is_null() => Ok(java_to_string(v)),
            _ => Err("Select option object must define a `value` field".into()),
        },
        other => Ok(java_to_string(other)),
    }
}

// ---------------------------------------------------------------------------
// INT / FLOAT

/// `Integer.valueOf(toString())`, plus a JSON number with no fractional part
/// (D4). Anything outside 32 bits is refused.
fn to_int(value: &Value) -> Result<i32, String> {
    let fail = || cannot_convert(value, "Integer");
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return i32::try_from(i).map_err(|_| fail());
            }
            let f = n.as_f64().ok_or_else(fail)?;
            if f.is_finite()
                && f.fract() == 0.0
                && f >= f64::from(i32::MIN)
                && f <= f64::from(i32::MAX)
            {
                // Exact: whole and in range.
                #[allow(clippy::cast_possible_truncation)]
                return Ok(f as i32);
            }
            Err(fail())
        }
        Value::String(s) => parse_java_int(s).ok_or_else(fail),
        _ => Err(fail()),
    }
}

/// `Integer.parseInt`: an optional sign, then ASCII digits, within 32 bits.
fn parse_java_int(s: &str) -> Option<i32> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `i32::from_str` takes the same shape (sign, digits) and range.
    s.parse().ok()
}

/// `Float.valueOf(toString())`, finite only (D4).
fn to_float(value: &Value) -> Result<f32, String> {
    let fail = || cannot_convert(value, "Float");
    let text = match value {
        Value::Number(_) | Value::String(_) => java_to_string(value),
        _ => return Err(fail()),
    };
    // `Float.valueOf` trims leading and trailing characters <= ' ', and takes
    // an optional `f`/`F`/`d`/`D` suffix.
    let t = text.trim_matches(|c: char| c <= ' ');
    let t = t.strip_suffix(['f', 'F', 'd', 'D']).unwrap_or(t);
    // Rust also reads `inf`/`nan` spellings Java refuses; neither is finite,
    // so both are refused below either way.
    let looks_decimal = t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'));
    if !looks_decimal {
        return Err(fail());
    }
    match t.parse::<f32>() {
        Ok(x) if x.is_finite() => Ok(x),
        _ => Err(fail()),
    }
}

/// A Java `Float` stored as a JSON number: the shortest decimal that reads
/// back as the same `float`, so `0.1` stays `0.1` and not `0.10000000149`.
fn float_value(x: f32) -> Value {
    let widened: f64 = format!("{x:e}").parse().unwrap_or(f64::from(x));
    serde_json::Number::from_f64(widened).map_or(Value::Null, Value::Number)
}

/// `Float.toString` (JDK 19+: the shortest decimal), e.g. `1.0`, `0.1`,
/// `1.0E10`, `1.0E-4`.
fn java_float(x: f32) -> String {
    java_fp(&format!("{x:e}"))
}

/// `Double.toString`; see [`java_float`].
fn java_double(x: f64) -> String {
    java_fp(&format!("{x:e}"))
}

/// Java's floating-point `toString` from Rust's shortest `{:e}` form
/// (`-1.2345e3`): plain decimal for `1e-3 <= |x| < 1e7`, otherwise
/// `d.dddE<n>`, always with a digit after the point.
fn java_fp(sci: &str) -> String {
    let (neg, sci) = match sci.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, sci),
    };
    let sign = if neg { "-" } else { "" };
    let Some((mantissa, exp)) = sci.split_once('e') else {
        return format!("{sign}{sci}");
    };
    let Ok(exp) = exp.parse::<i32>() else {
        return format!("{sign}{sci}");
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if digits.bytes().all(|b| b == b'0') {
        return format!("{sign}0.0");
    }
    if (-3..7).contains(&exp) {
        if exp >= 0 {
            let int_len = usize::try_from(exp).unwrap_or(0) + 1;
            let mut int_part = digits.chars().take(int_len).collect::<String>();
            while int_part.len() < int_len {
                int_part.push('0');
            }
            let frac: String = digits.chars().skip(int_len).collect();
            let frac = if frac.is_empty() { "0".into() } else { frac };
            format!("{sign}{int_part}.{frac}")
        } else {
            let zeros = "0".repeat(usize::try_from(-exp - 1).unwrap_or(0));
            format!("{sign}0.{zeros}{digits}")
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { "0" } else { rest };
        format!("{sign}{first}.{rest}E{exp}")
    }
}

/// `Object.toString()` of the value Jackson would hand Kestra: a string as
/// itself, a whole number as its digits, any other number as a `Double`, a
/// list as `[a, b]`, a map as `{k=v}`.
fn java_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return i.to_string();
            }
            if let Some(u) = n.as_u64() {
                return u.to_string();
            }
            let text = n.to_string();
            if !text.contains(['.', 'e', 'E']) {
                // A `BigInteger`: its digits.
                return text;
            }
            match n.as_f64() {
                Some(f) if f.is_finite() => java_double(f),
                _ => text,
            }
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(java_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{k}={}", java_to_string(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ---------------------------------------------------------------------------
// DATETIME / DATE / TIME / DURATION

/// Parse `value`'s `toString` with `parse`, or ``Cannot convert value ..``.
fn to_bounded<T>(value: &Value, class: &str, parse: fn(&str) -> Option<T>) -> Result<T, String> {
    parse(&java_to_string(value)).ok_or_else(|| cannot_convert(value, class))
}

/// `after`/`before`, inclusive: `isBefore(after)` and `isAfter(before)` fail.
/// The bound is printed with the type's own `toString`.
fn check_after_before<T: Ord>(
    v: &T,
    after: Option<&str>,
    before: Option<&str>,
    class: &str,
    parse: fn(&str) -> Option<T>,
    format: impl Fn(&T) -> String,
) -> Result<(), String> {
    if let Some(after) = after {
        let a = parse(after).ok_or_else(|| cannot_convert(&Value::from(after), class))?;
        if v < &a {
            return Err(format!("it must be after `{}`", format(&a)));
        }
    }
    if let Some(before) = before {
        let b = parse(before).ok_or_else(|| cannot_convert(&Value::from(before), class))?;
        if v > &b {
            return Err(format!("it must be before `{}`", format(&b)));
        }
    }
    Ok(())
}

/// Nanoseconds from up to nine fraction digits (`5` is 500 ms).
fn fraction_nanos(frac: &str) -> u32 {
    let mut padded = frac.to_string();
    while padded.len() < 9 {
        padded.push('0');
    }
    padded.parse().unwrap_or(0)
}

/// `Instant.parse` (`ISO_INSTANT`): `yyyy-MM-ddTHH:mm:ss[.f{0,9}]` then `Z`
/// or `±HH:MM[:SS]` (JDK 12+ takes an offset and converts it to UTC).
fn parse_instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"\A([0-9]{4}-[0-9]{2}-[0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{0,9}))?(?:(Z)|([+-])([0-9]{2}):([0-9]{2})(?::([0-9]{2}))?)\z",
        )
        .expect("a fixed pattern")
    });
    let c = RE.captures(s)?;
    let date = parse_date(&c[1])?;
    let num = |i: usize| c.get(i).and_then(|m| m.as_str().parse::<u32>().ok());
    let nanos = c.get(5).map_or(0, |m| fraction_nanos(m.as_str()));
    let time = chrono::NaiveTime::from_hms_nano_opt(num(2)?, num(3)?, num(4)?, nanos)?;
    let local = date.and_time(time);
    let offset_secs: i64 = if c.get(6).is_some() {
        0
    } else {
        let (h, m, sec) = (num(8)?, num(9)?, num(10).unwrap_or(0));
        if h > 18 || m > 59 || sec > 59 {
            return None;
        }
        let total = i64::from(h * 3600 + m * 60 + sec);
        if total > 18 * 3600 {
            return None;
        }
        if &c[7] == "-" {
            -total
        } else {
            total
        }
    };
    let utc = local.checked_sub_signed(chrono::TimeDelta::try_seconds(offset_secs)?)?;
    Some(utc.and_utc())
}

/// `Instant.toString`: seconds always, the fraction in groups of three.
fn format_instant(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// `LocalDate.parse` (`ISO_LOCAL_DATE`, strict): `yyyy-MM-dd`, a real day.
fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    let b = s.as_bytes();
    let shape = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shape {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(
        s[0..4].parse().ok()?,
        s[5..7].parse().ok()?,
        s[8..10].parse().ok()?,
    )
}

/// `LocalTime.parse` (`ISO_LOCAL_TIME`): `HH:mm[:ss[.f{0,9}]]`.
fn parse_time(s: &str) -> Option<chrono::NaiveTime> {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\A([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]{0,9}))?)?\z")
            .expect("a fixed pattern")
    });
    let c = RE.captures(s)?;
    let num = |i: usize| c.get(i).map_or(Some(0), |m| m.as_str().parse::<u32>().ok());
    let nanos = c.get(4).map_or(0, |m| fraction_nanos(m.as_str()));
    let (h, m, sec) = (num(1)?, num(2)?, num(3)?);
    if sec > 59 {
        return None;
    }
    chrono::NaiveTime::from_hms_nano_opt(h, m, sec, nanos)
}

/// `LocalTime.toString`: `HH:mm`, then `:ss` when seconds or nanos are set,
/// then the fraction in groups of three.
fn format_time(t: &chrono::NaiveTime) -> String {
    use chrono::Timelike;
    let mut out = format!("{:02}:{:02}", t.hour(), t.minute());
    let (sec, nano) = (t.second(), t.nanosecond());
    if sec > 0 || nano > 0 {
        out.push_str(&format!(":{sec:02}"));
        if nano > 0 {
            if nano % 1_000_000 == 0 {
                out.push_str(&format!(".{:03}", nano / 1_000_000));
            } else if nano % 1_000 == 0 {
                out.push_str(&format!(".{:06}", nano / 1_000));
            } else {
                out.push_str(&format!(".{nano:09}"));
            }
        }
    }
    out
}

/// `java.time.Duration`: seconds (floor) and a non-negative nano part, as
/// Java holds it, so the order and `toString` are Java's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct JavaDuration {
    seconds: i64,
    nanos: u32,
}

impl JavaDuration {
    /// `Duration.parse`: `[-+]PnDTnHnMn.nS`, each part optionally signed,
    /// case-insensitive (ASCII), at least one part, never a bare `T`, the
    /// total within a `long` of seconds.
    fn parse(s: &str) -> Option<Self> {
        static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(
                r"\A([-+]?)[pP](?:([-+]?[0-9]+)[dD])?([tT](?:([-+]?[0-9]+)[hH])?(?:([-+]?[0-9]+)[mM])?(?:([-+]?[0-9]+)(?:[.,]([0-9]{0,9}))?[sS])?)?\z",
            )
            .expect("a fixed pattern")
        });
        let c = RE.captures(s)?;
        if c.get(2).is_none() && c.get(4).is_none() && c.get(5).is_none() && c.get(6).is_none() {
            return None;
        }
        if c.get(3).is_some_and(|t| t.as_str().len() == 1) {
            return None;
        }
        let part = |i: usize, per: i128| -> Option<i128> {
            match c.get(i) {
                None => Some(0),
                Some(m) => Some(i128::from(m.as_str().parse::<i64>().ok()?) * per),
            }
        };
        const NS: i128 = 1_000_000_000;
        let mut total =
            part(2, 86_400 * NS)? + part(4, 3_600 * NS)? + part(5, 60 * NS)? + part(6, NS)?;
        if let Some(frac) = c.get(7) {
            let f = i128::from(fraction_nanos(frac.as_str()));
            // The fraction takes the seconds' sign (`parseFraction`).
            if c.get(6).is_some_and(|m| m.as_str().starts_with('-')) {
                total -= f;
            } else {
                total += f;
            }
        }
        if &c[1] == "-" {
            total = -total;
        }
        let seconds = i64::try_from(total.div_euclid(NS)).ok()?;
        let nanos = u32::try_from(total.rem_euclid(NS)).ok()?;
        Some(Self { seconds, nanos })
    }
}

/// `Duration.toString`, e.g. `PT24H` for `P1D`, `PT0.5S`, `PT-0.5S`.
impl std::fmt::Display for JavaDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (seconds, nanos) = (self.seconds, self.nanos);
        if seconds == 0 && nanos == 0 {
            return f.write_str("PT0S");
        }
        let effective = if seconds < 0 && nanos > 0 {
            seconds + 1
        } else {
            seconds
        };
        let hours = effective / 3600;
        let minutes = (effective % 3600) / 60;
        let secs = effective % 60;
        let mut buf = String::from("PT");
        if hours != 0 {
            buf.push_str(&format!("{hours}H"));
        }
        if minutes != 0 {
            buf.push_str(&format!("{minutes}M"));
        }
        if secs == 0 && nanos == 0 && buf.len() > 2 {
            return f.write_str(&buf);
        }
        if seconds < 0 && nanos > 0 && secs == 0 {
            buf.push_str("-0");
        } else {
            buf.push_str(&secs.to_string());
        }
        if nanos > 0 {
            let frac = if seconds < 0 {
                2_000_000_000 - nanos
            } else {
                nanos + 1_000_000_000
            };
            let frac = frac.to_string();
            // Drop the leading `1`, keep nine digits, trim trailing zeros.
            let digits = frac[1..].trim_end_matches('0');
            buf.push('.');
            buf.push_str(digits);
        }
        buf.push('S');
        f.write_str(&buf)
    }
}

// ---------------------------------------------------------------------------
// URI

/// `java.net.URI.create(s)` then "has a scheme" (`parseType`'s URI arm).
///
/// The syntax errors cover the common shapes of `URISyntaxException`'s
/// message -- a bad scheme, an empty scheme-specific part, an illegal
/// character or a malformed `%` escape -- not every one Java can produce.
fn check_uri(s: &str) -> Result<(), String> {
    // A scheme: a `:` before any of `/?#`, at a position > 0.
    let first_delim = s.find(['/', '?', '#']).unwrap_or(s.len());
    let colon = s[..first_delim].find(':');
    let scheme_end = match colon {
        Some(0) => return Err(format!("Expected scheme name at index 0: {s}")),
        Some(i) => {
            for (j, ch) in s[..i].char_indices() {
                let ok = if j == 0 {
                    ch.is_ascii_alphabetic()
                } else {
                    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')
                };
                if !ok {
                    return Err(format!(
                        "Illegal character in scheme name at index {j}: {s}"
                    ));
                }
            }
            if i + 1 == s.len() {
                return Err(format!(
                    "Expected scheme-specific part at index {}: {s}",
                    i + 1
                ));
            }
            Some(i + 1)
        }
        None => None,
    };
    let start = scheme_end.unwrap_or(0);
    let rest = &s[start..];
    let hierarchical = scheme_end.is_none() || rest.starts_with('/');
    let mut part = if !hierarchical {
        "opaque part"
    } else if rest.starts_with("//") {
        "authority"
    } else {
        "path"
    };
    let bytes = s.as_bytes();
    let mut i = start;
    for (off, ch) in rest.char_indices() {
        let idx = start + off;
        if idx < i {
            continue;
        }
        match ch {
            '?' if hierarchical && part != "query" && part != "fragment" => part = "query",
            '#' if part != "fragment" => part = "fragment",
            '/' if part == "authority" && idx > start + 1 => part = "path",
            '%' => {
                let hex = |k: usize| bytes.get(k).is_some_and(u8::is_ascii_hexdigit);
                if !(hex(idx + 1) && hex(idx + 2)) {
                    return Err(format!("Malformed escape pair at index {idx}: {s}"));
                }
                i = idx + 3;
                continue;
            }
            c if c.is_ascii_control()
                || matches!(
                    c,
                    ' ' | '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}'
                ) =>
            {
                return Err(format!("Illegal character in {part} at index {idx}: {s}"));
            }
            _ => {}
        }
        i = idx + ch.len_utf8();
    }
    if scheme_end.is_none() {
        return Err("Invalid URI format.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    /// The renderer of a test with no template in it.
    fn no_render(t: &str, _: &Map<String, Value>) -> Result<Value, String> {
        Err(format!("no renderer in this test: {t}"))
    }

    /// An input from YAML, the way a flow declares it.
    fn input(yaml: &str) -> Input {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    /// One input checked alone: the stored value, or the message.
    fn one(yaml: &str, posted: Value) -> Result<Option<Value>, String> {
        let i = input(yaml);
        let id = i.id.clone();
        resolve(
            std::slice::from_ref(&i),
            &json!({ id.clone(): posted }),
            &no_render,
        )
        .map(|m| m.get(&id).cloned())
        .map_err(|e| {
            assert_eq!(e.input_id, id);
            e.message
        })
    }

    /// Table runner: `(posted, expected)` where `expected` is `Ok(stored)` or
    /// `Err(cause)`; a cause is checked in Kestra's prefixed form.
    fn table(yaml: &str, cases: &[(Value, Result<Value, &str>)]) {
        let id = input(yaml).id;
        for (posted, want) in cases {
            let got = one(yaml, posted.clone());
            match want {
                Ok(v) => assert_eq!(got, Ok(Some(v.clone())), "{yaml} / {posted}"),
                Err(cause) => assert_eq!(
                    got,
                    Err(format!("Invalid value for input `{id}`. Cause: {cause}")),
                    "{yaml} / {posted}"
                ),
            }
        }
    }

    fn missing(yaml: &str, posted: Value) {
        let id = input(yaml).id;
        assert_eq!(
            one(yaml, posted),
            Err(format!("Missing required input:{id}"))
        );
    }

    fn absent(yaml: &str, posted: Value) {
        assert_eq!(one(yaml, posted), Ok(None), "{yaml}");
    }

    // --- required, null, empty string, undeclared keys, order -------------

    #[test]
    fn required_defaults_to_true_and_null_or_absent_fails() {
        let y = "id: n\ntype: INT\n";
        missing(y, Value::Null);
        let i = input(y);
        let err = resolve(&[i], &json!({}), &no_render).unwrap_err();
        assert_eq!(err.message, "Missing required input:n");
        assert_eq!(err.input_id, "n");
    }

    #[test]
    fn not_required_absent_is_left_out() {
        absent("id: n\ntype: INT\nrequired: false\n", Value::Null);
        absent("id: n\ntype: INT\nrequired: false\n", json!(""));
    }

    #[test]
    fn empty_string_is_absent_except_for_the_text_types() {
        for t in [
            "INT", "FLOAT", "BOOL", "DATETIME", "DATE", "TIME", "DURATION", "URI",
        ] {
            missing(&format!("id: x\ntype: {t}\n"), json!(""));
        }
        for t in ["STRING", "SELECT", "EMAIL", "SECRET"] {
            let y = format!("id: x\ntype: {t}\n");
            assert_eq!(one(&y, json!("")), Ok(Some(json!(""))), "{t}");
        }
    }

    #[test]
    fn undeclared_keys_are_dropped_and_order_decides_the_first_error() {
        let inputs = [
            input("id: a\ntype: INT\n"),
            input("id: b\ntype: INT\nmin: 5\n"),
            input("id: c\ntype: INT\n"),
        ];
        let ok = resolve(
            &inputs,
            &json!({"a": "1", "b": 7, "c": 3, "zz": 1}),
            &no_render,
        )
        .unwrap();
        assert_eq!(Value::Object(ok), json!({"a": 1, "b": 7, "c": 3}));

        // b and c are both bad: b is reported, it is declared first.
        let err = resolve(&inputs, &json!({"a": 1, "b": 1}), &no_render).unwrap_err();
        assert_eq!(err.input_id, "b");
        assert_eq!(
            err.message,
            "Invalid value for input `b`. Cause: it must be more than `5`"
        );
    }

    #[test]
    fn a_posted_value_that_is_not_an_object_is_empty() {
        let i = input("id: n\ntype: INT\nrequired: false\n");
        assert_eq!(
            resolve(&[i], &json!([1, 2]), &no_render).unwrap(),
            Map::new()
        );
    }

    #[test]
    fn a_literal_default_is_validated_but_not_stored() {
        absent("id: n\ntype: INT\ndefaults: 5\n", Value::Null);
        absent("id: n\ntype: INT\ndefaults: 5\n", json!(""));
        assert_eq!(
            one("id: n\ntype: INT\nmin: 10\ndefaults: 5\n", Value::Null),
            Err("Invalid value for input `n`. Cause: it must be more than `10`".into())
        );
        // Templated: not checked at start (D2).
        absent("id: n\ntype: INT\ndefaults: \"{{ 1 + 1 }}\"\n", Value::Null);
    }

    // --- STRING / SECRET -----------------------------------------------------

    #[test]
    fn string() {
        table(
            "id: s\ntype: STRING\n",
            &[
                (json!("abc"), Ok(json!("abc"))),
                (json!(5), Ok(json!("5"))),
                (json!(1.5), Ok(json!("1.5"))),
                (json!(true), Ok(json!("true"))),
                (json!({"a": 1}), Ok(json!("{a=1}"))),
                (json!([1, "x"]), Ok(json!("[1, x]"))),
            ],
        );
        missing("id: s\ntype: STRING\n", Value::Null);
    }

    #[test]
    fn string_validator_is_a_full_match() {
        let y = "id: s\ntype: STRING\nvalidator: A\\d+\n";
        table(
            y,
            &[
                (json!("A12"), Ok(json!("A12"))),
                (json!("A12x"), Err("it must match the pattern `A\\d+`")),
                (json!("xA12"), Err("it must match the pattern `A\\d+`")),
                (json!(""), Err("it must match the pattern `A\\d+`")),
            ],
        );
        // Alternation must backtrack into the longer branch, as `matches()` does.
        table(
            "id: s\ntype: STRING\nvalidator: a|ab\n",
            &[(json!("ab"), Ok(json!("ab")))],
        );
        // Lookaround (fancy-regex), the survey's SECRET pattern.
        table(
            "id: s\ntype: STRING\nvalidator: \"(?=.{8,})(?=.*[A-Z])(?=.*[0-9]).*\"\n",
            &[
                (json!("Abcdefg1"), Ok(json!("Abcdefg1"))),
                (
                    json!("short"),
                    Err("it must match the pattern `(?=.{8,})(?=.*[A-Z])(?=.*[0-9]).*`"),
                ),
            ],
        );
        // A number posted to a validated STRING is matched as its text.
        table(y, &[(json!(5), Err("it must match the pattern `A\\d+`"))]);
    }

    #[test]
    fn validator_backtrack_limit_gives_kestras_timeout_text() {
        // A backreference forces fancy-regex's backtracking VM.
        let pattern = r"(a*)*\1b";
        let y = format!("id: s\ntype: STRING\nvalidator: '{pattern}'\n");
        table(
            &y,
            &[(
                json!("a".repeat(40)),
                Err("the validator pattern `(a*)*\\1b` timed out — it may be too complex"),
            )],
        );
    }

    #[test]
    fn validator_that_does_not_compile() {
        table(
            "id: s\ntype: STRING\nvalidator: '('\n",
            &[(json!("x"), Err("invalid pattern [(]"))],
        );
    }

    #[test]
    fn secret_validator_sees_the_clear_value_and_prefixes_the_id() {
        let y = "id: input1\ntype: SECRET\nvalidator: \"(?=.{8,})(?=.*[A-Z])(?=.*[0-9]).*\"\n";
        assert_eq!(
            one(y, json!("weak")),
            Err(
                "Invalid value for input `input1`. Cause: input1: it must match the pattern \
                 `(?=.{8,})(?=.*[A-Z])(?=.*[0-9]).*`"
                    .into()
            )
        );
        assert_eq!(one(y, json!("Str0ngPass")), Ok(Some(json!("Str0ngPass"))));
        assert_eq!(
            one("id: k\ntype: SECRET\n", json!(42)),
            Ok(Some(json!("42")))
        );
        missing("id: k\ntype: SECRET\n", Value::Null);
    }

    // --- INT -----------------------------------------------------------------

    #[test]
    fn int() {
        table(
            "id: n\ntype: INT\nmin: 10\nmax: 20\n",
            &[
                (json!(10), Ok(json!(10))),
                (json!(20), Ok(json!(20))),
                (json!("15"), Ok(json!(15))),
                (json!("+15"), Ok(json!(15))),
                (json!(15.0), Ok(json!(15))),
                (json!(9), Err("it must be more than `10`")),
                (json!(21), Err("it must be less than `20`")),
                (json!("9"), Err("it must be more than `10`")),
            ],
        );
        table(
            "id: n\ntype: INT\n",
            &[
                (json!(-2147483648_i64), Ok(json!(-2147483648_i64))),
                (json!(2147483647), Ok(json!(2147483647))),
                (
                    json!(2147483648_i64),
                    Err("Cannot convert value '2147483648' to Integer"),
                ),
                (
                    json!("2147483648"),
                    Err("Cannot convert value '2147483648' to Integer"),
                ),
                (json!("1.0"), Err("Cannot convert value '1.0' to Integer")),
                (json!(1.5), Err("Cannot convert value '1.5' to Integer")),
                (json!("abc"), Err("Cannot convert value 'abc' to Integer")),
                (json!(" 1"), Err("Cannot convert value ' 1' to Integer")),
                (json!(true), Err("Cannot convert value 'true' to Integer")),
                (json!([1]), Err("Cannot convert value '[1]' to Integer")),
            ],
        );
    }

    // --- FLOAT ---------------------------------------------------------------

    #[test]
    fn float() {
        table(
            "id: f\ntype: FLOAT\nmin: 0.1\nmax: 0.5\n",
            &[
                (json!(0.1), Ok(json!(0.1))),
                (json!(0.5), Ok(json!(0.5))),
                (json!("0.25"), Ok(json!(0.25))),
                (json!(0.05), Err("it must be more than `0.1`")),
                (json!("0.6"), Err("it must be less than `0.5`")),
            ],
        );
        // An integral bound prints as Java's `Float.toString`: `1.0`.
        table(
            "id: f\ntype: FLOAT\nmin: 1\nmax: 10000000\n",
            &[
                (json!(0), Err("it must be more than `1.0`")),
                (json!(2e7), Err("it must be less than `1.0E7`")),
                (json!(1), Ok(json!(1.0))),
                (json!("1e3"), Ok(json!(1000.0))),
                (json!("2.5f"), Ok(json!(2.5))),
            ],
        );
        table(
            "id: f\ntype: FLOAT\n",
            &[
                (json!("NaN"), Err("Cannot convert value 'NaN' to Float")),
                (
                    json!("Infinity"),
                    Err("Cannot convert value 'Infinity' to Float"),
                ),
                (json!("inf"), Err("Cannot convert value 'inf' to Float")),
                (json!("1e40"), Err("Cannot convert value '1e40' to Float")),
                (json!("abc"), Err("Cannot convert value 'abc' to Float")),
                (json!(true), Err("Cannot convert value 'true' to Float")),
            ],
        );
    }

    #[test]
    fn java_float_to_string() {
        for (x, s) in [
            (1.0_f32, "1.0"),
            (0.1, "0.1"),
            (0.001, "0.001"),
            (0.0001, "1.0E-4"),
            (1234567.0, "1234567.0"),
            (1.0e7, "1.0E7"),
            (1.5e10, "1.5E10"),
            (-2.5, "-2.5"),
            (0.0, "0.0"),
            (100.0, "100.0"),
        ] {
            assert_eq!(java_float(x), s, "{x}");
        }
    }

    // --- BOOL ----------------------------------------------------------------

    #[test]
    fn bool_is_strict() {
        table(
            "id: b\ntype: BOOL\n",
            &[
                (json!(true), Ok(json!(true))),
                (json!(false), Ok(json!(false))),
                (json!("TRUE"), Ok(json!(true))),
                (json!("False"), Ok(json!(false))),
                (json!("yes"), Err("Cannot convert value 'yes' to Boolean")),
                (json!(1), Err("Cannot convert value '1' to Boolean")),
                (json!("1"), Err("Cannot convert value '1' to Boolean")),
            ],
        );
        missing("id: b\ntype: BOOL\n", Value::Null);
    }

    // --- DATETIME / DATE / TIME / DURATION ----------------------------------

    #[test]
    fn datetime() {
        table(
            "id: t\ntype: DATETIME\nafter: \"2023-01-01T00:00:00Z\"\nbefore: \"2023-12-31T23:59:59Z\"\n",
            &[
                (json!("2023-06-01T10:00:00Z"), Ok(json!("2023-06-01T10:00:00Z"))),
                (json!("2023-01-01T00:00:00Z"), Ok(json!("2023-01-01T00:00:00Z"))),
                (
                    json!("2023-06-01T12:00:00.5+02:00"),
                    Ok(json!("2023-06-01T12:00:00.5+02:00")),
                ),
                (
                    json!("2022-12-31T23:59:59Z"),
                    Err("it must be after `2023-01-01T00:00:00Z`"),
                ),
                (
                    json!("2024-01-01T00:00:00Z"),
                    Err("it must be before `2023-12-31T23:59:59Z`"),
                ),
            ],
        );
        table(
            "id: t\ntype: DATETIME\n",
            &[
                (
                    json!("2023-06-01"),
                    Err("Cannot convert value '2023-06-01' to Instant"),
                ),
                (
                    json!("2023-06-01T10:00Z"),
                    Err("Cannot convert value '2023-06-01T10:00Z' to Instant"),
                ),
                (
                    json!("2023-06-01 10:00:00Z"),
                    Err("Cannot convert value '2023-06-01 10:00:00Z' to Instant"),
                ),
                (
                    json!("2023-02-30T10:00:00Z"),
                    Err("Cannot convert value '2023-02-30T10:00:00Z' to Instant"),
                ),
                (json!(5), Err("Cannot convert value '5' to Instant")),
            ],
        );
    }

    #[test]
    fn date() {
        table(
            "id: d\ntype: DATE\nafter: 2023-01-01\nbefore: 2023-12-31\n",
            &[
                (json!("2023-06-01"), Ok(json!("2023-06-01"))),
                (json!("2023-12-31"), Ok(json!("2023-12-31"))),
                (json!("2022-12-31"), Err("it must be after `2023-01-01`")),
                (json!("2024-01-01"), Err("it must be before `2023-12-31`")),
                (
                    json!("2023-6-1"),
                    Err("Cannot convert value '2023-6-1' to LocalDate"),
                ),
                (
                    json!("2023-02-29"),
                    Err("Cannot convert value '2023-02-29' to LocalDate"),
                ),
                (
                    json!("2023-06-01T00:00:00Z"),
                    Err("Cannot convert value '2023-06-01T00:00:00Z' to LocalDate"),
                ),
            ],
        );
    }

    #[test]
    fn time() {
        table(
            "id: t\ntype: TIME\nafter: \"01:00:00\"\nbefore: \"11:59:59\"\n",
            &[
                (json!("10:00"), Ok(json!("10:00"))),
                (json!("10:00:00"), Ok(json!("10:00:00"))),
                (json!("10:00:01.5"), Ok(json!("10:00:01.5"))),
                (json!("00:59:59"), Err("it must be after `01:00`")),
                (json!("12:00"), Err("it must be before `11:59:59`")),
                (
                    json!("25:00"),
                    Err("Cannot convert value '25:00' to LocalTime"),
                ),
                (
                    json!("1:00"),
                    Err("Cannot convert value '1:00' to LocalTime"),
                ),
            ],
        );
    }

    #[test]
    fn duration() {
        table(
            "id: d\ntype: DURATION\nmin: PT10S\nmax: PT20S\n",
            &[
                (json!("PT15S"), Ok(json!("PT15S"))),
                (json!("PT10S"), Ok(json!("PT10S"))),
                (json!("pt15s"), Ok(json!("pt15s"))),
                (json!("PT1S"), Err("It must be more than `PT10S`")),
                (json!("PT30S"), Err("It must be less than `PT20S`")),
            ],
        );
        table(
            "id: d\ntype: DURATION\n",
            &[
                (json!("P1D"), Ok(json!("P1D"))),
                (json!("PT90M"), Ok(json!("PT90M"))),
                (json!("PT0.5S"), Ok(json!("PT0.5S"))),
                (json!("-PT0.5S"), Ok(json!("-PT0.5S"))),
                (json!("PT-1.5S"), Ok(json!("PT-1.5S"))),
                (json!("PT0S"), Ok(json!("PT0S"))),
                (json!("P1M"), Err("Cannot convert value 'P1M' to Duration")),
                (json!("PT"), Err("Cannot convert value 'PT' to Duration")),
                (json!("P"), Err("Cannot convert value 'P' to Duration")),
                (json!("1H"), Err("Cannot convert value '1H' to Duration")),
                (json!(60), Err("Cannot convert value '60' to Duration")),
            ],
        );
        // A bound is printed as Java's `Duration.toString`.
        table(
            "id: d\ntype: DURATION\nmin: P1D\n",
            &[(json!("PT1H"), Err("It must be more than `PT24H`"))],
        );
    }

    // --- URI / EMAIL ---------------------------------------------------------

    #[test]
    fn uri() {
        table(
            "id: u\ntype: URI\n",
            &[
                (
                    json!("https://kestra.io/a?b=c#d"),
                    Ok(json!("https://kestra.io/a?b=c#d")),
                ),
                (json!("mailto:a@b.c"), Ok(json!("mailto:a@b.c"))),
                (json!("kestra://x/y%20z"), Ok(json!("kestra://x/y%20z"))),
                (json!("no-scheme/path"), Err("Invalid URI format.")),
                (json!("/abs/path"), Err("Invalid URI format.")),
                (
                    json!("http://a b"),
                    Err("Illegal character in authority at index 8: http://a b"),
                ),
                (
                    json!("a b"),
                    Err("Illegal character in path at index 1: a b"),
                ),
                (
                    json!("http://x/a%2"),
                    Err("Malformed escape pair at index 10: http://x/a%2"),
                ),
                (json!(":x"), Err("Expected scheme name at index 0: :x")),
                (
                    json!("http:"),
                    Err("Expected scheme-specific part at index 5: http:"),
                ),
                (
                    json!("a_b:c"),
                    Err("Illegal character in scheme name at index 1: a_b:c"),
                ),
            ],
        );
    }

    #[test]
    fn email() {
        table(
            "id: e\ntype: EMAIL\n",
            &[
                (json!("a.b+c@example.com"), Ok(json!("a.b+c@example.com"))),
                (json!("o’neil@x.io"), Ok(json!("o’neil@x.io"))),
                (json!(""), Ok(json!(""))),
                (
                    json!("not-an-email"),
                    Err("The input must be a valid email"),
                ),
                (json!("a@b@c"), Err("The input must be a valid email")),
                (json!("a@b.c\n"), Err("The input must be a valid email")),
            ],
        );
        missing("id: e\ntype: EMAIL\n", Value::Null);
    }

    // --- SELECT --------------------------------------------------------------

    #[test]
    fn select() {
        let y = "id: s\ntype: SELECT\nvalues: [a, b]\n";
        table(
            y,
            &[
                (json!("a"), Ok(json!("a"))),
                (json!("c"), Err("it must match the values `[a, b]`")),
                (json!(""), Err("it must match the values `[a, b]`")),
            ],
        );
        // Not required: any value (Kestra's quirk, D4).
        table(
            "id: s\ntype: SELECT\nrequired: false\nvalues: [a, b]\n",
            &[(json!("c"), Ok(json!("c")))],
        );
        // allowCustomValue.
        table(
            "id: s\ntype: SELECT\nallowCustomValue: true\nvalues: [a, b]\n",
            &[(json!("c"), Ok(json!("c")))],
        );
        // {label, value} options compare and print by value; scalars by text.
        table(
            "id: s\ntype: SELECT\nvalues:\n  - {label: Production, value: \"123\"}\n  - 7\n",
            &[
                (json!("123"), Ok(json!("123"))),
                (json!(7), Ok(json!("7"))),
                (
                    json!("Production"),
                    Err("it must match the values `[123, 7]`"),
                ),
            ],
        );
        table(
            "id: s\ntype: SELECT\nvalues:\n  - {label: Production}\n",
            &[(
                json!("x"),
                Err("Select option object must define a `value` field"),
            )],
        );
    }

    // --- FILE / FORM (checked elsewhere / not supported) ---------------------

    #[test]
    fn file_and_form_pass_through() {
        let v = json!("kestra:///x/y.txt");
        assert_eq!(one("id: x\ntype: FILE\n", v.clone()), Ok(Some(v)));
        // FILE is still required (Kestra checks `required` for every type).
        missing("id: x\ntype: FILE\n", Value::Null);
        // FORM / REUSABLE_INPUTS: never required here.
        absent("id: x\ntype: FORM\n", Value::Null);
    }

    // --- ARRAY / MULTISELECT -------------------------------------------------

    /// `serde_json`'s message for `s`, the cause a bad JSON text gives.
    fn json_error(s: &str) -> String {
        parse_json(s).unwrap_err()
    }

    #[test]
    fn array_parses_each_element_with_item_type() {
        let y = "id: a\ntype: ARRAY\nitemType: INT\n";
        let bad_json = json_error("abc");
        table(
            y,
            &[
                (json!([1, "2"]), Ok(json!([1, 2]))),
                (json!("[1, 2]"), Ok(json!([1, 2]))),
                (json!([]), Ok(json!([]))),
                (
                    json!([1, "x"]),
                    Err("Unable to parse array element as `INT` on `x`"),
                ),
                (
                    json!("[1, \"x\"]"),
                    Err("Unable to parse array element as `INT` on `x`"),
                ),
                (
                    json!([null]),
                    Err("Unable to parse array element as `INT` on `null`"),
                ),
                (
                    json!([1.5]),
                    Err("Unable to parse array element as `INT` on `1.5`"),
                ),
                (json!("abc"), Err(bad_json.as_str())),
                (json!(5), Err("Cannot convert value '5' to List")),
                (json!({"a": 1}), Err("Cannot convert value '{a=1}' to List")),
                (
                    json!("{\"a\":1}"),
                    Err("Cannot convert value '{\"a\":1}' to List"),
                ),
            ],
        );
        missing(y, json!(""));
        missing(y, Value::Null);
    }

    #[test]
    fn array_item_types_store_what_their_scalar_stores() {
        table(
            "id: a\ntype: ARRAY\nitemType: BOOL\n",
            &[
                (json!(["TRUE", false]), Ok(json!([true, false]))),
                (
                    json!(["yes"]),
                    Err("Unable to parse array element as `BOOL` on `yes`"),
                ),
            ],
        );
        // A date element is checked and kept as posted.
        table(
            "id: a\ntype: ARRAY\nitemType: DATETIME\n",
            &[
                (
                    json!(["2023-01-01T00:00:00+01:00"]),
                    Ok(json!(["2023-01-01T00:00:00+01:00"])),
                ),
                (
                    json!(["2023-01-01"]),
                    Err("Unable to parse array element as `DATETIME` on `2023-01-01`"),
                ),
            ],
        );
        // STRING elements: a number's text, a map's Java text.
        table(
            "id: a\ntype: ARRAY\nitemType: STRING\n",
            &[(json!([1, {"k": 2}]), Ok(json!(["1", "{k=2}"])))],
        );
        // An element is parsed, not validated: an EMAIL element is not
        // matched against the email pattern (Kestra's parseType).
        table(
            "id: a\ntype: ARRAY\nitemType: EMAIL\n",
            &[(json!(["nope"]), Ok(json!(["nope"])))],
        );
        // No itemType: the list passes through.
        table(
            "id: a\ntype: ARRAY\n",
            &[(json!([1, "a", null]), Ok(json!([1, "a", null])))],
        );
    }

    #[test]
    fn multiselect_checks_every_element_against_the_values() {
        let y = "id: multi\ntype: MULTISELECT\nvalues: [A, B, C]\n";
        table(
            y,
            &[
                (json!(["A", "C"]), Ok(json!(["A", "C"]))),
                (json!("[\"B\"]"), Ok(json!(["B"]))),
                (json!([]), Ok(json!([]))),
            ],
        );
        // One line per distinct bad element, in element order.
        assert_eq!(
            one(y, json!(["D", "A", "E", "D"])),
            Err(
                "Invalid value for input `multi`. Cause: value `D` doesn't match the values \
                 `[A, B, C]`\n\
                 Invalid value for input `multi`. Cause: value `E` doesn't match the values \
                 `[A, B, C]`"
                    .into()
            )
        );
        // Checked even when not required (unlike SELECT).
        table(
            "id: m\ntype: MULTISELECT\nrequired: false\nvalues: [A]\n",
            &[(
                json!(["Z"]),
                Err("value `Z` doesn't match the values `[A]`"),
            )],
        );
        // allowCustomValue.
        table(
            "id: m\ntype: MULTISELECT\nallowCustomValue: true\nvalues: [A]\n",
            &[(json!(["Z"]), Ok(json!(["Z"])))],
        );
        // Default itemType STRING: a number element becomes its text.
        table(
            "id: m\ntype: MULTISELECT\nvalues: [\"1\", A]\n",
            &[(json!([1]), Ok(json!(["1"])))],
        );
        // itemType INT: parsed, then compared by text (divergence, module doc).
        table(
            "id: m\ntype: MULTISELECT\nitemType: INT\nvalues: [\"1\", \"2\"]\n",
            &[
                (json!(["2"]), Ok(json!([2]))),
                (
                    json!(["x"]),
                    Err("Unable to parse array element as `INT` on `x`"),
                ),
            ],
        );
        // {label, value} options.
        table(
            "id: m\ntype: MULTISELECT\nvalues:\n  - {label: Prod, value: p}\n",
            &[
                (json!(["p"]), Ok(json!(["p"]))),
                (
                    json!(["Prod"]),
                    Err("value `Prod` doesn't match the values `[p]`"),
                ),
            ],
        );
        missing(y, json!(""));
    }

    #[test]
    fn a_literal_list_default_is_validated() {
        // A YAML list default is held as its JSON text and parsed back.
        absent(
            "id: m\ntype: MULTISELECT\nvalues: [A, B]\ndefaults: [A]\n",
            Value::Null,
        );
        assert_eq!(
            one(
                "id: m\ntype: MULTISELECT\nvalues: [A, B]\ndefaults: [Z]\n",
                Value::Null
            ),
            Err(
                "Invalid value for input `m`. Cause: value `Z` doesn't match the values `[A, B]`"
                    .into()
            )
        );
        absent(
            "id: a\ntype: ARRAY\nitemType: INT\ndefaults: [1, 2]\n",
            Value::Null,
        );
    }

    // --- JSON / YAML / ION ---------------------------------------------------

    #[test]
    fn json_parses_a_string_and_passes_structures_through() {
        let bad = json_error("{a");
        table(
            "id: j\ntype: JSON\n",
            &[
                (json!("{\"a\": 1}"), Ok(json!({"a": 1}))),
                (json!("[1, 2]"), Ok(json!([1, 2]))),
                (json!("\"s\""), Ok(json!("s"))),
                (json!({"a": [1]}), Ok(json!({"a": [1]}))),
                (json!([1]), Ok(json!([1]))),
                (json!(5), Ok(json!(5))),
                (json!(true), Ok(json!(true))),
                // Jackson reads one value and leaves the rest.
                (json!("1 2"), Ok(json!(1))),
                (json!("{a"), Err(bad.as_str())),
            ],
        );
        missing("id: j\ntype: JSON\n", json!(""));
    }

    #[test]
    fn yaml_parses_a_string_and_a_scalar_stays_a_scalar() {
        let bad = serde_norway::from_str::<Value>("a: [")
            .unwrap_err()
            .to_string();
        table(
            "id: y\ntype: YAML\n",
            &[
                (
                    json!("a: 1\nb: [x, y]\n"),
                    Ok(json!({"a": 1, "b": ["x", "y"]})),
                ),
                (json!("hello"), Ok(json!("hello"))),
                (json!("- 1\n- 2\n"), Ok(json!([1, 2]))),
                (json!({"a": 1}), Ok(json!({"a": 1}))),
                (json!("a: ["), Err(bad.as_str())),
            ],
        );
    }

    #[test]
    fn ion_reads_the_first_value() {
        table(
            "id: i\ntype: ION\n",
            &[
                (json!("{a: 1, b: \"x\"}"), Ok(json!({"a": 1, "b": "x"}))),
                (json!("[1, 2]"), Ok(json!([1, 2]))),
                (json!("1 2"), Ok(json!(1))),
                (json!({"a": 1}), Ok(json!({"a": 1}))),
                (json!("   "), Err("No content to map due to end-of-input")),
            ],
        );
        // A syntax error is the reader's message, prefixed.
        let err = one("id: i\ntype: ION\n", json!("{a:")).unwrap_err();
        assert!(
            err.starts_with("Invalid value for input `i`. Cause: ") && err.len() > 40,
            "{err}"
        );
    }

    #[test]
    fn json_schema_is_not_checked() {
        table(
            "id: j\ntype: JSON\njsonSchema: '{\"type\": \"object\"}'\n",
            &[(json!("[1]"), Ok(json!([1])))],
        );
    }

    // --- dependsOn -----------------------------------------------------------

    fn inputs(yaml: &str) -> Vec<Input> {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    const A_THEN_B: &str = "
- id: a
  type: STRING
  required: false
- id: b
  type: INT
  dependsOn:
    inputs: [a]
";

    #[test]
    fn a_disabled_input_is_dropped_and_skips_required() {
        let i = inputs(A_THEN_B);
        // a is null: b is disabled, its required is skipped, its value dropped.
        assert_eq!(resolve(&i, &json!({}), &no_render), Ok(Map::new()));
        assert_eq!(
            Value::Object(resolve(&i, &json!({"b": 5}), &no_render).unwrap()),
            json!({})
        );
        // a is set: b is enabled and required.
        assert_eq!(
            resolve(&i, &json!({"a": "x"}), &no_render)
                .unwrap_err()
                .message,
            "Missing required input:b"
        );
        assert_eq!(
            Value::Object(resolve(&i, &json!({"a": "x", "b": "5"}), &no_render).unwrap()),
            json!({"a": "x", "b": 5})
        );
        let all = resolve_all(&i, &json!({}), &no_render);
        assert!(all[0].enabled && !all[1].enabled, "{all:?}");
        assert!(all[1].errors.is_empty(), "{all:?}");
    }

    #[test]
    fn dependencies_resolve_first_whatever_the_declared_order() {
        // b is declared before a, the input it depends on.
        let i = inputs(
            "
- id: b
  type: INT
  dependsOn:
    inputs: [a]
- id: a
  type: STRING
  required: false
",
        );
        assert_eq!(resolve(&i, &json!({}), &no_render), Ok(Map::new()));
        assert_eq!(
            resolve(&i, &json!({"a": "x"}), &no_render)
                .unwrap_err()
                .message,
            "Missing required input:b"
        );
        // The outcomes stay in declared order.
        let ids: Vec<_> = resolve_all(&i, &json!({}), &no_render)
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn a_dependency_must_itself_be_enabled_and_non_null() {
        let i = inputs(
            "
- id: a
  type: STRING
  required: false
- id: b
  type: STRING
  required: false
  dependsOn:
    inputs: [a]
- id: c
  type: INT
  dependsOn:
    inputs: [b]
",
        );
        // b is posted but disabled (a is null): c is disabled too.
        assert_eq!(
            Value::Object(resolve(&i, &json!({"b": "y"}), &no_render).unwrap()),
            json!({})
        );
        assert_eq!(
            resolve(&i, &json!({"a": "x", "b": "y"}), &no_render)
                .unwrap_err()
                .message,
            "Missing required input:c"
        );
    }

    #[test]
    fn a_dependency_with_a_default_or_an_error_counts_as_set() {
        let i = inputs(
            "
- id: a
  type: INT
  defaults: 5
- id: b
  type: INT
  dependsOn:
    inputs: [a]
",
        );
        // a's literal default makes it non-null: b is enabled.
        assert_eq!(
            resolve(&i, &json!({}), &no_render).unwrap_err().message,
            "Missing required input:b"
        );
        // An errored dependency keeps its raw value (Kestra): b is enabled,
        // and a's error is the one reported, being declared first.
        let all = resolve_all(&i, &json!({"a": "x"}), &no_render);
        assert!(all[1].enabled, "{all:?}");
        assert_eq!(
            resolve(&i, &json!({"a": "x"}), &no_render)
                .unwrap_err()
                .message,
            "Invalid value for input `a`. Cause: Cannot convert value 'x' to Integer"
        );
    }

    #[test]
    fn an_unknown_dependency_and_the_input_itself_are_ignored() {
        let i = inputs("- id: c\n  type: INT\n  dependsOn:\n    inputs: [nope, c]\n");
        assert_eq!(
            resolve(&i, &json!({}), &no_render).unwrap_err().message,
            "Missing required input:c"
        );
    }

    #[test]
    fn a_dependency_cycle_disables_and_terminates() {
        let i = inputs(
            "
- id: x
  type: INT
  dependsOn:
    inputs: [y]
- id: y
  type: INT
  dependsOn:
    inputs: [x]
",
        );
        assert_eq!(
            resolve(&i, &json!({"x": 1, "y": 2}), &no_render),
            Ok(Map::new())
        );
        assert!(resolve_all(&i, &json!({}), &no_render)
            .iter()
            .all(|o| !o.enabled));
    }

    const CONDITION: &str = "
- id: a
  type: STRING
- id: b
  type: INT
  dependsOn:
    inputs: [a]
    condition: \"{{ inputs.a == 'yes' }}\"
";

    #[test]
    fn a_condition_sees_the_dependencies_and_only_true_enables() {
        let i = inputs(CONDITION);
        let seen = std::cell::RefCell::new(Vec::new());
        let render = |t: &str, deps: &Map<String, Value>| -> Result<Value, String> {
            seen.borrow_mut()
                .push((t.to_string(), Value::Object(deps.clone())));
            Ok(Value::Bool(deps.get("a") == Some(&json!("yes"))))
        };
        assert_eq!(
            Value::Object(resolve(&i, &json!({"a": "no", "b": 1}), &render).unwrap()),
            json!({"a": "no"})
        );
        assert_eq!(
            seen.borrow()[0],
            ("{{ inputs.a == 'yes' }}".to_string(), json!({"a": "no"}))
        );
        assert_eq!(
            resolve(&i, &json!({"a": "yes"}), &render)
                .unwrap_err()
                .message,
            "Missing required input:b"
        );
        // A truthy non-boolean does not enable (`Boolean.TRUE.equals`).
        let truthy =
            |_: &str, _: &Map<String, Value>| -> Result<Value, String> { Ok(json!("true")) };
        assert_eq!(
            Value::Object(resolve(&i, &json!({"a": "yes"}), &truthy).unwrap()),
            json!({"a": "yes"})
        );
        // The condition is not rendered when a dependency is already null.
        seen.borrow_mut().clear();
        let _ = resolve(&i, &json!({}), &render);
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn a_failing_condition_disables_with_an_error_not_reported_at_start() {
        let i = inputs(CONDITION);
        let boom =
            |_: &str, _: &Map<String, Value>| -> Result<Value, String> { Err("boom".to_string()) };
        assert_eq!(
            Value::Object(resolve(&i, &json!({"a": "yes", "b": 1}), &boom).unwrap()),
            json!({"a": "yes"})
        );
        let all = resolve_all(&i, &json!({"a": "yes"}), &boom);
        assert!(!all[1].enabled);
        assert_eq!(all[1].errors, ["Invalid condition: boom"]);
    }

    #[test]
    fn a_templated_default_is_rendered_only_for_a_dependent() {
        let i = inputs(
            "
- id: a
  type: STRING
  defaults: \"{{ x }}\"
- id: b
  type: INT
  dependsOn:
    inputs: [a]
",
        );
        let value = |_: &str, _: &Map<String, Value>| -> Result<Value, String> { Ok(json!("v")) };
        assert_eq!(
            resolve(&i, &json!({}), &value).unwrap_err().message,
            "Missing required input:b"
        );
        // A default that does not render leaves the dependency null.
        assert_eq!(resolve(&i, &json!({}), &no_render), Ok(Map::new()));
        // Never stored (D2).
        assert_eq!(
            Value::Object(resolve(&i, &json!({"b": 1}), &value).unwrap()),
            json!({"b": 1})
        );
    }

    // --- SELECT / MULTISELECT expression and autoSelectFirst -----------------

    fn options(_: &str, _: &Map<String, Value>) -> Result<Value, String> {
        Ok(json!(["x", null, {"label": "Why", "value": "y"}]))
    }

    #[test]
    fn an_expression_gives_the_options() {
        let s = input("id: s\ntype: SELECT\nexpression: \" {{ opts }} \"\n");
        let check = |posted: Value, r: &dyn InputRenderer| {
            resolve(std::slice::from_ref(&s), &json!({"s": posted}), r)
                .map(Value::Object)
                .map_err(|e| e.message)
        };
        assert_eq!(check(json!("y"), &options), Ok(json!({"s": "y"})));
        assert_eq!(
            check(json!("z"), &options),
            Err("Invalid value for input `s`. Cause: it must match the values `[x, y]`".into())
        );
        // The expression is trimmed before it is rendered.
        let trimmed = |t: &str, _: &Map<String, Value>| -> Result<Value, String> {
            assert_eq!(t, "{{ opts }}");
            Ok(json!(["x"]))
        };
        assert_eq!(check(json!("x"), &trimmed), Ok(json!({"s": "x"})));
        let scalar = |_: &str, _: &Map<String, Value>| -> Result<Value, String> { Ok(json!("x")) };
        assert_eq!(
            check(json!("x"), &scalar),
            Err(
                "Invalid value for input `s`. Cause: s: Invalid expression result. Expected a \
                 list of values"
                    .into()
            )
        );
        let boom =
            |_: &str, _: &Map<String, Value>| -> Result<Value, String> { Err("boom".to_string()) };
        assert_eq!(
            check(json!("x"), &boom),
            Err(
                "Invalid value for input `s`. Cause: s: Cannot render 'expression'. Cause: boom"
                    .into()
            )
        );
        let no_value = |_: &str, _: &Map<String, Value>| -> Result<Value, String> {
            Ok(json!([{"label": "L"}]))
        };
        assert_eq!(
            check(json!("x"), &no_value),
            Err(
                "Invalid value for input `s`. Cause: Select option object must define a \
                 `value` field"
                    .into()
            )
        );
    }

    #[test]
    fn a_multiselect_expression_gives_the_options() {
        let m = input("id: m\ntype: MULTISELECT\nexpression: \"{{ opts }}\"\n");
        let r = resolve(
            std::slice::from_ref(&m),
            &json!({"m": ["x", "q"]}),
            &options,
        );
        assert_eq!(
            r.unwrap_err().message,
            "Invalid value for input `m`. Cause: value `q` doesn't match the values `[x, y]`"
        );
    }

    #[test]
    fn auto_select_first_picks_and_stores_the_first_option() {
        let check = |yaml: &str, posted: Value, r: &dyn InputRenderer| {
            let i = input(yaml);
            resolve(
                std::slice::from_ref(&i),
                &json!({ i.id.clone(): posted }),
                r,
            )
            .map(Value::Object)
            .map_err(|e| e.message)
        };
        let select = "id: s\ntype: SELECT\nautoSelectFirst: true\nvalues: [a, b]\n";
        assert_eq!(
            check(select, Value::Null, &no_render),
            Ok(json!({"s": "a"}))
        );
        assert_eq!(check(select, json!("b"), &no_render), Ok(json!({"s": "b"})));
        assert_eq!(
            check(
                "id: m\ntype: MULTISELECT\nautoSelectFirst: true\nvalues: [{label: A, value: a}, b]\n",
                Value::Null,
                &no_render
            ),
            Ok(json!({"m": ["a"]}))
        );
        // After an expression: the first rendered option.
        assert_eq!(
            check(
                "id: s\ntype: SELECT\nautoSelectFirst: true\nexpression: \"{{ o }}\"\n",
                Value::Null,
                &options
            ),
            Ok(json!({"s": "x"}))
        );
        // A declared default wins, and is not stored (D2).
        assert_eq!(
            check(
                "id: s\ntype: SELECT\nautoSelectFirst: true\ndefaults: b\nvalues: [a, b]\n",
                Value::Null,
                &no_render
            ),
            Ok(json!({}))
        );
        // Without it, a required SELECT is missing.
        assert_eq!(
            check(
                "id: s\ntype: SELECT\nvalues: [a, b]\n",
                Value::Null,
                &no_render
            ),
            Err("Missing required input:s".into())
        );
        let all = resolve_all(&[input(select)], &json!({}), &no_render);
        assert!(all[0].is_default);
    }
}
